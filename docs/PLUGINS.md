# Writing rirc plugins

rirc runs Python plugins in a separate **plugin host** process. Plugins load
from any folder named `plugins` in the usual places, or from directories you
pass explicitly. Dropping a `.py` file into a plugin folder is the whole
installation step.

Plugins are **trusted code**. There is no sandbox: a plugin can read files,
open sockets, start threads, and use any installed Python package. The separate
process exists so a buggy plugin can crash the host without taking down the
terminal UI or corrupting client state.

## Requirements

- Python 3.9 or newer is recommended.
- rirc looks for Python in this order:
  - `$RIRC_PYTHON`, if set (an executable path)
  - Windows: `py -3`, `python`, then `python3`
  - Linux/macOS: `python3`, then `python`
- If Python is missing, rirc still runs; `/plugins` explains why the host is
  unavailable.

## Where plugins live

rirc scans these directories (top-level `.py` files only):

1. `<directory containing rirc>/plugins`
2. `<current working directory>/plugins`
3. The per-user config directory:
   - Windows: `%APPDATA%\rirc\plugins`
   - macOS: `~/Library/Application Support/rirc/plugins`
   - Linux: `$XDG_CONFIG_HOME/rirc/plugins` or `~/.config/rirc/plugins`
4. Every directory in `$RIRC_PLUGIN_DIR` (use the OS path separator: `;` on
   Windows, `:` elsewhere)
5. Every `--plugin-dir <DIR>` passed on the command line

Files whose names start with `_` are ignored. Subdirectories are not scanned.

Set `RIRC_PLUGIN_DIR` or use `--plugin-dir` when developing outside the repo.
Use `--no-plugins` to disable the host entirely.

After adding, editing, or removing a plugin, run:

```text
/plugins reload
```

The host restarts, plugins are imported again, and commands are re-registered.
There is no per-plugin reload yet.

## The smallest plugin

```python
PLUGIN = {
    "name": "hello",
    "version": "1.0",
    "description": "Say hello",
}

def setup(api):
    @api.command("hello", help="Say hello", usage="/hello [name]")
    def hello(api, args):
        who = args.strip() or api.nick()
        api.send(f"Hello, {who}!")
```

Save it as `plugins/hello.py`, run `/plugins reload`, then type `/hello`.
`PLUGIN` is optional; the filename is used as the name if it is absent.

`setup(api)` runs once while the plugin is loading. Use it to register commands
and initialize state. Actions that need live buffers should wait for
`on_start`.

## Lifecycle

| Function | When it runs |
| --- | --- |
| module body | import time; keep this light |
| `setup(api)` | once during loading; register commands here |
| `on_start(api)` | once after rirc sends its first state snapshot |
| `on_stop(api)` | when the host shuts down or reloads |
| `on_active(api, active)` | when the focused buffer changes |
| `on_event(api, event, data)` | every event, in addition to the specific hook |

Example:

```python
def setup(api):
    api.log("loading")

def on_start(api):
    api.log("active buffer is", api.active().get("buffer"))

def on_stop(api):
    api.log("bye")
```

## Commands

Register slash commands with the decorator:

```python
def setup(api):
    @api.command("seen", help="Show the last message from a nick", usage="/seen <nick>")
    def seen(api, args):
        api.reply(f"looking up {args}")
```

- Command names are lowercased and must not contain spaces.
- Built-in rirc commands always win. Do not name a plugin command `/join`,
  `/msg`, `/plugins`, etc.
- Two plugins cannot register the same command; the second plugin's setup
  fails and the error is written to the plugin log.
- Plugin commands appear in tab completion.
- `/plugins` lists every loaded command.
- `args` is everything after the command, with surrounding whitespace trimmed.

## Context

Every hook and command gets an API object whose `context` describes where the
call came from:

```python
{
    "server": 0,                 # internal server id
    "server_name": "Libera.Chat",
    "buffer": "#rust",           # current buffer name
    "target": "#rust",           # original target (may be @#rust for STATUSMSG)
    "reply_target": "#rust",     # safe destination for replies, or None
    "kind": "channel",           # "channel", "query", or "server"
    "is_channel": True,
    "nick": "yournick",
    "topic": "...",
    "joined": True,
}
```

