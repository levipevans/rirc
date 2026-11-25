"""Persistent /seen tracking.

Records the last thing each nick said and answers `/seen <nick>`. Data lives in
the per-plugin directory supplied by rirc, so it survives restarts.

Try:
    /seen alice
"""

import json
import os
import time

PLUGIN = {
    "name": "seen",
    "version": "1.0",
    "description": "Remember the last message from each nick",
}

_DATA = {}
_PATH = None


def _load(path):
    global _DATA, _PATH
    _PATH = path
    try:
        with open(path, "r", encoding="utf-8") as handle:
            data = json.load(handle)
        if isinstance(data, dict):
            _DATA = data
    except FileNotFoundError:
        _DATA = {}
    except Exception:
        _DATA = {}


def _save():
    if not _PATH:
        return
    tmp = _PATH + ".tmp"
    try:
        with open(tmp, "w", encoding="utf-8") as handle:
            json.dump(_DATA, handle, ensure_ascii=False, indent=2)
        os.replace(tmp, _PATH)
    except Exception:
        pass


def setup(api):
    _load(os.path.join(api.data_dir, "seen.json"))

    @api.command("seen", help="Show the last message from a nick", usage="/seen <nick>")
    def seen(api, args):
        nick = args.strip()
        if not nick:
            api.notify("usage: /seen <nick>")
            return
        entry = _DATA.get(nick.lower())
        if not entry:
            message = f"I have not seen {nick} say anything yet."
        else:
            stamp = time.strftime("%Y-%m-%d %H:%M:%S", time.localtime(entry.get("when", 0)))
            message = (
                f"{entry.get('nick', nick)} was last seen at {stamp} "
                f"in {entry.get('target', '?')}: {entry.get('text', '')}"
            )
        target = api.context.get("reply_target")
        if target:
            api.send(message)
        else:
            api.notify(message)


def on_message(api, message):
    if message.get("own"):
        return
    nick = message.get("nick") or ""
    text = message.get("text") or ""
    if not nick or not text:
        return
    _DATA[nick.lower()] = {
        "nick": nick,
        "when": message.get("timestamp") or time.time(),
        "target": message.get("buffer") or message.get("target") or "?",
        "text": text,
    }
    _save()
