//! Data model: servers, buffers, messages and channel members.

use chrono::{DateTime, Local};
use tokio::sync::mpsc::UnboundedSender;

use crate::util;

pub const MAX_MESSAGES: usize = 4000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MsgKind {
    Privmsg,
    Action,
    Notice,
    Join,
    Part,
    Quit,
    Kick,
    Mode,
    Topic,
    Nick,
    Server,
    Error,
    Info,
    Motd,
}

impl MsgKind {
    pub fn label(self) -> &'static str {
        match self {
            MsgKind::Privmsg => "message",
            MsgKind::Action => "action",
            MsgKind::Notice => "notice",
            MsgKind::Join => "join",
            MsgKind::Part => "part",
            MsgKind::Quit => "quit",
            MsgKind::Kick => "kick",
            MsgKind::Mode => "mode",
            MsgKind::Topic => "topic",
            MsgKind::Nick => "nick",
            MsgKind::Server => "server",
            MsgKind::Error => "error",
            MsgKind::Info => "info",
            MsgKind::Motd => "motd",
        }
    }

    pub fn is_chat(self) -> bool {
        matches!(self, MsgKind::Privmsg | MsgKind::Action | MsgKind::Notice)
    }
}

#[derive(Clone, Debug)]
pub struct Message {
    pub kind: MsgKind,
    pub nick: Option<String>,
    pub text: String,
    pub ts: DateTime<Local>,
    pub highlight: bool,
    pub own: bool,
}

impl Message {
    pub fn new(kind: MsgKind, nick: Option<&str>, text: impl Into<String>) -> Self {
        Self {
            kind,
            nick: nick.map(util::sanitize_line),
            text: util::sanitize_line(&text.into()),
            ts: Local::now(),
            highlight: false,
            own: false,
        }
    }

    pub fn system(kind: MsgKind, text: impl Into<String>) -> Self {
        Self::new(kind, None, text)
    }

    pub fn at(mut self, ts: DateTime<Local>) -> Self {
        self.ts = ts;
        self
    }

    /// Plain text representation used by yank.
    pub fn yank(&self) -> String {
        match (&self.nick, self.kind) {
            (Some(nick), MsgKind::Privmsg) => format!("<{nick}> {}", self.text),
            (Some(nick), MsgKind::Action) => format!("* {nick} {}", self.text),
            (Some(nick), MsgKind::Notice) => format!("-{nick}- {}", self.text),
            (Some(nick), _) => format!("{nick}: {}", self.text),
            (None, _) => self.text.clone(),
        }
    }

    pub fn yank_stamped(&self) -> String {
        format!("[{}] {}", util::clock(self.ts), self.yank())
    }
}

#[derive(Clone, Debug, Default)]
pub struct Member {
    pub nick: String,
    /// Prefix symbols currently held, e.g. `@+`.
    pub prefixes: String,
    /// `Some(reason)` when away (reason may be empty).
    pub away: Option<String>,
    pub account: Option<String>,
    pub host: Option<String>,
    pub realname: Option<String>,
}

impl Member {
    pub fn new(nick: impl Into<String>, prefixes: impl Into<String>) -> Self {
        Self {
            nick: nick.into(),
            prefixes: prefixes.into(),
            ..Default::default()
        }
    }

    pub fn rank(&self, server: &Server) -> usize {
        server.rank_for(&self.prefixes)
    }
}

#[derive(Clone, Debug)]
pub enum BufferKind {
    Server {
        title: String,
    },
    Channel {
        name: String,
        topic: String,
        topic_by: Option<String>,
        modes: String,
        members: Vec<Member>,
        joined: bool,
    },
    Query {
        member: Member,
    },
}

#[derive(Clone, Debug)]
pub struct Buffer {
    pub id: usize,
    pub server: usize,
    pub kind: BufferKind,
    pub messages: Vec<Message>,
    pub unread: u32,
    pub mentions: u32,
    /// Index of the message under the normal-mode cursor.
    pub selected: usize,
    /// Lines scrolled up from the bottom (0 = pinned to newest).
    pub scroll: usize,
    pub draft: String,
}

impl Buffer {
    pub fn new(id: usize, server: usize, kind: BufferKind) -> Self {
        Self {
            id,
            server,
            kind,
            messages: Vec::new(),
            unread: 0,
            mentions: 0,
            selected: 0,
            scroll: 0,
            draft: String::new(),
        }
    }

    pub fn name(&self) -> &str {
        match &self.kind {
            BufferKind::Server { title } => title,
            BufferKind::Channel { name, .. } => name,
            BufferKind::Query { member } => &member.nick,
        }
    }

