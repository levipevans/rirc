//! Deterministic sample session, used by `--demo` and the render tests.

use chrono::{Duration, Local};

use crate::app::App;
use crate::list::ListEntry;
use crate::model::{BufferKind, Member, Message, MsgKind, Server};

fn at(minutes: i64) -> chrono::DateTime<Local> {
    Local::now() - Duration::minutes(minutes)
}

/// Sample results for `/list` in demo mode.
pub fn channel_list() -> Vec<ListEntry> {
    [
        (
            "#rust",
            2481u32,
            "The Rust programming language — ask boldly, answer kindly",
        ),
        (
            "#rust-beginners",
            640,
            "New to Rust? Start here, no question is too small",
        ),
        ("#ratatui", 512, "Terminal user interfaces in Rust"),
        (
            "#vim",
            1902,
            "The venerable text editor · :help is your friend",
        ),
        (
            "#neovim",
            3140,
            "Vim-fork focused on extensibility and usability",
        ),
        ("#emacs", 1210, "The extensible, self-documenting editor"),
        (
            "#linux",
            5210,
            "General Linux discussion · be excellent to each other",
        ),
        ("#archlinux", 1870, "Arch Linux support and discussion"),
        ("#debian", 1320, "The universal operating system"),
        ("#python", 4102, "The Python programming language"),
        ("#typescript", 1200, "Typed JavaScript at scale"),
        ("#go", 1890, "The Go programming language"),
        ("#c", 1650, "C programming, from embedded to kernels"),
        ("#cpp", 1400, "C++ discussion and help"),
        ("#security", 760, "Defensive security, CTFs and research"),
        (
            "#homelab",
            1120,
            "Servers, racks and questionable rack purchases",
        ),
        ("#selfhosted", 830, "Run your own services"),
        ("#libera", 700, "About Libera.Chat and its services"),
        ("#irssi", 420, "The IRC client of choice for many"),
        ("#weechat", 510, "Fast, light and extensible chat client"),
        (
            "#terminal",
            380,
            "Terminal emulators, multiplexers and fonts",
        ),
        ("#commandline", 460, "Tips for the shell and command line"),
        (
            "#unixporn",
            2100,
            "Screenshots of beautiful Unix environments",
        ),
        (
            "#programming",
            980,
            "Language-agnostic programming discussion",
        ),
        ("#networking", 610, "Packets, protocols and routing"),
        ("#opensource", 540, "Free and open-source software"),
        ("#retrocomputing", 300, "Old machines, new tricks"),
        ("#plan9", 120, "Plan 9 from Bell Labs"),
    ]
    .into_iter()
    .map(|(channel, users, topic)| ListEntry::new(channel, users, topic))
    .collect()
}

fn member(nick: &str, prefixes: &str) -> Member {
    Member::new(nick, prefixes)
}