For private messages, `buffer`, `target`, and `reply_target` are the sender's
nick and `kind` is `"query"`. For server consoles there is no `reply_target`;
pass an explicit target to `send()`.

`api.current()` returns a copy. `api.context` is the live dictionary for the
current call. Timers receive a cloned API with the context captured when the
timer was scheduled.

## Sending IRC

All methods that send to a channel or nick use `reply_target` by default.

```python
api.send("hello")                       # current channel/query
api.send("hello", target="#other")      # explicit target
api.reply("hi")                         # alias for send()
api.notice("heads up")                  # NOTICE
api.me("waves")                         # CTCP ACTION
api.send_raw("MODE #chan +o alice")     # one raw line
api.join("#newchan")
api.join("#private", key="sekrit")
api.part("#newchan", reason="bye")
api.set_topic("New topic", channel="#chan")
api.whois("alice")
api.activate("#rust")                   # focus an existing buffer
api.copy("text for the clipboard")
```

- `send`/`notice`/`me` echo the message locally.
- Channel and query buffers are created automatically for `send`/`notice`/`me`.
- `send_raw` accepts exactly one IRC line; rirc rejects CR/LF/NUL and lines
  longer than 510 bytes.
- `api.notify(text, level="info")` shows a transient message in the bottom bar.
  `level` may be `"info"`, `"warn"`, or `"error"`.
- `api.log(*parts)` writes to the plugin log; this is the preferred debugging
  tool.

## Reading client state

rirc pushes a state snapshot on startup, when the active buffer changes, and
periodically after changes:

```python
api.state            # the raw snapshot dict
api.servers()        # [{id, host, port, tls, network, name, nick, connected, ...}]
api.buffers()        # [{id, server, name, kind, topic, unread, mentions, joined}]
api.channels()       # channel buffers only
api.active()         # the active context dict
api.users()          # members of the active channel, sorted by privilege
api.history("#rust", limit=30)   # recent messages cached by the host
api.nick()           # our nick on the current server
api.connected()      # current server connection state
```

Notes:

- `users()` currently exposes the **active** channel, capped at 2,000 members.
  Use `on_join`/`on_part`/`on_quit` if you need to follow other channels.
- `history()` returns messages seen by the host since it started, newest last.
  The snapshot seeds the active buffer with its last 100 messages. The host
  keeps up to 300 messages per buffer in memory. Reloading plugins discards
  that cache.
- `api.state` is a snapshot, not a live view; re-read it after events.

## Events

Implement any of these top-level functions. Each receives `(api, data)`, where
`data` is a dictionary. `on_event(api, event, data)` is called for all events
too, which is useful for logging or generic bridges.

| Event | Extra fields |
| --- | --- |
| `on_message` | `server`, `server_name`, `buffer`, `target`, `reply_target`, `nick`, `text`, `kind` (`message`/`action`/`notice`), `highlight`, `own`, `timestamp`, `is_channel`, `private` |
| `on_join` | `server`, `server_name`, `channel`, `nick`, `self`, `account`, `realname` |
| `on_part` | `server`, `server_name`, `channel`, `nick`, `self`, `reason` |
| `on_quit` | `server`, `server_name`, `nick`, `self`, `reason` |
| `on_nick` | `server`, `server_name`, `old`, `new`, `self` |
| `on_topic` | `server`, `server_name`, `channel`, `topic`, `nick` |
| `on_mode` | `server`, `server_name`, `target`, `modes`, `args`, `setter`, `is_channel` |
| `on_kick` | `server`, `server_name`, `channel`, `nick` (victim), `by`, `reason` |
| `on_away` | `server`, `server_name`, `nick`, `away` (string or null) |
| `on_account` | `server`, `server_name`, `nick`, `account` (string or null) |
| `on_connected` | `server`, `server_name`, `nick`, `sasl_required`, `authenticated` |
| `on_disconnected` | `server`, `server_name`, `reason`, `error` |
| `on_raw` | `server`, `server_name`, `line`, `direction: "in"` |

