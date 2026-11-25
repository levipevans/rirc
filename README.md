# rirc

A small IRCv3 client for the terminal with a three-column TUI and vim-style
modal editing. Rust, `ratatui`, no daemon, no config file.

```
 ┌─ buffers ──────────┐ ┌─ #rust ─────────────────────────────┐ ┌─ members 8 ─┐
 │ ▾ ● Libera.Chat    │ │ [18:34] alice  welcome aboard       │ │ @carol      │
 │   #rust            │ │ [18:35] vimmer thanks!              │ │ @ferris     │
 │   #vim          4  │ │ [18:36] bob    try the easy label   │ │ +alice      │
 │   alice        @1  │ │ [18:38] * bob sips coffee           │ │  dave       │
 └────────────────────┘ └─────────────────────────────────────┘ └─────────────┘
  NORMAL · j/k move · Enter menu · v select · y yank · i write · Tab panel
```

- **Three columns**: servers/channels/queries, messages, channel members.
- **Modal**: `NORMAL` to move and act, `TEXT` to type, `VISUAL` to select a
  block of messages, `COMMAND` for IRC commands.
- **Menus everywhere**: press Enter on a server, channel, member or message to
  get an info + actions popup. Includes whois, topic changes, op/voice, kick,
  copy, reply, join/leave and more.
- **Yank**: `y` in normal mode copies the selected message; `v`, move, `y`
  copies a multi-message selection with timestamps.
- **Channel browser**: `/list` collects the network's channel list into a
  filterable full-screen view; `l` joins, `Enter` opens a channel menu.
- **Python plugins**: drop a `.py` file in a `plugins/` folder, run
  `/plugins reload`, and add commands, hooks, timers and IRC actions. See
  [docs/PLUGINS.md](docs/PLUGINS.md).
- IRCv3: `CAP LS 302`, message tags, server-time, multi-prefix, away-notify,
  extended-join, account-tag, SASL PLAIN, TLS. Local echo, unread/mention
  badges, member modes, WHOIS popups, mouse support.

## Download

