#!/usr/bin/env python3
"""rirc plugin host.

This file is embedded in the rirc binary and written to a temporary path at
startup. It is not meant to be launched by hand, though it can be for testing:

    python plugin_host.py --plugin-dir ./plugins --data-dir ./plugin-data

Protocol: newline-delimited JSON on stdin/stdout. Plugin stdout is redirected
to stderr so accidental print() calls cannot corrupt the protocol.
"""

from __future__ import annotations

import argparse
import importlib.util
import json
import os
import re
import sys
import threading
import time
import traceback

PROTOCOL_OUT = sys.stdout
try:
    sys.stdin.reconfigure(encoding="utf-8")
    sys.stderr.reconfigure(encoding="utf-8")
    PROTOCOL_OUT.reconfigure(encoding="utf-8", newline="\n")
except Exception:
    pass
# Plugins may print for debugging; keep the protocol stream clean.
sys.stdout = sys.stderr


class Writer:
    def __init__(self) -> None:
        self._lock = threading.Lock()

    def send(self, payload: dict) -> None:
        data = json.dumps(payload, ensure_ascii=False, separators=(",", ":"))
        with self._lock:
            try:
                PROTOCOL_OUT.write(data + "\n")
                PROTOCOL_OUT.flush()
            except Exception:
                # The client has gone away.
                pass


class State:
    def __init__(self) -> None:
        self.data: dict = {}
        self.history: dict[str, list[dict]] = {}

    def update(self, data: dict) -> None:
        self.data = data
        active = (data.get("active") or {}).get("buffer")
        messages = data.get("messages") or []
        if active:
            self.history[active] = list(messages)

    def add_message(self, event: dict) -> None:
        buffer = event.get("buffer")
        if not buffer:
            return
        entries = self.history.setdefault(buffer, [])
        entries.append(event)
        if len(entries) > 300:
            del entries[:-300]

    def messages(self, target: str | None, limit: int) -> list[dict]:
        if not target:
            active = (self.data.get("active") or {}).get("buffer")
            target = active
        if not target:
            return []
        return list(self.history.get(target, []))[-max(0, int(limit)) :]


class Registry:
    def __init__(self, writer: Writer) -> None:
        self.writer = writer
        self.commands: dict[str, tuple[str, object, str, str]] = {}

    def register(self, plugin: str, name: str, handler, help_text: str, usage: str) -> None:
        name = name.strip().lower().lstrip("/")
        if not name or any(ch.isspace() for ch in name):
            raise ValueError(f"invalid command name: {name!r}")
        if name in self.commands:
            other = self.commands[name][0]
            raise ValueError(f"command /{name} is already registered by {other}")
        self.commands[name] = (plugin, handler, str(help_text), str(usage))
        self.writer.send(
            {
                "type": "command",
                "plugin": plugin,
                "name": name,
                "help": str(help_text),
                "usage": str(usage),
            }
        )

    def unregister_plugin(self, plugin: str) -> None:
        for name, (owner, _, _, _) in list(self.commands.items()):
            if owner == plugin:
                del self.commands[name]

    def describe(self) -> list[dict]:
        return [
            {"name": name, "plugin": owner, "help": help_text, "usage": usage}
            for name, (owner, _, help_text, usage) in sorted(self.commands.items())
        ]


