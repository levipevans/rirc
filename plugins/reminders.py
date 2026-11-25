"""Timers and a small in-memory reminder queue.

Try:
    /remind 10m stretch
    /remind 30s check the build
    /reminders
"""

import re
import threading
import time

PLUGIN = {
    "name": "reminders",
    "version": "1.0",
    "description": "Schedule /remind alerts with API timers",
}

_LOCK = threading.Lock()
_REMINDERS = []
_NEXT_ID = 1


def _parse_duration(value):
    match = re.fullmatch(r"(\d+)([smhd]?)", value.strip().lower())
    if not match:
        return None
    amount = int(match.group(1))
    unit = match.group(2) or "s"
    factor = {"s": 1, "m": 60, "h": 3600, "d": 86400}[unit]
    return amount * factor


def _send(api, text):
    target = api.context.get("reply_target")
    if target:
        api.send(text)
    else:
        api.notify(text)


def setup(api):
    @api.command(
        "remind",
        help="Schedule a reminder",
        usage="/remind <10s|5m|2h> <text>",
    )
    def remind(api, args):
        global _NEXT_ID
        parts = args.strip().split(None, 1)
        if len(parts) != 2:
            api.notify("usage: /remind <10s|5m|2h> <text>")
            return
        seconds = _parse_duration(parts[0])
        text = parts[1]
        if seconds is None or seconds <= 0:
            api.notify("that is not a duration I understand")
            return

        with _LOCK:
            job_id = _NEXT_ID
            _NEXT_ID += 1
            entry = {
                "id": job_id,
                "due": time.time() + seconds,
                "text": text,
                "target": api.context.get("reply_target"),
            }
            _REMINDERS.append(entry)

        def fire(api):
            with _LOCK:
                _REMINDERS[:] = [item for item in _REMINDERS if item["id"] != job_id]
            _send(api, f"⏰ Reminder: {text}")

        api.after(seconds, fire)
        api.notify(f"Reminder #{job_id} set for {time.strftime('%H:%M:%S', time.localtime(entry['due']))}")

    @api.command("reminders", help="List pending reminders", usage="/reminders")
    def reminders(api, args):
        with _LOCK:
            entries = list(_REMINDERS)
        if not entries:
            api.notify("no pending reminders")
            return
        for entry in entries:
            due = time.strftime("%H:%M:%S", time.localtime(entry["due"]))
            _send(api, f"#{entry['id']} at {due}: {entry['text']}")
