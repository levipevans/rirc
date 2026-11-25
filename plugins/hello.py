"""A tiny example plugin.

Install: copy this file into a `plugins/` folder next to rirc (or in the
config directory), then run `/plugins reload`.

Try:
    /hello
    /hello world

If someone says "hello-plugin" in a channel, the plugin replies using the
event's reply target.
"""

PLUGIN = {
    "name": "hello",
    "version": "1.0",
    "description": "Minimal command, event and reply example",
}


def setup(api):
    @api.command("hello", help="Say hello from Python", usage="/hello [name]")
    def hello(api, args):
        who = args.strip() or api.nick() or "friend"
        target = api.context.get("reply_target")
        text = f"Hello, {who}! This message came from a Python plugin."
        if target:
            api.send(text)
        else:
            api.notify(text)


def on_start(api):
    api.log("hello plugin ready; active buffer:", api.active().get("buffer", "?"))


def on_message(api, message):
    if message.get("own"):
        return
    if "hello-plugin" in message.get("text", "").lower():
        api.reply("👋 hello from the example plugin")