`on_message` fires for incoming messages only. Locally echoed messages are in
`api.history()` but do not trigger a hook, which prevents accidental reply
loops.

Example:

```python
def on_message(api, message):
    if message["own"]:
        return
    if "hello-plugin" in message["text"].lower():
        api.reply("👋")
```

## Timers

Timers run in daemon threads owned by the host:

```python
def setup(api):
    def announce(api):
        api.notify("five minutes elapsed")

    api.after(300, announce)      # one shot
    api.every(3600, announce)     # repeating

    @api.command("later")
    def later(api, args):
        api.after(10, lambda a: a.send("ten seconds later"))
```

- Callbacks receive a cloned API with the scheduling context.
- Exceptions in a timer are logged; they do not stop the timer loop.
- Timers stop when the host shuts down or reloads.
- Do not block in callbacks; long work blocks the host's event loop. Use your
  own Python threads for heavy jobs, and call the API from them (the writer is
  thread-safe).

## Persistent data

Each plugin gets a private, writable directory:

```python
import os, json

def setup(api):
    path = os.path.join(api.data_dir, "settings.json")
    ...
```

The directory is created automatically under the rirc config directory
(`.../rirc/plugin-data/<plugin-name>`). Use this instead of scattering files
next to the plugin script.

## Error handling and logs

- Import errors, `setup` errors, and exceptions in hooks/commands are caught
  per plugin and written to the plugin log with a traceback.
- A failing hook does not stop other plugins.
- A host crash stops all plugins; rirc shows a notice and clears the plugin
  command registry. `/plugins reload` starts it again.
- `print()` is redirected to stderr and ends up in the plugin log. Prefer
  `api.log()`.
- View logs with `/plugins logs`, or the “Plugin logs” action in `/plugins`.
- `/plugins` shows host status, Python version, plugin directories, loaded
  plugins, commands, and recent log lines.

## API quick reference

```python
api.plugin                         # plugin name
api.context                        # dict for the current call
api.current()                      # copy of context
api.data_dir                       # private writable directory

api.command(name, *, help="", usage="")   # decorator
api.send(text, target=None, server=None)
api.reply(text, **kwargs)
api.notice(text, target=None, server=None)
api.me(text, target=None, server=None)
api.send_raw(line, server=None)
api.join(channel, key=None, server=None)
api.part(channel=None, reason="", server=None)
api.set_topic(topic, channel=None, server=None)
api.whois(nick, server=None)
api.activate(target, server=None)
api.copy(text)
api.notify(text, level="info")
api.log(*parts)

api.state
api.servers()
api.buffers()
api.channels()
api.active()
api.users(target=None)
api.history(target=None, limit=50)
api.nick()
api.connected()

api.after(seconds, callback)
api.every(seconds, callback)
```

## Examples in this repo

The `plugins/` directory contains three loadable examples:

- `hello.py` — a command, a message hook, and `reply()`.
- `seen.py` — persistent `/seen <nick>` state in `api.data_dir`.
- `reminders.py` — `/remind 10m stretch` using `api.after`.

They load automatically when running from the repository. Copy them to your
config plugin folder if you want them available everywhere.

## Trust and security

Plugins execute arbitrary Python with the same OS permissions as rirc. Only
install plugins you trust. Do not run code from an untrusted channel or paste
bin into the plugin folder.

The host inherits rirc's environment, so environment variables such as
`RIRC_PASS` are visible to plugins. If that matters for your setup, launch rirc
with credentials supplied another way, or remove the variable before starting
rirc.

## Limitations

- One plugin file = one module. Python packages and subdirectories are not
  discovered automatically.
- Reload is global, not per plugin.
- State snapshots are throttled; `users()` is limited to the active channel.
- History is in-memory in the host and is lost on reload.
- There is no sandbox and no permission model.
- No plugin API for editing the TUI layout or drawing custom widgets (yet).