pub fn populate(app: &mut App) {
    let sid = app.next_server;
    app.next_server += 1;
    let mut server = Server::new(sid, "irc.libera.chat", 6697, true, "vimmer");
    server.connected = true;
    server.registered = true;
    server.demo = true;
    server.network = Some("Libera.Chat".to_string());
    server.prefix_modes = vec![('q', '~'), ('a', '&'), ('o', '@'), ('h', '%'), ('v', '+')];
    server.chantypes = vec!['#'];
    server.user_modes = String::new();
    app.servers.push(server);

    let console = app.create_buffer(
        sid,
        BufferKind::Server {
            title: "Libera.Chat".to_string(),
        },
    );
    for (kind, text, mins) in [
        (
            MsgKind::Server,
            "Welcome to the Libera.Chat IRC network vimmer!",
            42,
        ),
        (
            MsgKind::Motd,
            "This is a demo session — no packets are sent.",
            42,
        ),
        (MsgKind::Motd, "Type :connect to join a real network.", 42),
        (
            MsgKind::Info,
            "Capabilities negotiated: message-tags server-time multi-prefix away-notify",
            41,
        ),
        (MsgKind::Info, "You are now known as vimmer", 41),
    ] {
        app.push_msg(console, Message::system(kind, text).at(at(mins)));
    }

    let rust_members = vec![
        {
            let mut m = member("carol", "@+");
            m.account = Some("carol".into());
            m
        },
        {
            let mut m = member("ferris", "@");
            m.account = Some("ferris".into());
            m
        },
        {
            let mut m = member("erin", "%");
            m.host = Some("erin.users.libera.chat".into());
            m
        },
        member("alice", "+"),
        member("bob", ""),
        {
            let mut m = member("dave", "");
            m.away = Some("lunch".into());
            m
        },
        {
            let mut m = member("grace", "");
            m.account = Some("grace".into());
            m
        },
        member("vimmer", "@"),
    ];

    let rust = app.create_buffer(
        sid,
        BufferKind::Channel {
            name: "#rust".to_string(),
            topic:
                "Rust: a language empowering everyone to build reliable and efficient software. \
                    Ask boldly, answer kindly — no question is too small."
                    .to_string(),
            topic_by: Some("ferris at 2026-11-20 09:14".to_string()),
            modes: "nt".to_string(),
            members: rust_members,
            joined: true,
        },
    );

    let rust_msgs: &[(MsgKind, Option<&str>, &str, i64)] = &[
        (MsgKind::Join, None, "alice joined", 36),
        (
            MsgKind::Privmsg,
            Some("ferris"),
            "morning! the new borrow checker errors are so much friendlier",
            34,
        ),
        (
            MsgKind::Privmsg,
            Some("alice"),
            "right? it even suggests the fix inline now",
            33,
        ),
        (
            MsgKind::Action,
            Some("bob"),
            "sips coffee and stares at a lifetime",
            32,
        ),
        (
            MsgKind::Privmsg,
            Some("carol"),
            "does anyone have a good pattern for streaming JSON into serde?",
            30,
        ),
        (
            MsgKind::Privmsg,
            Some("ferris"),
            "serde_json::Deserializer::from_reader + StreamDeserializer is the usual answer",
            29,
        ),
        (
            MsgKind::Privmsg,
            Some("dave"),
            "or just read_line if it is line-delimited",
            28,
        ),
        (
            MsgKind::Privmsg,
            Some("erin"),
            "dave: that only works if the producer cooperates though",
            27,
        ),
        (
            MsgKind::Notice,
            Some("Grace"),
            "build succeeded: rust-lang/rust#12345 all green",
            24,
        ),
        (
            MsgKind::Privmsg,
            Some("alice"),
            "vimmer: welcome! there is a pinned list of beginner issues in the topic",
            18,
        ),
        (
            MsgKind::Privmsg,
            Some("vimmer"),
            "thanks alice — reading it now",
            17,
        ),
        (
            MsgKind::Privmsg,
            Some("bob"),
            "if you want a tiny first PR, the docs team always needs copy edits",
            16,
        ),
        (
            MsgKind::Privmsg,
            Some("carol"),
            "seconded. documentation PRs get reviewed fast",
            15,
        ),
        (MsgKind::Mode, None, "carol sets mode +v bob on #rust", 14),
        (
            MsgKind::Privmsg,
            Some("bob"),
            "oh hey, voice. thank you!",
            13,
        ),
        (
            MsgKind::Privmsg,
            Some("ferris"),
            "reminder: the triage meeting moved to Thursday 17:00 UTC",
            8,
        ),
        (
            MsgKind::Privmsg,
            Some("grace"),
            "noted, updating my calendar",
            6,
        ),
        (
            MsgKind::Privmsg,
            Some("alice"),
            "vimmer: how are you finding the TUI so far?",
            2,
        ),
    ];
    for (kind, nick, text, mins) in rust_msgs {
        app.push_msg(rust, Message::new(*kind, *nick, *text).at(at(*mins)));
    }

    let vim = app.create_buffer(
        sid,
        BufferKind::Channel {
            name: "#vim".to_string(),
            topic: "The venerable text editor. :help is your friend.".to_string(),
            topic_by: Some("bram at 2026-10-02 11:00".to_string()),
            modes: "nt".to_string(),
            members: vec![
                member("tux", "@"),
                member("alice", ""),
                member("nano_fan", ""),
                member("vimmer", ""),
            ],
            joined: true,
        },
    );
    for (kind, nick, text, mins) in [
        (
            MsgKind::Privmsg,
            Some("tux"),
            "how do you yank a visual block into the system clipboard?",
            22,
        ),
        (
            MsgKind::Privmsg,
            Some("alice"),
            "\"+y with clipboard=unnamedplus, or :'<,'>w !pbcopy",
            21,
        ),
        (
            MsgKind::Privmsg,
            Some("nano_fan"),
            "or use a terminal emulator that speaks OSC 52",
            20,
        ),
        (
            MsgKind::Privmsg,
            Some("tux"),
            "OSC 52 is the one that works over ssh, right?",
            19,
        ),
        (
            MsgKind::Privmsg,
            Some("alice"),
            "exactly. life changing for remote boxes",
            18,
        ),
    ] {
        app.push_msg(vim, Message::new(kind, nick, text).at(at(mins)));
    }

    let linux = app.create_buffer(
        sid,
        BufferKind::Channel {
            name: "#linux".to_string(),
            topic: "General Linux chat · be excellent to each other".to_string(),
            topic_by: None,
            modes: "nt".to_string(),
            members: vec![member("root", "@"), member("tux", ""), member("vimmer", "")],
            joined: true,
        },
    );
    for (kind, nick, text, mins) in [
        (
            MsgKind::Privmsg,
            Some("root"),
            "6.13-rc4 is out, the io_uring changes are worth a look",
            40,
        ),
        (
            MsgKind::Privmsg,
            Some("tux"),
            "vimmer: are you on Wayland or X11 for that terminal?",
            38,
        ),
    ] {
        app.push_msg(linux, Message::new(kind, nick, text).at(at(mins)));
    }

    let alice = app.create_buffer(
        sid,
        BufferKind::Query {
            member: {
                let mut m = member("alice", "+");
                m.account = Some("alice".into());
                m
            },
        },
    );
    for (kind, nick, text, mins) in [
        (
            MsgKind::Privmsg,
            Some("alice"),
            "hey, saw you joined #rust — welcome aboard",
            28,
        ),
        (
            MsgKind::Privmsg,
            Some("vimmer"),
            "thanks! still finding my way around the issue tracker",
            27,
        ),
        (
            MsgKind::Privmsg,
            Some("alice"),
            "start with the E-easy label, they are genuinely easy",
            26,
        ),
    ] {
        app.push_msg(alice, Message::new(kind, nick, text).at(at(mins)));
    }

    app.active = Some(rust);
    app.sidebar_nav = 1;
    app.focus = crate::app::Focus::Messages;
    app.mode = crate::app::Mode::Normal;
    if let Some(buffer) = app.buffers.get_mut(rust) {
        buffer.unread = 0;
        buffer.mentions = 0;
        buffer.selected = buffer.messages.len().saturating_sub(1);
    }
    app.input.set_text("");
    if let Some(buffer) = app.buffers.get_mut(vim) {
        buffer.unread = 4;
    }
    if let Some(buffer) = app.buffers.get_mut(linux) {
        buffer.unread = 1;
        buffer.mentions = 1;
    }
    if let Some(buffer) = app.buffers.get_mut(alice) {
        buffer.unread = 1;
    }
    app.sync_sidebar();
}