Prebuilt Windows (x86_64) builds are attached to the
[latest release](https://github.com/levipevans/rirc/releases/latest).
On Linux and macOS, build from source with `cargo build --release`.

## Build & run

```sh
cargo run --release -- --demo
```

`--demo` opens a fully populated local session; nothing is sent over the
network.

Connect for real:

```sh
cargo run --release -- -s irc.libera.chat -n mynick -c '#rust,#vim'
```

The binary is `target/release/rirc.exe` (Windows) or `target/release/rirc`.
TLS is used by default (port 6697). For systems without a system certificate
store setup, `native-tls` uses schannel on Windows and Security.framework on
macOS; on Linux install OpenSSL development headers. `PASS` and SASL
credentials are refused unless TLS is enabled, and existing connections are
not reused when the requested transport differs.

### Options

```
-s, --server <HOST[:PORT]>   connect on startup (TLS by default)
-n, --nick <NAME>            nickname (default $RIRC_NICK or $USERNAME)
    --port <PORT>            port override (6667 defaults to plaintext)
    --tls / --no-tls         force TLS on/off
    --sasl <USER:PASS>       SASL PLAIN; omit PASS to read $RIRC_PASS
    --pass <PASS>            server password (PASS command)
-c, --channel <CHANNEL>      auto-join; repeat or comma-separate
    --plugin-dir <DIR>       extra plugin directory; repeatable
    --no-plugins             disable the Python plugin host
    --demo                   local sample session
    --snapshot               render demo frames to stdout and exit
-h, --help                   usage
-V, --version                version
```

Inside the client, use `/connect host[:port] [tls|notls]` to add another
network. Multiple servers can be connected at once.

## Modes & keys

| Mode | How | Keys |
| --- | --- | --- |
| **NORMAL** | `Esc` | `j`/`k` move, `g`/`G` ends, `Ctrl-d`/`Ctrl-u` page, `Tab`/`Shift-Tab` panel, `Enter` menu, `l` open buffer/message, `v` visual, `y` yank, `i`/`a` text, `:` command, `/` slash command, `Ctrl-n`/`Ctrl-p` next/prev buffer, `?` help, `Q` quit |
| **TEXT** | `i` or `a` | type, `Enter` send, `Tab` complete nick, `↑`/`↓` history, `Ctrl-w`/`Ctrl-u`/`Ctrl-a`/`Ctrl-e`, `Esc` normal |
| **VISUAL** | `v` in messages | `j`/`k` extend, `g`/`G` extend to ends, `o` swap ends, `y` yank + exit, `Enter` menu for the block, `Esc` cancel |
| **COMMAND** | `:` or `/` | type a command, `Tab` completes command names, `Enter` run, `Esc` cancel |

In the buffer column `Enter` opens the channel/server menu; in the member
column it opens the user menu. `l` opens the selected buffer/conversation.
Mouse clicks select; the wheel moves whichever panel has focus (while typing,
it scrolls the message view).

### Menus

- **Server**: open console, join a channel, change nick, reconnect/disconnect,
  copy address, quit.
- **Channel**: topic + modes + member count, open, copy/change topic, leave,
  clear, close.
- **Member**: role/away/account/host, message, whois, mention, copy nickname,
  op/voice, kick.
- **Message**: sender/time/kind, copy text, copy with timestamps, reply,
  open/whois the sender.
- **Help**: `?`, also reachable from the local welcome buffer.

Menus are keyboard driven: `j`/`k` move, `Enter` select, `y` copy the info
text, digits jump, `Esc` closes.

## Plugins

Drop a Python file into a `plugins/` folder next to the executable, in the
current directory, or in the per-user config directory, then run
`/plugins reload`. Plugins can register slash commands, react to messages and
joins, read recent state, send IRC messages, use timers, and store persistent
data.

```python
PLUGIN = {"name": "hello", "version": "1.0"}
def setup(api):
    @api.command("hello")
    def hello(api, args):
        api.reply("hello from Python")
```

- Host status, commands and errors: `/plugins`
- Logs: `/plugins logs`
- Reload: `/plugins reload`
- Full API and installation guide: [docs/PLUGINS.md](docs/PLUGINS.md)
- Examples: [`plugins/`](plugins/)

Python 3 is optional. If it is unavailable, rirc runs normally and `/plugins`
explains the problem. Plugins are trusted code; they are not sandboxed.

## Clipboard

Yank writes to the native clipboard (`clip.exe`, `pbcopy`, `wl-copy`,
`xclip`, `xsel`) **and** emits an OSC 52 sequence. OSC 52 is what makes copy
work over SSH and inside tmux/WezTerm/Windows Terminal/iTerm2. If your
terminal needs it, set `OSC52=c` (the default) or an appropriate selector.
For tmux, enable `set -g allow-passthrough on` so the sequence reaches the
outer terminal.

## Commands

```
/join #chan[,#chan] [keys]   /part [#chan] [reason]    /me <action>
/msg <nick> [text]       /query <nick>             /nick <newnick>
/topic [text]            /mode <target> <modes>    /whois <nick>
/away [reason]           /invite <nick> [#chan]    /kick <nick> [reason]
/op /deop /voice /devoice <nick>                   /clear
/list [server-side-filter]
/close                   /connect <host[:port]>    /disconnect
/reconnect               /raw <line>               /help
/plugins [reload|logs]
/quit
```

`/list` accepts whatever your network supports after the command, for example
`/list >20`, `/list <50`, or `/list #rust,#vim`. The results open in a
full-screen browser: `j`/`k` move, `Enter` opens the channel menu, `l` joins,
`f` or `/` filters locally, `y` copies the channel name, `r` refreshes, and
`Esc` closes (or clears an active filter first).

On a server console buffer, plain text is sent raw to the server (power-user
escape hatch).

## Testing

```sh
cargo test
```

The suite covers protocol parsing, wrapping, incremental state changes, CLI
parsing, mode transitions and headless render snapshots at several terminal
sizes. Two opt-in tests talk to Libera.Chat to exercise TLS and full
CAP/registration:

```sh
cargo test -- --ignored --nocapture connects_to_libera
cargo test -- --ignored --nocapture registers_with_libera
```

Run those sparingly: Libera can temporarily network-ban automated probe
connections. They are ignored by default for exactly that reason.

`--snapshot` is handy for checking layout without a TTY:

```sh
cargo run -- --snapshot
```

## Layout of the code

| File | Responsibility |
| --- | --- |
| `src/main.rs` | CLI, terminal lifecycle, event loop |
| `src/app.rs` | state, buffering, IRC event handling, commands, menus, mouse |
| `src/modes/` | one module per mode (`normal`, `visual`, `insert`) |
| `src/ui.rs` | layout, message rows, sidebars, input, popups, snapshots |
| `src/irc.rs` | wire parser, CAP/SASL state helpers, async TLS connection task |
| `src/model.rs` | servers, buffers, messages, members, whois |
| `src/menu.rs` | menu model, actions, help text |
| `src/list.rs` | `/list` results, filtering and selection |
| `src/input.rs` | single-line editor, history, completion primitives |
| `src/clipboard.rs` | native + OSC 52 clipboard |
| `src/plugins.rs` | Python host process, protocol, command registry |
| `src/plugin_host.py` | embedded Python host and plugin API runtime |
| `docs/PLUGINS.md` | plugin author documentation |
| `plugins/` | loadable example plugins |
| `src/demo.rs` | deterministic sample session |
| `src/util.rs` | wrapping, case folding, mention detection, colors |

## Limitations & notes

- One line of input; pasted newlines become spaces.
- No chat history replay (IRCv3 `chathistory`), DCC, file transfer, or
  scripting.
- Completions are nick/command prefix completion, not full fuzzy search.
- Menus are modal overlays; clicking menu rows is not implemented (use keys).
- Clipboard helpers are best-effort: some Linux setups need `wl-clipboard` or
  `xclip`; OSC 52 support depends on the terminal.
- Long outgoing messages are split into ~420-character chunks.