    pub fn is_server(&self) -> bool {
        matches!(self.kind, BufferKind::Server { .. })
    }

    pub fn is_channel(&self) -> bool {
        matches!(self.kind, BufferKind::Channel { .. })
    }

    pub fn topic(&self) -> Option<&str> {
        match &self.kind {
            BufferKind::Channel { topic, .. } => Some(topic),
            _ => None,
        }
    }

    pub fn members(&self) -> &[Member] {
        match &self.kind {
            BufferKind::Channel { members, .. } => members,
            _ => &[],
        }
    }

    pub fn members_mut(&mut self) -> Option<&mut Vec<Member>> {
        match &mut self.kind {
            BufferKind::Channel { members, .. } => Some(members),
            _ => None,
        }
    }

    pub fn sort_key(&self) -> (u8, String) {
        let group = if self.is_channel() { 0 } else { 1 };
        (group, util::fold(self.name()))
    }
}

#[derive(Clone, Debug, Default)]
pub struct Whois {
    pub nick: String,
    pub user: String,
    pub host: String,
    pub realname: String,
    pub server: String,
    pub account: Option<String>,
    pub channels: Vec<String>,
    pub idle: Option<i64>,
    pub signon: Option<i64>,
    pub away: Option<String>,
    pub oper: bool,
    pub secure: bool,
    pub registered: bool,
}

impl Whois {
    pub fn new(nick: impl Into<String>) -> Self {
        Self {
            nick: nick.into(),
            ..Default::default()
        }
    }

    pub fn hostmask(&self) -> String {
        if self.user.is_empty() && self.host.is_empty() {
            self.nick.clone()
        } else {
            format!("{}!{}@{}", self.nick, self.user, self.host)
        }
    }
}

#[derive(Debug)]
pub struct Server {
    pub id: usize,
    pub host: String,
    pub port: u16,
    pub tls: bool,
    pub nick: String,
    /// The welcome buffer is not attached to a real socket.
    pub local: bool,
    /// A sandboxed server used by `--demo`; outbound lines are discarded.
    pub demo: bool,
    pub connected: bool,
    pub registered: bool,
    pub connecting: bool,
    pub token: u64,
    pub network: Option<String>,
    pub prefix_modes: Vec<(char, char)>,
    pub chantypes: Vec<char>,
    pub tx: Option<UnboundedSender<String>>,
    pub sasl: Option<(String, String)>,
    pub pass: Option<String>,
    pub autojoins: Vec<String>,
    pub last_error: Option<String>,
    pub cap_offered: Vec<String>,
    pub cap_requested: bool,
    pub caps_done: bool,
    pub sasl_wait: bool,
    pub whois: Option<Whois>,
    pub user_modes: String,
}

impl Server {
    pub fn new(
        id: usize,
        host: impl Into<String>,
        port: u16,
        tls: bool,
        nick: impl Into<String>,
    ) -> Self {
        Self {
            id,
            host: host.into(),
            port,
            tls,
            nick: nick.into(),
            local: false,
            demo: false,
            connected: false,
            registered: false,
            connecting: false,
            token: 0,
            network: None,
            prefix_modes: vec![('o', '@'), ('v', '+')],
            chantypes: vec!['#'],
            tx: None,
            sasl: None,
            pass: None,
            autojoins: Vec::new(),
            last_error: None,
            cap_offered: Vec::new(),
            cap_requested: false,
            caps_done: false,
            sasl_wait: false,
            whois: None,
            user_modes: String::new(),
        }
    }

    pub fn display(&self) -> &str {
        self.network.as_deref().unwrap_or(&self.host)
    }

    pub fn status(&self) -> &'static str {
        if self.local {
            "local"
        } else if self.connected && self.registered {
            "connected"
        } else if self.connecting || self.connected {
            "connecting"
        } else {
            "disconnected"
        }
    }

    pub fn prefix_for_mode(&self, mode: char) -> Option<char> {
        self.prefix_modes
            .iter()
            .find(|(m, _)| *m == mode)
            .map(|(_, p)| *p)
    }

    pub fn mode_for_prefix(&self, prefix: char) -> Option<char> {
        self.prefix_modes
            .iter()
            .find(|(_, p)| *p == prefix)
            .map(|(m, _)| *m)
    }

    pub fn is_channel_name(&self, name: &str) -> bool {
        name.chars()
            .next()
            .map(|c| self.chantypes.contains(&c) || self.chantypes.is_empty() && c == '#')
            .unwrap_or(false)
    }

    pub fn rank_for(&self, prefixes: &str) -> usize {
        self.prefix_modes
            .iter()
            .position(|(_, symbol)| prefixes.contains(*symbol))
            .unwrap_or(usize::MAX)
    }
}