class PluginAPI:
    def __init__(
        self,
        plugin: str,
        writer: Writer,
        state: State,
        registry: Registry,
        data_dir: str,
        shutdown: threading.Event,
        context: dict | None = None,
    ) -> None:
        self.plugin = plugin
        self._writer = writer
        self._state = state
        self._registry = registry
        self._shutdown = shutdown
        self.data_dir = data_dir
        self.context = dict(context or {})
        for key in ("data_dir", "plugin"):
            self.context.setdefault(key, data_dir if key == "data_dir" else plugin)

    # -- context -------------------------------------------------------
    def _clone(self, context: dict) -> "PluginAPI":
        return PluginAPI(
            self.plugin,
            self._writer,
            self._state,
            self._registry,
            self.data_dir,
            self._shutdown,
            context,
        )

    def current(self) -> dict:
        return dict(self.context)

    # -- commands ------------------------------------------------------
    def command(self, name: str, *, help: str = "", usage: str = ""):
        """Decorator: register a slash command handled by this plugin."""

        def decorator(fn):
            self._registry.register(self.plugin, name, fn, help, usage)
            return fn

        return decorator

    # -- outgoing IRC --------------------------------------------------
    def _action(self, action: str, **data) -> None:
        payload = {"type": "action", "plugin": self.plugin, "action": action, "data": data}
        self._writer.send(payload)

    def _target(self, target: str | None) -> str | None:
        if target:
            return str(target)
        return self.context.get("reply_target") or self.context.get("buffer")

    def _server(self, server: int | None) -> int | None:
        if server is not None:
            return int(server)
        value = self.context.get("server")
        return int(value) if value is not None else None

    def send(self, text: str, target: str | None = None, server: int | None = None) -> None:
        """Send a PRIVMSG to a channel or nick."""
        dest = self._target(target)
        if not dest:
            raise ValueError("send() needs a target outside an event context")
        self._action("send_message", server=self._server(server), target=dest, text=str(text))

    def reply(self, text: str, **kwargs) -> None:
        """Reply to the current message/command context."""
        self.send(text, **kwargs)

    def notice(self, text: str, target: str | None = None, server: int | None = None) -> None:
        dest = self._target(target)
        if not dest:
            raise ValueError("notice() needs a target")
        self._action("notice", server=self._server(server), target=dest, text=str(text))

    def me(self, text: str, target: str | None = None, server: int | None = None) -> None:
        dest = self._target(target)
        if not dest:
            raise ValueError("me() needs a target")
        self._action("me", server=self._server(server), target=dest, text=str(text))

    def send_raw(self, line: str, server: int | None = None) -> None:
        """Send one raw IRC line. rirc still validates CR/LF/NUL and length."""
        self._action("send_raw", server=self._server(server), line=str(line))

    def join(self, channel: str, key: str | None = None, server: int | None = None) -> None:
        self._action("join", server=self._server(server), channel=str(channel), key=key)

    def part(self, channel: str | None = None, reason: str = "", server: int | None = None) -> None:
        channel = channel or self.context.get("buffer") or self.context.get("target")
        if not channel:
            raise ValueError("part() needs a channel")
        self._action("part", server=self._server(server), channel=str(channel), reason=str(reason))

    def set_topic(self, topic: str, channel: str | None = None, server: int | None = None) -> None:
        channel = channel or self.context.get("buffer")
        if not channel:
            raise ValueError("set_topic() needs a channel")
        self._action("set_topic", server=self._server(server), channel=str(channel), text=str(topic))

    def whois(self, nick: str, server: int | None = None) -> None:
        self._action("whois", server=self._server(server), nick=str(nick))

    def activate(self, target: str, server: int | None = None) -> None:
        self._action("activate", server=self._server(server), target=str(target))

    def copy(self, text: str) -> None:
        self._action("copy", text=str(text))

    def notify(self, text: str, level: str = "info") -> None:
        self._writer.send(
            {
                "type": "notify",
                "plugin": self.plugin,
                "text": str(text),
                "level": str(level),
            }
        )

    def log(self, *parts) -> None:
        text = " ".join(str(part) for part in parts)
        self._writer.send({"type": "log", "plugin": self.plugin, "text": text})

    # -- local state ---------------------------------------------------
    @property
    def state(self) -> dict:
        return self._state.data

    def servers(self) -> list[dict]:
        return list(self._state.data.get("servers") or [])

    def buffers(self) -> list[dict]:
        return list(self._state.data.get("buffers") or [])

    def channels(self) -> list[dict]:
        return [b for b in self.buffers() if b.get("kind") == "channel"]

    def active(self) -> dict:
        return dict(self._state.data.get("active") or {})

    def users(self, target: str | None = None) -> list[dict]:
        """Members of the active channel from the latest state snapshot."""
        active = self.active()
        if target and target != active.get("buffer") and target != active.get("target"):
            return []
        return list(self._state.data.get("users") or [])

    def history(self, target: str | None = None, limit: int = 50) -> list[dict]:
        """Recent messages for a buffer seen by the host (newest last)."""
        return self._state.messages(target or self.context.get("buffer"), limit)

    def nick(self) -> str:
        server = self.context.get("server")
        for entry in self.servers():
            if entry.get("id") == server:
                return str(entry.get("nick") or "")
        active = self.active()
        return str(active.get("nick") or "")

    def connected(self) -> bool:
        server = self.context.get("server")
        for entry in self.servers():
            if entry.get("id") == server:
                return bool(entry.get("connected"))
        return False

    # -- timers --------------------------------------------------------
    def after(self, seconds: float, callback) -> None:
        """Run callback(api) once after `seconds`."""
        self._schedule(seconds, callback, repeat=False)

    def every(self, seconds: float, callback) -> None:
        """Run callback(api) every `seconds` until shutdown."""
        self._schedule(seconds, callback, repeat=True)

    def _schedule(self, seconds: float, callback, repeat: bool) -> None:
        delay = max(0.05, float(seconds))
        context = dict(self.context)

        def invoke() -> None:
            api = self._clone(context)
            try:
                callback(api)
            except Exception:
                self._writer.send(
                    {
                        "type": "error",
                        "plugin": self.plugin,
                        "text": traceback.format_exc(),
                    }
                )

        if repeat:

            def loop() -> None:
                while not self._shutdown.wait(delay):
                    invoke()

            thread = threading.Thread(target=loop, name=f"rirc-timer-{self.plugin}", daemon=True)
        else:
            thread = threading.Timer(delay, invoke)

        thread.daemon = True
        thread.start()


