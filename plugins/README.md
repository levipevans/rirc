# Plugins

Drop a `.py` file in this folder and run `/plugins reload` inside rirc.

- `hello.py` — minimal command, message hook, and reply example.
- `seen.py` — persistent `/seen <nick>` tracking.
- `reminders.py` — `/remind 10m …` using plugin timers.

Full author documentation: [`docs/PLUGINS.md`](../docs/PLUGINS.md).

The host process is optional: if Python 3 is not available, rirc still runs and
`/plugins` explains why plugins are unavailable. Plugin stdout is redirected to
stderr; `print()` will show up in the plugin log.