class Plugin:
    def __init__(self, name: str, version: str, description: str, path: str, api: PluginAPI, module):
        self.name = name
        self.version = version
        self.description = description
        self.path = path
        self.api = api
        self.module = module


class Host:
    def __init__(self, writer: Writer, state: State, registry: Registry, shutdown: threading.Event, data_root: str):
        self.writer = writer
        self.state = state
        self.registry = registry
        self.shutdown = shutdown
        self.data_root = data_root
        self.plugins: list[Plugin] = []
        self.started = False
        self.pending: list[tuple[str, dict]] = []
        self.last_active: str | None = None

    def data_dir_for(self, name: str) -> str:
        safe = re.sub(r"[^A-Za-z0-9_.-]+", "_", name).strip("._") or "plugin"
        path = os.path.join(self.data_root, safe)
        os.makedirs(path, exist_ok=True)
        return path

    def load(self, directories: list[str]) -> None:
        seen_names = set()
        for directory in directories:
            if not os.path.isdir(directory):
                continue
            for entry in sorted(os.listdir(directory)):
                if not entry.endswith(".py") or entry.startswith("_"):
                    continue
                path = os.path.join(directory, entry)
                stem = os.path.splitext(entry)[0]
                name = stem
                try:
                    spec = importlib.util.spec_from_file_location(
                        f"rirc_plugin_{stem}_{abs(hash(path))}", path
                    )
                    if spec is None or spec.loader is None:
                        raise RuntimeError("could not create import spec")
                    module = importlib.util.module_from_spec(spec)
                    spec.loader.exec_module(module)
                    meta = getattr(module, "PLUGIN", {}) or {}
                    name = str(meta.get("name") or stem)
                    if name in seen_names:
                        raise RuntimeError(f"duplicate plugin name: {name}")
                    seen_names.add(name)
                    api = PluginAPI(
                        name,
                        self.writer,
                        self.state,
                        self.registry,
                        self.data_dir_for(name),
                        self.shutdown,
                    )
                    setup = getattr(module, "setup", None)
                    if setup is not None:
                        setup(api)
                    self.plugins.append(
                        Plugin(
                            name,
                            str(meta.get("version") or ""),
                            str(meta.get("description") or ""),
                            path,
                            api,
                            module,
                        )
                    )
                except Exception:
                    self.registry.unregister_plugin(name)
                    self.writer.send(
                        {
                            "type": "error",
                            "plugin": name,
                            "text": traceback.format_exc(),
                        }
                    )

    def send_ready(self) -> None:
        self.writer.send(
            {
                "type": "ready",
                "python": sys.version.split()[0],
                "plugins": [
                    {
                        "name": p.name,
                        "version": p.version,
                        "description": p.description,
                        "path": p.path,
                    }
                    for p in self.plugins
                ],
                "commands": self.registry.describe(),
            }
        )

    def context_from(self, data: dict) -> dict:
        context = dict(data or {})
        context.setdefault("plugin", "")
        return context

    def dispatch(self, event: str, data: dict) -> None:
        if not self.started:
            if len(self.pending) < 200:
                self.pending.append((event, data))
            return
        for plugin in self.plugins:
            fn = getattr(plugin.module, f"on_{event}", None)
            if fn is None:
                continue
            api = plugin.api._clone(self.context_from(data))
            try:
                fn(api, data)
            except Exception:
                self.writer.send(
                    {
                        "type": "error",
                        "plugin": plugin.name,
                        "text": traceback.format_exc(),
                    }
                )
        for plugin in self.plugins:
            fn = getattr(plugin.module, "on_event", None)
            if fn is None:
                continue
            api = plugin.api._clone(self.context_from(data))
            try:
                fn(api, event, data)
            except Exception:
                self.writer.send(
                    {
                        "type": "error",
                        "plugin": plugin.name,
                        "text": traceback.format_exc(),
                    }
                )

    def state_update(self, data: dict) -> None:
        self.state.update(data)
        active = data.get("active") or {}
        active_buffer = active.get("buffer")
        if not self.started:
            self.started = True
            self.last_active = active_buffer
            for plugin in self.plugins:
                fn = getattr(plugin.module, "on_start", None)
                if fn is None:
                    continue
                api = plugin.api._clone(self.context_from(active))
                try:
                    fn(api)
                except Exception:
                    self.writer.send(
                        {
                            "type": "error",
                            "plugin": plugin.name,
                            "text": traceback.format_exc(),
                        }
                    )
            pending, self.pending = self.pending, []
            for event, event_data in pending:
                self.dispatch(event, event_data)
        elif active_buffer != self.last_active:
            self.last_active = active_buffer
            for plugin in self.plugins:
                fn = getattr(plugin.module, "on_active", None)
                if fn is None:
                    continue
                api = plugin.api._clone(self.context_from(active))
                try:
                    fn(api, active)
                except Exception:
                    self.writer.send(
                        {
                            "type": "error",
                            "plugin": plugin.name,
                            "text": traceback.format_exc(),
                        }
                    )

    def command(self, name: str, args: str, data: dict) -> None:
        entry = self.registry.commands.get(name)
        if entry is None:
            self.writer.send(
                {
                    "type": "log",
                    "plugin": None,
                    "text": f"unknown plugin command: /{name}",
                }
            )
            return
        plugin_name, handler, _help, _usage = entry
        plugin = next((p for p in self.plugins if p.name == plugin_name), None)
        if plugin is None:
            return
        api = plugin.api._clone(self.context_from(data))
        try:
            handler(api, args)
        except Exception:
            self.writer.send(
                {
                    "type": "error",
                    "plugin": plugin.name,
                    "text": traceback.format_exc(),
                }
            )

    def stop(self) -> None:
        for plugin in self.plugins:
            fn = getattr(plugin.module, "on_stop", None)
            if fn is None:
                continue
            api = plugin.api._clone(self.context_from(self.state.data.get("active") or {}))
            try:
                fn(api)
            except Exception:
                self.writer.send(
                    {
                        "type": "error",
                        "plugin": plugin.name,
                        "text": traceback.format_exc(),
                    }
                )
        self.shutdown.set()


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--plugin-dir", action="append", default=[])
    parser.add_argument("--data-dir", required=True)
    args = parser.parse_args()

    writer = Writer()
    state = State()
    registry = Registry(writer)
    shutdown = threading.Event()
    os.makedirs(args.data_dir, exist_ok=True)
    host = Host(writer, state, registry, shutdown, args.data_dir)
    host.load(args.plugin_dir)
    host.send_ready()

    try:
        for raw in sys.stdin:
            if shutdown.is_set():
                break
            raw = raw.strip()
            if not raw:
                continue
            try:
                message = json.loads(raw)
            except Exception:
                print(f"rirc plugin host: invalid JSON: {raw!r}", file=sys.stderr)
                continue
            kind = message.get("type")
            if kind == "state":
                host.state_update(message.get("data") or {})
            elif kind == "event":
                event = str(message.get("event") or "")
                data = message.get("data") or {}
                if event == "message":
                    state.add_message(data)
                host.dispatch(event, data)
            elif kind == "command":
                host.command(
                    str(message.get("name") or ""),
                    str(message.get("args") or ""),
                    message.get("data") or {},
                )
            elif kind == "shutdown":
                break
    except KeyboardInterrupt:
        pass
    except Exception:
        print(traceback.format_exc(), file=sys.stderr)
    finally:
        host.stop()
    return 0


if __name__ == "__main__":
    sys.exit(main())
