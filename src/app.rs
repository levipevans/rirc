//! Application state and behaviour: buffers, incoming IRC events, commands,
//! menus, mouse handling and mode routing.

use std::sync::mpsc::Sender;
use std::time::{Duration, Instant};

use chrono::Local;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::Rect;
use tokio::runtime::Handle;

use crate::config::{Config, ServerConfig};
use crate::input::Input;
use crate::irc::{self, IrcMessage, NetEvent};
use crate::list::{ChannelList, ListEntry};
use crate::menu::{help_menu, Menu, MenuAction};
use crate::model::*;
use crate::modes::{insert, normal, visual};
use crate::util;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Normal,
    Visual { anchor: usize },
    Insert,
    Command,
}

impl Mode {
    pub fn label(self) -> &'static str {
        match self {
            Mode::Normal => "NORMAL",
            Mode::Visual { .. } => "VISUAL",
            Mode::Insert => "TEXT",
            Mode::Command => "COMMAND",
        }
    }

    pub fn wants_input(self) -> bool {
        matches!(self, Mode::Insert | Mode::Command)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Focus {
    Channels,
    Messages,
    Members,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SideEntry {
    Server(usize),
    Buffer(usize),
}

#[derive(Clone, Debug, Default)]
pub struct Hit {
    pub sidebar: Vec<(u16, usize)>,
    pub members: Vec<(u16, usize)>,
    pub messages: Vec<(u16, usize)>,
    pub sidebar_rect: Rect,
    pub members_rect: Rect,
    pub messages_rect: Rect,
}

pub struct App {
    pub cfg: Config,
    pub servers: Vec<Server>,
    pub buffers: Vec<Buffer>,
    pub active: Option<usize>,
    pub mode: Mode,
    pub focus: Focus,
    pub last_focus: Focus,
    pub input: Input,
    pub menu: Option<Menu>,
    pub list: Option<ChannelList>,
    pub sidebar_nav: usize,
    pub member_nav: usize,
    pub toast: Option<(String, Instant)>,
    pub quit: bool,
    pub rt: Option<Handle>,
    pub net_tx: Sender<NetEvent>,
    pub next_server: usize,
    pub next_buffer: usize,
    pub width: u16,
    pub height: u16,
    pub hit: Hit,
    pub default_nick: String,
}

const COMMANDS: &[&str] = &[
    "join",
    "part",
    "me",
    "msg",
    "query",
    "nick",
    "topic",
    "mode",
    "whois",
    "away",
    "invite",
    "kick",
    "op",
    "deop",
    "voice",
    "devoice",
    "list",
    "clear",
    "close",
    "connect",
    "disconnect",
    "reconnect",
    "raw",
    "quote",
    "help",
    "quit",
    "q",
    "exit",
];

impl App {
    pub fn new(cfg: Config, rt: Option<Handle>, net_tx: Sender<NetEvent>) -> Self {
        let nick = cfg.nick.clone();
        let demo = cfg.demo;
        let mut app = App {
            cfg,
            servers: Vec::new(),
            buffers: Vec::new(),
            active: None,
            mode: Mode::Normal,
            focus: Focus::Messages,
            last_focus: Focus::Messages,
            input: Input::new(),
            menu: None,
            list: None,
            sidebar_nav: 0,
            member_nav: 0,
            toast: None,
            quit: false,
            rt,
            net_tx,
            next_server: 0,
            next_buffer: 0,
            width: 120,
            height: 40,
            hit: Hit::default(),
            default_nick: nick,
        };
        if demo {
            crate::demo::populate(&mut app);
        } else {
            if app.cfg.start.is_none() {
                app.add_welcome();
            }
            if let Some(server) = app.cfg.start.clone() {
                app.start_server(server);
            }
        }
        app
    }

    // ---------------------------------------------------------------- boot ---

    fn add_welcome(&mut self) {
        let sid = self.next_server;
        self.next_server += 1;
        let mut server = Server::new(sid, "local", 0, false, &self.default_nick);
        server.local = true;
        server.connected = true;
        server.registered = true;
        server.network = Some("welcome".to_string());
        self.servers.push(server);

        let bid = self.create_buffer(
            sid,
            BufferKind::Server {
                title: "welcome".to_string(),
            },
        );
        let lines = [
            "rirc — a tiny, vim-flavoured IRCv3 client.",
            "",
            "Not connected yet. Try  :connect irc.libera.chat",
            "then  /join #rust  and press  i  to write.",
            "",
            "Tab moves between the buffer list, messages and members.",
            "Enter opens the menu for the thing under the cursor.",
            "v starts a visual selection; y yanks it to the clipboard.",
            "? shows all keys.",
        ];
        for line in lines {
            self.push_msg(bid, Message::system(MsgKind::Info, line));
        }
        self.active = Some(bid);
        self.sidebar_nav = 0;
        self.focus = Focus::Messages;
    }

    pub fn start_server(&mut self, cfg: ServerConfig) {
        let sid = self.next_server;
        self.next_server += 1;
        let mut server = Server::new(sid, cfg.host.clone(), cfg.port, cfg.tls, cfg.nick.clone());
        server.sasl = cfg.sasl.clone();
        server.pass = cfg.pass.clone();
        server.autojoins = cfg.channels.clone();
        self.servers.push(server);

        let bid = self.create_buffer(
            sid,
            BufferKind::Server {
                title: cfg.host.clone(),
            },
        );
        let tls = if cfg.tls { "TLS" } else { "plaintext" };
        self.push_msg(
            bid,
            Message::system(
                MsgKind::Info,
                format!("Connecting to {}:{} ({tls})…", cfg.host, cfg.port),
            ),
        );
        self.set_active(bid);
        self.spawn_server(sid);
    }

    fn spawn_server(&mut self, sid: usize) {
        let Some(pos) = self.server_pos(sid) else {
            return;
        };
        if self.servers[pos].local {
            return;
        }
        if self.servers[pos].demo {
            self.toast("demo session — use :connect to reach a real network");
            return;
        }
        self.servers[pos].token += 1;
        self.servers[pos].connecting = true;
        self.servers[pos].connected = false;
        self.servers[pos].registered = false;
        self.servers[pos].caps_done = false;
        self.servers[pos].cap_requested = false;
        self.servers[pos].cap_offered.clear();
        self.servers[pos].sasl_wait = false;
        let token = self.servers[pos].token;
        let (host, port, tls) = (
            self.servers[pos].host.clone(),
            self.servers[pos].port,
            self.servers[pos].tls,
        );
        if let Some(rt) = &self.rt {
            let tx = irc::connect(rt, sid, token, &host, port, tls, self.net_tx.clone());
            self.servers[pos].tx = Some(tx);
        } else {
            self.toast("no async runtime available");
        }
    }

    pub fn create_buffer(&mut self, server: usize, kind: BufferKind) -> usize {
        let id = self.next_buffer;
        self.next_buffer += 1;
        self.buffers.push(Buffer::new(id, server, kind));
        self.buffers.len() - 1
    }

    // ------------------------------------------------------------- lookups ---

    pub fn server_pos(&self, sid: usize) -> Option<usize> {
        self.servers.iter().position(|s| s.id == sid)
    }

    pub fn server(&self, sid: usize) -> Option<&Server> {
        self.servers.iter().find(|s| s.id == sid)
    }

    fn server_mut(&mut self, sid: usize) -> Option<&mut Server> {
        self.servers.iter_mut().find(|s| s.id == sid)
    }

    pub fn nick_of(&self, sid: usize) -> String {
        self.server(sid)
            .map(|s| s.nick.clone())
            .unwrap_or_else(|| self.default_nick.clone())
    }

    pub fn active_buffer(&self) -> Option<&Buffer> {
        self.active.and_then(|i| self.buffers.get(i))
    }

    pub fn active_buffer_mut(&mut self) -> Option<&mut Buffer> {
        match self.active {
            Some(i) => self.buffers.get_mut(i),
            None => None,
        }
    }

    pub fn server_buffer(&self, sid: usize) -> Option<usize> {
        self.buffers
            .iter()
            .position(|b| b.server == sid && b.is_server())
    }

    pub fn ensure_server_buffer(&mut self, sid: usize) -> usize {
        if let Some(idx) = self.server_buffer(sid) {
            return idx;
        }
        let title = self
            .server(sid)
            .map(|s| s.display().to_string())
            .unwrap_or_else(|| "server".to_string());
        self.create_buffer(sid, BufferKind::Server { title })
    }

    pub fn find_channel(&self, sid: usize, name: &str) -> Option<usize> {
        let name = util::fold(name);
        self.buffers.iter().position(|b| {
            b.server == sid
                && matches!(&b.kind, BufferKind::Channel { name: n, .. } if util::fold(n) == name)
        })
    }

    pub fn find_query(&self, sid: usize, nick: &str) -> Option<usize> {
        let nick = util::fold(nick);
        self.buffers.iter().position(|b| {
            b.server == sid
                && matches!(&b.kind, BufferKind::Query { member } if util::fold(&member.nick) == nick)
        })
    }

    pub fn ensure_channel(&mut self, sid: usize, name: &str) -> usize {
        let name = util::sanitize_line(name);
        if let Some(idx) = self.find_channel(sid, &name) {
            return idx;
        }
        self.create_buffer(
            sid,
            BufferKind::Channel {
                name,
                topic: String::new(),
                topic_by: None,
                modes: String::new(),
                members: Vec::new(),
                joined: false,
            },
        )
    }

    pub fn ensure_query(&mut self, sid: usize, nick: &str) -> usize {
        let nick = util::sanitize_line(nick);
        if let Some(idx) = self.find_query(sid, &nick) {
            return idx;
        }
        let mut member = Member::new(nick.clone(), "");
        for buffer in &self.buffers {
            if buffer.server != sid {
                continue;
            }
            if let Some(found) = buffer
                .members()
                .iter()
                .find(|m| util::fold(&m.nick) == util::fold(&nick))
            {
                member = found.clone();
                break;
            }
        }
        self.create_buffer(sid, BufferKind::Query { member })
    }

    /// Buffers in sidebar order: each server's console, channels, then queries.
    pub fn ordered_buffers(&self) -> Vec<usize> {
        let mut out = Vec::new();
        for server in &self.servers {
            let mut list: Vec<usize> = self
                .buffers
                .iter()
                .enumerate()
                .filter(|(_, b)| b.server == server.id)
                .map(|(i, _)| i)
                .collect();
            list.sort_by_key(|&i| {
                let b = &self.buffers[i];
                if b.is_server() {
                    (0u8, String::new())
                } else {
                    let (g, n) = b.sort_key();
                    (g + 1, n)
                }
            });
            out.extend(list);
        }
        out
    }

    pub fn sidebar_entries(&self) -> Vec<SideEntry> {
        let mut out = Vec::new();
        for server in &self.servers {
            if let Some(pos) = self.server_pos(server.id) {
                out.push(SideEntry::Server(pos));
            }
            let mut list: Vec<usize> = self
                .buffers
                .iter()
                .enumerate()
                .filter(|(_, b)| b.server == server.id && !b.is_server())
                .map(|(i, _)| i)
                .collect();
            list.sort_by_key(|&i| self.buffers[i].sort_key());
            for idx in list {
                out.push(SideEntry::Buffer(idx));
            }
        }
        out
    }

    /// Member indices of `bid` in display order (privilege, then nick).
    pub fn member_order(&self, bid: usize) -> Vec<usize> {
        let Some(buffer) = self.buffers.get(bid) else {
            return Vec::new();
        };
        let server = self.server(buffer.server);
        let mut idxs: Vec<usize> = (0..buffer.members().len()).collect();
        idxs.sort_by(|&a, &b| {
            let ma = &buffer.members()[a];
            let mb = &buffer.members()[b];
            let ra = server.map(|s| ma.rank(s)).unwrap_or(usize::MAX);
            let rb = server.map(|s| mb.rank(s)).unwrap_or(usize::MAX);
            ra.cmp(&rb)
                .then_with(|| util::fold(&ma.nick).cmp(&util::fold(&mb.nick)))
        });
        idxs
    }

    // ------------------------------------------------------------- mutation ---

    pub fn set_active(&mut self, idx: usize) {
        if idx >= self.buffers.len() {
            return;
        }
        if self.active == Some(idx) {
            if let Some(buffer) = self.buffers.get_mut(idx) {
                buffer.unread = 0;
                buffer.mentions = 0;
                buffer.scroll = 0;
                buffer.selected = buffer.messages.len().saturating_sub(1);
            }
            self.mode = Mode::Normal;
            self.menu = None;
            self.focus = Focus::Messages;
            self.member_nav = 0;
            self.sync_sidebar();
            return;
        }
        if let Some(old) = self.active {
            if old != idx && old < self.buffers.len() && self.mode != Mode::Command {
                self.buffers[old].draft = self.input.text();
            }
        }
        self.active = Some(idx);
        if let Some(buffer) = self.buffers.get_mut(idx) {
            buffer.unread = 0;
            buffer.mentions = 0;
            buffer.scroll = 0;
            buffer.selected = buffer.messages.len().saturating_sub(1);
        }
        let draft = self
            .buffers
            .get(idx)
            .map(|b| b.draft.clone())
            .unwrap_or_default();
        self.input.set_text(&draft);
        self.mode = Mode::Normal;
        self.menu = None;
        self.focus = Focus::Messages;
        self.member_nav = 0;
        self.sync_sidebar();
    }

    pub fn sync_sidebar(&mut self) {
        let Some(active) = self.active else { return };
        let is_server = self
            .buffers
            .get(active)
            .map(|b| b.is_server())
            .unwrap_or(false);
        let sid = self.buffers.get(active).map(|b| b.server);
        let entries = self.sidebar_entries();
        let found = entries.iter().position(|entry| match entry {
            SideEntry::Buffer(i) => *i == active,
            SideEntry::Server(pos) => {
                is_server
                    && self
                        .servers
                        .get(*pos)
                        .map(|s| Some(s.id) == sid)
                        .unwrap_or(false)
            }
        });
        if let Some(pos) = found {
            self.sidebar_nav = pos;
        } else {
            self.sidebar_nav = self.sidebar_nav.min(entries.len().saturating_sub(1));
        }
    }

    pub fn push_msg(&mut self, bid: usize, mut msg: Message) {
        let active = self.active == Some(bid);
        let sid = match self.buffers.get(bid) {
            Some(b) => b.server,
            None => return,
        };
        let my_nick = self.nick_of(sid);
        if !msg.own && msg.kind.is_chat() && util::mentions(&msg.text, &my_nick) {
            msg.highlight = true;
        }
        let highlight = msg.highlight;
        let counts = msg.kind.is_chat();
        let Some(buffer) = self.buffers.get_mut(bid) else {
            return;
        };
        let at_bottom = buffer.selected + 1 >= buffer.messages.len();
        buffer.messages.push(msg);
        if buffer.messages.len() > MAX_MESSAGES {
            let drop = buffer.messages.len() - MAX_MESSAGES;
            buffer.messages.drain(0..drop);
            buffer.selected = buffer.selected.saturating_sub(drop);
            if at_bottom {
                buffer.selected = buffer.messages.len().saturating_sub(1);
            }
        }
        if active {
            buffer.unread = 0;
            buffer.mentions = 0;
            if at_bottom {
                buffer.selected = buffer.messages.len().saturating_sub(1);
            }
        } else if counts {
            buffer.unread = buffer.unread.saturating_add(1);
            if highlight {
                buffer.mentions = buffer.mentions.saturating_add(1);
            }
        }
    }

    pub fn push_system(&mut self, bid: usize, kind: MsgKind, text: impl Into<String>) {
        self.push_msg(bid, Message::system(kind, text));
    }

    pub fn server_msg(&mut self, sid: usize, kind: MsgKind, text: impl Into<String>) {
        let bid = self.ensure_server_buffer(sid);
        self.push_system(bid, kind, text);
    }

    pub fn toast(&mut self, text: impl Into<String>) {
        self.toast = Some((text.into(), Instant::now() + Duration::from_secs(5)));
    }

    pub fn tick(&mut self) {
        if let Some((_, until)) = &self.toast {
            if Instant::now() >= *until {
                self.toast = None;
            }
        }
    }

    // ------------------------------------------------------------- network ---

    pub fn send(&mut self, sid: usize, line: impl Into<String>) {
        if !self.send_silent(sid, line) {
            self.toast("not connected");
        }
    }

    fn send_silent(&mut self, sid: usize, line: impl Into<String>) -> bool {
        let line = line.into();
        if let Some(server) = self.server_mut(sid) {
            if server.demo {
                return true;
            }
            if let Some(tx) = &server.tx {
                return tx.send(line).is_ok();
            }
        }
        false
    }

    fn token_ok(&self, sid: usize, token: u64) -> bool {
        self.server(sid).map(|s| s.token == token).unwrap_or(false)
    }

    pub fn on_net(&mut self, event: NetEvent) {
        match event {
            NetEvent::Connected { server, token } => {
                if !self.token_ok(server, token) {
                    return;
                }
                if let Some(s) = self.server_mut(server) {
                    s.connected = true;
                    s.connecting = false;
                }
                self.server_msg(server, MsgKind::Info, "Transport connected, registering…");
                self.begin_registration(server);
            }
            NetEvent::Error {
                server,
                token,
                message,
            } => {
                if !self.token_ok(server, token) {
                    return;
                }
                if let Some(s) = self.server_mut(server) {
                    s.connected = false;
                    s.connecting = false;
                    s.last_error = Some(message.clone());
                    s.tx = None;
                }
                if let Some(list) = self.list.as_mut() {
                    if list.server == server && list.loading {
                        list.fail(&message);
                    }
                }
                self.server_msg(
                    server,
                    MsgKind::Error,
                    format!("Connection failed: {message}"),
                );
                self.toast(message);
            }
            NetEvent::Closed {
                server,
                token,
                reason,
            } => {
                if !self.token_ok(server, token) {
                    return;
                }
                let was = self.server(server).map(|s| s.connected).unwrap_or(false);
                if let Some(s) = self.server_mut(server) {
                    s.connected = false;
                    s.connecting = false;
                    s.registered = false;
                    s.tx = None;
                }
                for buffer in &mut self.buffers {
                    if buffer.server == server {
                        if let BufferKind::Channel {
                            joined, members, ..
                        } = &mut buffer.kind
                        {
                            *joined = false;
                            members.clear();
                        }
                    }
                }
                if let Some(list) = self.list.as_mut() {
                    if list.server == server && list.loading {
                        list.fail(&format!("disconnected: {reason}"));
                    }
                }
                if reason != "client closed" {
                    let text = if was {
                        format!("Disconnected: {reason}")
                    } else {
                        format!("Connection lost: {reason}")
                    };
                    self.server_msg(server, MsgKind::Error, text);
                }
            }
            NetEvent::Line {
                server,
                token,
                line,
            } => {
                if !self.token_ok(server, token) {
                    return;
                }
                self.handle_line(server, &line);
            }
        }
    }

    fn begin_registration(&mut self, sid: usize) {
        let (nick, pass) = match self.server(sid) {
            Some(s) => (s.nick.clone(), s.pass.clone()),
            None => return,
        };
        if let Some(pass) = pass {
            self.send(sid, format!("PASS {pass}"));
        }
        self.send(sid, "CAP LS 302");
        self.send(sid, format!("NICK {nick}"));
        self.send(sid, format!("USER {nick} 0 * :{nick}"));
        if let Some(s) = self.server_mut(sid) {
            s.cap_offered.clear();
            s.cap_requested = false;
            s.caps_done = false;
            s.sasl_wait = false;
        }
    }

    fn request_caps(&mut self, sid: usize) {
        let Some(server) = self.server(sid) else {
            return;
        };
        let offered = server.cap_offered.clone();
        let wants_sasl = server.sasl.is_some();
        let has = |name: &str| {
            offered
                .iter()
                .any(|cap| cap.split('=').next() == Some(name))
        };
        let mut want: Vec<&str> = Vec::new();
        for cap in [
            "message-tags",
            "server-time",
            "multi-prefix",
            "away-notify",
            "extended-join",
            "account-tag",
        ] {
            if has(cap) {
                want.push(cap);
            }
        }
        if wants_sasl && has("sasl") {
            want.push("sasl");
        }
        if let Some(s) = self.server_mut(sid) {
            s.cap_requested = true;
        }
        if want.is_empty() {
            self.cap_end(sid);
        } else {
            self.send(sid, format!("CAP REQ :{}", want.join(" ")));
        }
    }

    fn cap_end(&mut self, sid: usize) {
        let done = self.server(sid).map(|s| s.caps_done).unwrap_or(true);
        if done {
            return;
        }
        if let Some(s) = self.server_mut(sid) {
            s.caps_done = true;
            s.sasl_wait = false;
        }
        self.send(sid, "CAP END");
    }

    fn handle_cap(&mut self, sid: usize, msg: &IrcMessage) {
        let sub = msg.param(1).unwrap_or("").to_ascii_uppercase();
        match sub.as_str() {
            "LS" => {
                let more = msg.param(2).map(|p| p == "*").unwrap_or(false);
                let caps = msg.trailing().to_string();
                if let Some(server) = self.server_mut(sid) {
                    for cap in caps.split_whitespace() {
                        if !server.cap_offered.iter().any(|c| c == cap) {
                            server.cap_offered.push(cap.to_string());
                        }
                    }
                }
                if !more {
                    self.request_caps(sid);
                }
            }
            "ACK" => {
                let acked = msg.trailing().to_string();
                let sasl = self.server(sid).map(|s| s.sasl.is_some()).unwrap_or(false);
                let sasl_acked = acked
                    .split_whitespace()
                    .any(|cap| cap == "sasl" || cap.starts_with("sasl="));
                if sasl && sasl_acked {
                    if let Some(s) = self.server_mut(sid) {
                        s.sasl_wait = true;
                    }
                    self.send(sid, "AUTHENTICATE PLAIN");
                } else {
                    self.cap_end(sid);
                }
            }
            "NAK" => {
                let denied = msg.trailing().to_string();
                if !denied.is_empty() {
                    self.server_msg(
                        sid,
                        MsgKind::Info,
                        format!("Server refused capabilities: {denied}"),
                    );
                }
                self.cap_end(sid);
            }
            _ => {}
        }
    }

    // -------------------------------------------------------------- wire ---

    fn handle_line(&mut self, sid: usize, line: &str) {
        let Some(msg) = irc::parse(line) else { return };
        match msg.command.as_str() {
            "PING" => {
                let token = msg.trailing().to_string();
                self.send(sid, format!("PONG :{token}"));
            }
            "PONG" => {}
            "CAP" => self.handle_cap(sid, &msg),
            "AUTHENTICATE" => {
                if msg.param(0) == Some("+") {
                    if let Some((user, pass)) = self.server(sid).and_then(|s| s.sasl.clone()) {
                        let payload = format!("{user}\0{user}\0{pass}");
                        self.send(
                            sid,
                            format!("AUTHENTICATE {}", util::base64(payload.as_bytes())),
                        );
                    }
                }
            }
            "001" => self.on_welcome(sid, &msg),
            "PRIVMSG" | "NOTICE" => self.on_chat(sid, &msg),
            "JOIN" => self.on_join(sid, &msg),
            "PART" => self.on_part(sid, &msg),
            "QUIT" => self.on_quit(sid, &msg),
            "KICK" => self.on_kick(sid, &msg),
            "NICK" => self.on_nick(sid, &msg),
            "MODE" => self.on_mode(sid, &msg),
            "TOPIC" => self.on_topic(sid, &msg),
            "AWAY" => self.on_away(sid, &msg),
            "ACCOUNT" => self.on_account(sid, &msg),
            "CHGHOST" => self.on_chghost(sid, &msg),
            "SETNAME" => {
                if let Some(nick) = msg.nick() {
                    let realname = msg.trailing().to_string();
                    self.update_member_meta(sid, nick, |m| m.realname = Some(realname.clone()));
                }
            }
            "INVITE" => {
                let channel = msg
                    .param(1)
                    .or_else(|| msg.param(0))
                    .unwrap_or("")
                    .to_string();
                let from = msg.nick().unwrap_or("someone").to_string();
                self.server_msg(
                    sid,
                    MsgKind::Info,
                    format!("{from} invited you to {channel}"),
                );
            }
            "ERROR" => {
                self.server_msg(sid, MsgKind::Error, msg.trailing().to_string());
            }
            "WALLOPS" => {
                self.server_msg(sid, MsgKind::Notice, format!("WALLOPS: {}", msg.trailing()));
            }
            "FAIL" | "WARN" | "NOTE" => {
                let kind = if msg.command == "FAIL" {
                    MsgKind::Error
                } else {
                    MsgKind::Info
                };
                self.server_msg(
                    sid,
                    kind,
                    format!(
                        "{} {}: {}",
                        msg.command,
                        msg.param(1).unwrap_or(""),
                        msg.trailing()
                    ),
                );
            }
            "BATCH" | "TAGMSG" => {}
            command if is_numeric(command) => self.on_numeric(sid, command, &msg),
            _ => {}
        }
    }

    fn on_welcome(&mut self, sid: usize, msg: &IrcMessage) {
        let nick = msg.param(0).unwrap_or("").to_string();
        let autojoins = {
            let Some(server) = self.server_mut(sid) else {
                return;
            };
            server.nick = nick.clone();
            server.connected = true;
            server.registered = true;
            server.connecting = false;
            server.autojoins.clone()
        };
        let label = self
            .server(sid)
            .map(|s| s.display().to_string())
            .unwrap_or_else(|| "server".to_string());
        self.server_msg(
            sid,
            MsgKind::Info,
            format!("Connected to {label} as {nick}"),
        );
        if let Some(bid) = self.server_buffer(sid) {
            if let BufferKind::Server { title } = &mut self.buffers[bid].kind {
                *title = label;
            }
        }
        for channel in autojoins {
            self.send(sid, format!("JOIN {channel}"));
        }
    }

    fn on_chat(&mut self, sid: usize, msg: &IrcMessage) {
        let target = msg.param(0).unwrap_or("").to_string();
        let mut text = msg.param(1).unwrap_or("").to_string();
        let nick = msg.nick().unwrap_or("").to_string();
        let my_nick = self.nick_of(sid);
        let own = util::fold(&nick) == util::fold(&my_nick);
        let ts = msg
            .tag("time")
            .and_then(util::parse_server_time)
            .unwrap_or_else(Local::now);

        let mut kind = if msg.command == "NOTICE" {
            MsgKind::Notice
        } else {
            MsgKind::Privmsg
        };

        if text.starts_with('\u{1}') {
            let inner = text.trim_matches('\u{1}');
            if let Some(action) = inner.strip_prefix("ACTION ") {
                kind = MsgKind::Action;
                text = action.to_string();
            } else {
                let name = inner.split(' ').next().unwrap_or("CTCP");
                if !own {
                    self.server_msg(sid, MsgKind::Info, format!("CTCP {name} from {nick}"));
                    if name.eq_ignore_ascii_case("VERSION") && !nick.is_empty() {
                        self.send(
                            sid,
                            format!(
                                "NOTICE {nick} :\u{1}VERSION rirc {}\u{1}",
                                env!("CARGO_PKG_VERSION")
                            ),
                        );
                    }
                }
                return;
            }
        }

        let is_channel = self
            .server(sid)
            .map(|s| s.is_channel_name(&target))
            .unwrap_or_else(|| target.starts_with('#'));
        let from_user = msg
            .prefix
            .as_deref()
            .map(|p| p.contains('!'))
            .unwrap_or(false);
        let bid = if is_channel {
            self.ensure_channel(sid, &target)
        } else if from_user && util::fold(&target) == util::fold(&my_nick) {
            self.ensure_query(sid, &nick)
        } else if from_user && target != "*" {
            self.ensure_query(sid, &target)
        } else {
            self.ensure_server_buffer(sid)
        };

        if is_channel && !nick.is_empty() {
            self.merge_member(bid, &nick, "", false, None, None);
        }
        if let Some(account) = msg.tag("account") {
            if !nick.is_empty() {
                let account = account.to_string();
                self.update_member_meta(sid, &nick, |m| m.account = Some(account.clone()));
            }
        }

        let mut message = Message::new(kind, Some(&nick), text).at(ts);
        message.own = own;
        self.push_msg(bid, message);
    }

    fn on_join(&mut self, sid: usize, msg: &IrcMessage) {
        let channel = msg.param(0).unwrap_or("").to_string();
        let nick = msg.nick().unwrap_or("").to_string();
        if nick.is_empty() || channel.is_empty() {
            return;
        }
        let my_nick = self.nick_of(sid);
        let own = util::fold(&nick) == util::fold(&my_nick);
        let existed = self.find_channel(sid, &channel).is_some();
        let bid = self.ensure_channel(sid, &channel);
        let account = msg
            .param(1)
            .filter(|a| *a != "*")
            .map(str::to_string)
            .or_else(|| msg.tag("account").map(str::to_string));
        let realname = msg.param(2).map(str::to_string);
        self.merge_member(bid, &nick, "", false, account, realname);

        if own {
            if let BufferKind::Channel { joined, .. } = &mut self.buffers[bid].kind {
                *joined = true;
            }
            self.push_system(bid, MsgKind::Join, format!("→ You joined {channel}"));
            if !existed {
                self.set_active(bid);
            }
        } else {
            self.push_system(bid, MsgKind::Join, format!("→ {nick} joined"));
        }
    }

    fn on_part(&mut self, sid: usize, msg: &IrcMessage) {
        let channel = msg.param(0).unwrap_or("").to_string();
        let nick = msg.nick().unwrap_or("").to_string();
        let reason = msg.param(1).unwrap_or("").to_string();
        let my_nick = self.nick_of(sid);
        let own = util::fold(&nick) == util::fold(&my_nick);
        let Some(bid) = self.find_channel(sid, &channel) else {
            return;
        };
        self.remove_member(bid, &nick);
        let text = if reason.is_empty() {
            format!("← {nick} left {channel}")
        } else {
            format!("← {nick} left {channel} ({reason})")
        };
        self.push_system(bid, MsgKind::Part, text);
        if own {
            if let BufferKind::Channel { joined, .. } = &mut self.buffers[bid].kind {
                *joined = false;
            }
        }
    }

    fn on_quit(&mut self, sid: usize, msg: &IrcMessage) {
        let nick = msg.nick().unwrap_or("").to_string();
        let reason = msg.param(0).unwrap_or("").to_string();
        if nick.is_empty() {
            return;
        }
        let my_nick = self.nick_of(sid);
        let own = util::fold(&nick) == util::fold(&my_nick);
        let text = if reason.is_empty() {
            format!("✕ {nick} quit")
        } else {
            format!("✕ {nick} quit ({reason})")
        };
        let targets: Vec<usize> = self
            .buffers
            .iter()
            .enumerate()
            .filter(|(_, b)| {
                b.server == sid
                    && b.is_channel()
                    && b.members()
                        .iter()
                        .any(|m| util::fold(&m.nick) == util::fold(&nick))
            })
            .map(|(i, _)| i)
            .collect();
        for bid in targets {
            self.remove_member(bid, &nick);
            self.push_system(bid, MsgKind::Quit, text.clone());
        }
        if own {
            if let Some(s) = self.server_mut(sid) {
                s.connected = false;
                s.registered = false;
            }
            self.server_msg(sid, MsgKind::Info, "You quit the server");
        }
    }

    fn on_kick(&mut self, sid: usize, msg: &IrcMessage) {
        let channel = msg.param(0).unwrap_or("").to_string();
        let victim = msg.param(1).unwrap_or("").to_string();
        let reason = msg.param(2).unwrap_or("").to_string();
        let by = msg.nick().unwrap_or("server").to_string();
        let Some(bid) = self.find_channel(sid, &channel) else {
            return;
        };
        self.remove_member(bid, &victim);
        let text = if reason.is_empty() {
            format!("! {by} kicked {victim} from {channel}")
        } else {
            format!("! {by} kicked {victim} from {channel} ({reason})")
        };
        self.push_system(bid, MsgKind::Kick, text);
        if util::fold(&victim) == util::fold(&self.nick_of(sid)) {
            if let BufferKind::Channel { joined, .. } = &mut self.buffers[bid].kind {
                *joined = false;
            }
        }
    }

    fn on_nick(&mut self, sid: usize, msg: &IrcMessage) {
        let old = msg.nick().unwrap_or("").to_string();
        let new = msg.param(0).unwrap_or("").to_string();
        if old.is_empty() || new.is_empty() {
            return;
        }
        let my_nick = self.nick_of(sid);
        let own = util::fold(&old) == util::fold(&my_nick);
        for buffer in &mut self.buffers {
            if buffer.server != sid {
                continue;
            }
            match &mut buffer.kind {
                BufferKind::Channel { members, .. } => {
                    if let Some(member) = members
                        .iter_mut()
                        .find(|m| util::fold(&m.nick) == util::fold(&old))
                    {
                        member.nick = new.clone();
                    }
                }
                BufferKind::Query { member } => {
                    if util::fold(&member.nick) == util::fold(&old) {
                        member.nick = new.clone();
                    }
                }
                _ => {}
            }
        }
        if own {
            if let Some(server) = self.server_mut(sid) {
                server.nick = new.clone();
            }
            self.server_msg(sid, MsgKind::Nick, format!("You are now known as {new}"));
        } else {
            let targets: Vec<usize> = self
                .buffers
                .iter()
                .enumerate()
                .filter(|(_, b)| {
                    b.server == sid
                        && b.is_channel()
                        && b.members()
                            .iter()
                            .any(|m| util::fold(&m.nick) == util::fold(&new))
                })
                .map(|(i, _)| i)
                .collect();
            for bid in targets {
                self.push_system(bid, MsgKind::Nick, format!("{old} is now known as {new}"));
            }
        }
    }

    fn on_topic(&mut self, sid: usize, msg: &IrcMessage) {
        let channel = msg.param(0).unwrap_or("").to_string();
        let topic = util::sanitize_line(msg.param(1).unwrap_or(""));
        let by = msg.nick().unwrap_or("server").to_string();
        let Some(bid) = self.find_channel(sid, &channel) else {
            return;
        };
        if let BufferKind::Channel {
            topic: current,
            topic_by,
            ..
        } = &mut self.buffers[bid].kind
        {
            *current = topic.clone();
            *topic_by = Some(by.clone());
        }
        let text = if topic.is_empty() {
            format!("{by} cleared the topic")
        } else {
            format!("{by} set the topic: {topic}")
        };
        self.push_system(bid, MsgKind::Topic, text);
    }

    fn on_mode(&mut self, sid: usize, msg: &IrcMessage) {
        let target = msg.param(0).unwrap_or("").to_string();
        let modes = msg.param(1).unwrap_or("").to_string();
        let setter = msg.nick().unwrap_or("server").to_string();
        if target.is_empty() || modes.is_empty() {
            return;
        }
        let is_channel = self
            .server(sid)
            .map(|s| s.is_channel_name(&target))
            .unwrap_or(false);
        let prefix_modes = self
            .server(sid)
            .map(|s| s.prefix_modes.clone())
            .unwrap_or_default();
        let mut adding = true;
        let mut arg = 2usize;
        let mut swaps: Vec<(char, bool, String)> = Vec::new();
        let mut mode_letters: Vec<(bool, char)> = Vec::new();

        for ch in modes.chars() {
            match ch {
                '+' => adding = true,
                '-' => adding = false,
                _ => {
                    let prefix_mode = prefix_modes.iter().any(|(m, _)| *m == ch);
                    let takes_arg = prefix_mode || matches!(ch, 'k' | 'l' | 'b' | 'e' | 'I');
                    let value = if takes_arg {
                        let v = msg.param(arg).map(str::to_string);
                        if v.is_some() {
                            arg += 1;
                        }
                        v
                    } else {
                        None
                    };
                    if prefix_mode {
                        if let (Some(value), Some(symbol)) = (
                            value,
                            prefix_modes.iter().find(|(m, _)| *m == ch).map(|(_, p)| *p),
                        ) {
                            swaps.push((symbol, adding, value));
                        }
                    } else if !matches!(ch, 'b' | 'e' | 'I') && ch.is_ascii_alphabetic() {
                        mode_letters.push((adding, ch));
                    }
                }
            }
        }

        if is_channel {
            let bid = self.ensure_channel(sid, &target);
            for (symbol, adding, nick) in &swaps {
                self.set_member_prefix(bid, nick, *symbol, *adding);
            }
            if let BufferKind::Channel { modes, .. } = &mut self.buffers[bid].kind {
                let mut set: Vec<char> =
                    modes.chars().filter(|c| c.is_ascii_alphabetic()).collect();
                for (adding, ch) in &mode_letters {
                    if *adding {
                        if !set.contains(ch) {
                            set.push(*ch);
                        }
                    } else {
                        set.retain(|c| c != ch);
                    }
                }
                set.sort_unstable();
                *modes = set.into_iter().collect();
            }
            let mut rendered = modes.clone();
            let extra: Vec<&str> = msg.params.iter().skip(2).map(String::as_str).collect();
            if !extra.is_empty() {
                rendered.push(' ');
                rendered.push_str(&extra.join(" "));
            }
            self.push_system(
                bid,
                MsgKind::Mode,
                format!("{setter} sets mode {rendered} on {target}"),
            );
        } else {
            if util::fold(&target) == util::fold(&self.nick_of(sid)) {
                if let Some(server) = self.server_mut(sid) {
                    let mut set: Vec<char> = server
                        .user_modes
                        .chars()
                        .filter(|c| c.is_ascii_alphabetic())
                        .collect();
                    for (adding, ch) in &mode_letters {
                        if *adding {
                            if !set.contains(ch) {
                                set.push(*ch);
                            }
                        } else {
                            set.retain(|c| c != ch);
                        }
                    }
                    set.sort_unstable();
                    server.user_modes = set.into_iter().collect();
                }
            }
            self.server_msg(
                sid,
                MsgKind::Mode,
                format!("{setter} sets mode {modes} on {target}"),
            );
        }
    }

    fn on_away(&mut self, sid: usize, msg: &IrcMessage) {
        let Some(nick) = msg.nick().map(str::to_string) else {
            return;
        };
        let away = if msg.params.is_empty() {
            None
        } else {
            Some(msg.trailing().to_string())
        };
        for buffer in &mut self.buffers {
            if buffer.server != sid {
                continue;
            }
            if let BufferKind::Channel { members, .. } = &mut buffer.kind {
                if let Some(member) = members
                    .iter_mut()
                    .find(|m| util::fold(&m.nick) == util::fold(&nick))
                {
                    member.away = away.clone();
                }
            }
            if let BufferKind::Query { member } = &mut buffer.kind {
                if util::fold(&member.nick) == util::fold(&nick) {
                    member.away = away.clone();
                }
            }
        }
    }

    fn on_account(&mut self, sid: usize, msg: &IrcMessage) {
        let Some(nick) = msg.nick().map(str::to_string) else {
            return;
        };
        let account = msg.param(0).map(str::to_string).filter(|a| a != "*");
        self.update_member_meta(sid, &nick, |m| m.account = account.clone());
    }

    fn on_chghost(&mut self, sid: usize, msg: &IrcMessage) {
        let Some(nick) = msg.nick().map(str::to_string) else {
            return;
        };
        let host = msg.param(1).map(str::to_string);
        self.update_member_meta(sid, &nick, |m| {
            m.host = host.clone();
        });
    }

    fn on_numeric(&mut self, sid: usize, code: &str, msg: &IrcMessage) {
        match code {
            "002" | "003" | "004" => {
                let text = msg.trailing().to_string();
                self.server_msg(sid, MsgKind::Server, text);
            }
            "005" => self.on_isupport(sid, msg),
            "251" | "252" | "253" | "254" | "255" | "265" | "266" => {
                let text = msg.trailing().to_string();
                self.server_msg(sid, MsgKind::Info, text);
            }
            "372" => {
                let text = msg.trailing().trim_start_matches("- ").to_string();
                self.server_msg(sid, MsgKind::Motd, text);
            }
            "375" => self.server_msg(sid, MsgKind::Motd, "— message of the day —"),
            "376" => self.server_msg(sid, MsgKind::Motd, "— end of motd —"),
            "321" => {}
            "322" => self.on_list_entry(sid, msg),
            "323" => {
                let mut count = None;
                if let Some(list) = self.list.as_mut() {
                    if list.server == sid {
                        list.finish();
                        count = Some(list.entries.len());
                    }
                }
                if let Some(count) = count {
                    self.toast(format!("Channel list: {count} channels"));
                }
            }
            "263" | "416" => {
                let text = msg.trailing().to_string();
                let mut shown = false;
                if let Some(list) = self.list.as_mut() {
                    if list.server == sid {
                        list.fail(&text);
                        shown = true;
                    }
                }
                if shown {
                    self.toast(format!("LIST failed: {text}"));
                } else {
                    self.server_msg(sid, MsgKind::Error, format!("{code}: {text}"));
                }
            }
            "331" => {
                if let Some(channel) = msg.param(1) {
                    if let Some(bid) = self.find_channel(sid, channel) {
                        if let BufferKind::Channel { topic, .. } = &mut self.buffers[bid].kind {
                            topic.clear();
                        }
                        self.push_system(bid, MsgKind::Topic, "No topic is set");
                    }
                }
            }
            "332" => {
                if let (Some(channel), Some(topic)) = (msg.param(1), msg.param(2)) {
                    let topic = util::sanitize_line(topic);
                    let bid = self.ensure_channel(sid, channel);
                    if let BufferKind::Channel { topic: t, .. } = &mut self.buffers[bid].kind {
                        *t = topic.clone();
                    }
                    self.push_system(bid, MsgKind::Topic, format!("Topic: {topic}"));
                }
            }
            "333" => {
                if let (Some(channel), Some(who)) = (msg.param(1), msg.param(2)) {
                    let when = msg
                        .param(3)
                        .and_then(|s| s.parse::<i64>().ok())
                        .and_then(|secs| chrono::DateTime::from_timestamp(secs, 0))
                        .map(|dt| {
                            dt.with_timezone(&Local)
                                .format("%Y-%m-%d %H:%M")
                                .to_string()
                        });
                    if let Some(bid) = self.find_channel(sid, channel) {
                        if let BufferKind::Channel { topic_by, .. } = &mut self.buffers[bid].kind {
                            *topic_by = Some(match when {
                                Some(when) => format!("{who} at {when}"),
                                None => who.to_string(),
                            });
                        }
                    }
                }
            }
            "353" => self.on_names(sid, msg),
            "366" => {}
            "324" => {
                if let (Some(channel), Some(modes)) = (msg.param(1), msg.param(2)) {
                    if let Some(bid) = self.find_channel(sid, channel) {
                        if let BufferKind::Channel { modes: m, .. } = &mut self.buffers[bid].kind {
                            *m = modes.trim_start_matches(['+', '-']).to_string();
                        }
                    }
                }
            }
            "301" => {
                if let Some(nick) = msg.param(1) {
                    let away = msg.trailing().to_string();
                    if let Some(whois) = self.server_mut(sid).and_then(|s| s.whois.as_mut()) {
                        if util::fold(&whois.nick) == util::fold(nick) {
                            whois.away = Some(away);
                        }
                    }
                }
            }
            "311" => {
                if let Some(whois) = self.server_mut(sid).and_then(|s| s.whois.as_mut()) {
                    if let (Some(nick), Some(user), Some(host), Some(real)) =
                        (msg.param(1), msg.param(2), msg.param(3), msg.param(5))
                    {
                        if util::fold(&whois.nick) == util::fold(nick) {
                            whois.user = user.to_string();
                            whois.host = host.to_string();
                            whois.realname = real.to_string();
                        }
                    }
                }
            }
            "312" => {
                if let Some(whois) = self.server_mut(sid).and_then(|s| s.whois.as_mut()) {
                    if let Some(server) = msg.param(2) {
                        whois.server = server.to_string();
                    }
                }
            }
            "313" => {
                if let Some(whois) = self.server_mut(sid).and_then(|s| s.whois.as_mut()) {
                    whois.oper = true;
                }
            }
            "317" => {
                if let Some(whois) = self.server_mut(sid).and_then(|s| s.whois.as_mut()) {
                    whois.idle = msg.param(2).and_then(|s| s.parse().ok());
                    whois.signon = msg.param(3).and_then(|s| s.parse().ok());
                }
            }
            "319" => {
                if let Some(whois) = self.server_mut(sid).and_then(|s| s.whois.as_mut()) {
                    whois.channels = msg
                        .trailing()
                        .split_whitespace()
                        .map(str::to_string)
                        .collect();
                }
            }
            "330" => {
                if let Some(whois) = self.server_mut(sid).and_then(|s| s.whois.as_mut()) {
                    whois.account = msg.param(2).map(str::to_string);
                }
            }
            "671" => {
                if let Some(whois) = self.server_mut(sid).and_then(|s| s.whois.as_mut()) {
                    whois.secure = true;
                }
            }
            "307" => {
                if let Some(whois) = self.server_mut(sid).and_then(|s| s.whois.as_mut()) {
                    whois.registered = true;
                }
            }
            "318" => self.finish_whois(sid),
            "432" | "433" | "436" | "437" => self.on_nick_taken(sid),
            "401" | "406" => {
                let text = msg.trailing().to_string();
                self.server_msg(sid, MsgKind::Error, format!("{code}: {text}"));
                if let Some(server) = self.server_mut(sid) {
                    server.whois = None;
                }
            }
            "221" => {
                if let Some(modes) = msg.param(1) {
                    if let Some(server) = self.server_mut(sid) {
                        server.user_modes = modes.to_string();
                    }
                }
            }
            "305" => self.server_msg(sid, MsgKind::Info, "You are no longer away"),
            "306" => self.server_msg(sid, MsgKind::Info, "You are now marked away"),
            "303" => {
                let text = msg.trailing().to_string();
                self.server_msg(sid, MsgKind::Info, format!("Away: {text}"));
            }
            "900" => {
                let text = msg.trailing().to_string();
                self.server_msg(sid, MsgKind::Info, text);
            }
            "903" => {
                self.server_msg(sid, MsgKind::Info, "SASL authentication successful");
                self.cap_end(sid);
            }
            "904" | "905" | "906" | "907" => {
                let text = msg.trailing().to_string();
                self.server_msg(sid, MsgKind::Error, format!("SASL failed: {text}"));
                self.cap_end(sid);
            }
            "908" => {
                let text = msg.trailing().to_string();
                self.server_msg(sid, MsgKind::Info, format!("SASL mechanisms: {text}"));
            }
            _ => {
                if code.starts_with('4') || code.starts_with('5') {
                    let text = msg.trailing().to_string();
                    let cmd = msg.param(1).unwrap_or("");
                    if text.is_empty() {
                        self.server_msg(sid, MsgKind::Error, format!("{code} {cmd}"));
                    } else {
                        self.server_msg(sid, MsgKind::Error, format!("{code} {cmd}: {text}"));
                    }
                }
            }
        }
    }

    fn on_list_entry(&mut self, sid: usize, msg: &IrcMessage) {
        let channel = util::sanitize_line(msg.param(1).unwrap_or(""));
        if channel.is_empty() {
            return;
        }
        let users = msg
            .param(2)
            .and_then(|value| value.parse::<u32>().ok())
            .unwrap_or(0);
        let topic = util::sanitize_line(msg.param(3).unwrap_or(""));
        let Some(list) = self.list.as_mut() else {
            return;
        };
        if list.server != sid {
            return;
        }
        if list.entries.len() >= 100_000 {
            list.fail("channel list is too large");
            return;
        }
        list.add(ListEntry::new(channel, users, topic));
    }

    fn on_isupport(&mut self, sid: usize, msg: &IrcMessage) {
        let mut network: Option<String> = None;
        let mut prefixes: Option<Vec<(char, char)>> = None;
        let mut chantypes: Option<Vec<char>> = None;
        for token in msg.params.iter().skip(1) {
            if let Some(value) = token.strip_prefix("NETWORK=") {
                network = Some(util::sanitize_line(value));
            } else if let Some(value) = token.strip_prefix("PREFIX=") {
                if let Some(open) = value.find('(') {
                    if let Some(close) = value.find(')') {
                        let modes: Vec<char> = value[open + 1..close].chars().collect();
                        let symbols: Vec<char> = value[close + 1..].chars().collect();
                        let mut map = Vec::new();
                        for (mode, symbol) in modes.iter().zip(symbols.iter()) {
                            map.push((*mode, *symbol));
                        }
                        if !map.is_empty() {
                            prefixes = Some(map);
                        }
                    }
                }
            } else if let Some(value) = token.strip_prefix("CHANTYPES=") {
                chantypes = Some(value.chars().collect());
            }
        }
        if let Some(server) = self.server_mut(sid) {
            if let Some(network) = network.clone() {
                server.network = Some(network);
            }
            if let Some(prefixes) = prefixes {
                server.prefix_modes = prefixes;
            }
            if let Some(chantypes) = chantypes {
                server.chantypes = chantypes;
            }
        }
        if let Some(network) = network {
            if let Some(bid) = self.server_buffer(sid) {
                if let BufferKind::Server { title } = &mut self.buffers[bid].kind {
                    *title = network;
                }
            }
        }
    }

    fn on_names(&mut self, sid: usize, msg: &IrcMessage) {
        let Some(channel) = msg
            .params
            .iter()
            .skip(1)
            .find(|p| {
                self.server(sid)
                    .map(|s| s.is_channel_name(p))
                    .unwrap_or_else(|| p.starts_with('#'))
            })
            .cloned()
        else {
            return;
        };
        let symbols: String = self
            .server(sid)
            .map(|s| s.prefix_modes.iter().map(|(_, p)| *p).collect())
            .unwrap_or_else(|| "@+".to_string());
        let names = msg.trailing().to_string();
        let bid = self.ensure_channel(sid, &channel);
        for token in names.split_whitespace() {
            let mut nick = token;
            let mut prefixes = String::new();
            while let Some(c) = nick.chars().next() {
                if symbols.contains(c) {
                    prefixes.push(c);
                    nick = &nick[c.len_utf8()..];
                } else {
                    break;
                }
            }
            if nick.is_empty() {
                continue;
            }
            self.merge_member(bid, nick, &prefixes, true, None, None);
        }
    }

    fn on_nick_taken(&mut self, sid: usize) {
        let registered = self.server(sid).map(|s| s.registered).unwrap_or(false);
        if registered {
            self.server_msg(sid, MsgKind::Error, "Nickname is unavailable");
            return;
        }
        let new_nick = {
            let Some(server) = self.server_mut(sid) else {
                return;
            };
            if server.nick.len() > 24 {
                return;
            }
            server.nick.push('_');
            server.nick.clone()
        };
        self.server_msg(
            sid,
            MsgKind::Info,
            format!("Nickname taken, trying {new_nick}"),
        );
        self.send(sid, format!("NICK {new_nick}"));
    }

    fn finish_whois(&mut self, sid: usize) {
        let Some(whois) = self.server_mut(sid).and_then(|s| s.whois.take()) else {
            return;
        };
        if whois.nick.is_empty() {
            return;
        }
        let mut menu = Menu::new(format!("whois {}", whois.nick));
        menu.info.push(whois.hostmask());
        if !whois.realname.is_empty() {
            menu.info.push(format!(
                "realname   {}",
                util::sanitize_line(&whois.realname)
            ));
        }
        if !whois.server.is_empty() {
            menu.info
                .push(format!("server     {}", util::sanitize_line(&whois.server)));
        }
        if let Some(account) = &whois.account {
            menu.info.push(format!("account    {account}"));
        }
        if let Some(away) = &whois.away {
            menu.info
                .push(format!("away       {}", util::sanitize_line(away)));
        }
        if let Some(idle) = whois.idle {
            menu.info
                .push(format!("idle       {}", util::short_duration(idle)));
        }
        if let Some(signon) = whois.signon {
            if let Some(dt) = chrono::DateTime::from_timestamp(signon, 0) {
                menu.info.push(format!(
                    "signed on  {}",
                    dt.with_timezone(&Local).format("%Y-%m-%d %H:%M")
                ));
            }
        }
        if !whois.channels.is_empty() {
            menu.info.push(format!(
                "channels   {}",
                util::sanitize_line(&whois.channels.join(" "))
            ));
        }
        if whois.oper {
            menu.info.push("status     IRC operator".to_string());
        }
        if whois.secure {
            menu.info
                .push("status     using a secure connection".to_string());
        }
        if whois.registered {
            menu.info.push("status     registered nickname".to_string());
        }
        menu.info.push("".to_string());
        menu.actions.push(MenuAction::Copy {
            text: whois.hostmask(),
            label: "Copy hostmask".to_string(),
        });
        menu.actions.push(MenuAction::Query {
            server: sid,
            nick: whois.nick.clone(),
        });
        menu.actions.push(MenuAction::CloseMenu);
        self.menu = Some(menu);
    }

    fn merge_member(
        &mut self,
        bid: usize,
        nick: &str,
        prefixes: &str,
        replace: bool,
        account: Option<String>,
        realname: Option<String>,
    ) {
        let Some(buffer) = self.buffers.get_mut(bid) else {
            return;
        };
        let Some(members) = buffer.members_mut() else {
            return;
        };
        let clean_nick = util::sanitize_line(nick);
        if let Some(member) = members
            .iter_mut()
            .find(|m| util::fold(&m.nick) == util::fold(&clean_nick))
        {
            if !prefixes.is_empty() {
                if replace {
                    member.prefixes = prefixes.to_string();
                } else {
                    for symbol in prefixes.chars() {
                        if !member.prefixes.contains(symbol) {
                            member.prefixes.push(symbol);
                        }
                    }
                }
            }
            if account.is_some() {
                member.account = account;
            }
            if realname.is_some() {
                member.realname = realname;
            }
        } else {
            let mut member = Member::new(clean_nick, prefixes);
            member.account = account;
            member.realname = realname;
            members.push(member);
        }
    }

    fn remove_member(&mut self, bid: usize, nick: &str) {
        if let Some(buffer) = self.buffers.get_mut(bid) {
            if let Some(members) = buffer.members_mut() {
                members.retain(|m| util::fold(&m.nick) != util::fold(nick));
            }
        }
    }

    fn set_member_prefix(&mut self, bid: usize, nick: &str, symbol: char, adding: bool) {
        if let Some(buffer) = self.buffers.get_mut(bid) {
            if let Some(members) = buffer.members_mut() {
                if let Some(member) = members
                    .iter_mut()
                    .find(|m| util::fold(&m.nick) == util::fold(nick))
                {
                    if adding {
                        if !member.prefixes.contains(symbol) {
                            member.prefixes.push(symbol);
                        }
                    } else {
                        member.prefixes.retain(|c| c != symbol);
                    }
                }
            }
        }
    }

    fn update_member_meta<F: Fn(&mut Member)>(&mut self, sid: usize, nick: &str, update: F) {
        for buffer in &mut self.buffers {
            if buffer.server != sid {
                continue;
            }
            match &mut buffer.kind {
                BufferKind::Channel { members, .. } => {
                    for member in members.iter_mut() {
                        if util::fold(&member.nick) == util::fold(nick) {
                            update(member);
                        }
                    }
                }
                BufferKind::Query { member } => {
                    if util::fold(&member.nick) == util::fold(nick) {
                        update(member);
                    }
                }
                _ => {}
            }
        }
    }

    // -------------------------------------------------------------- input ---

    pub fn handle_key(&mut self, key: KeyEvent) {
        if self.menu.is_some() {
            self.handle_menu_key(key);
            return;
        }
        if self.list.is_some() {
            self.handle_list_key(key);
            return;
        }
        match self.mode {
            Mode::Normal => normal::handle(self, key),
            Mode::Visual { .. } => visual::handle(self, key),
            Mode::Insert | Mode::Command => insert::handle(self, key),
        }
    }

    fn handle_menu_key(&mut self, key: KeyEvent) {
        let mut run: Option<MenuAction> = None;
        let mut close = false;
        let mut copied: Option<String> = None;
        {
            let Some(menu) = self.menu.as_mut() else {
                return;
            };
            let len = menu.actions.len();
            match key.code {
                KeyCode::Esc => close = true,
                KeyCode::Char('q') => close = true,
                KeyCode::Char('j') | KeyCode::Down => {
                    if len > 0 {
                        menu.selected = (menu.selected + 1) % len;
                    }
                }
                KeyCode::Char('k') | KeyCode::Up => {
                    if len > 0 {
                        menu.selected = (menu.selected + len - 1) % len;
                    }
                }
                KeyCode::Char('g') => menu.selected = 0,
                KeyCode::Char('G') => menu.selected = len.saturating_sub(1),
                KeyCode::Enter => {
                    if let Some(action) = menu.actions.get(menu.selected).cloned() {
                        run = Some(action);
                        close = true;
                    } else {
                        close = true;
                    }
                }
                KeyCode::Char('y') => copied = Some(menu.info_text()),
                KeyCode::Char(c) if c.is_ascii_digit() => {
                    let idx = (c as u8).saturating_sub(b'1') as usize;
                    if idx < len {
                        menu.selected = idx;
                    }
                }
                _ => {}
            }
        }
        if let Some(text) = copied {
            self.copy(text, "menu details");
            return;
        }
        if close {
            self.menu = None;
        }
        if let Some(action) = run {
            self.run_action(action);
        }
    }

    fn handle_list_key(&mut self, key: KeyEvent) {
        let mut join: Option<(usize, String)> = None;
        let mut open_menu = false;
        let mut refresh = false;
        let mut close = false;
        let mut copied: Option<String> = None;
        {
            let Some(list) = self.list.as_mut() else {
                return;
            };
            let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
            if list.filtering {
                match key.code {
                    KeyCode::Enter | KeyCode::Esc => list.filtering = false,
                    KeyCode::Backspace => {
                        list.filter.pop();
                        list.normalize_selection();
                    }
                    KeyCode::Char('u') if ctrl => list.clear_filter(),
                    KeyCode::Char(c) if !ctrl && !key.modifiers.contains(KeyModifiers::ALT) => {
                        list.filter.push(c);
                        list.normalize_selection();
                    }
                    _ => {}
                }
                return;
            }
            match key.code {
                KeyCode::Esc => {
                    if !list.filter.is_empty() {
                        list.clear_filter();
                    } else {
                        close = true;
                    }
                }
                KeyCode::Char('f') | KeyCode::Char('/') => list.filtering = true,
                KeyCode::Char('j') | KeyCode::Down => list.move_selection(1),
                KeyCode::Char('k') | KeyCode::Up => list.move_selection(-1),
                KeyCode::Char('g') | KeyCode::Home => list.jump(false),
                KeyCode::Char('G') | KeyCode::End => list.jump(true),
                KeyCode::PageDown => list.move_selection(15),
                KeyCode::PageUp => list.move_selection(-15),
                KeyCode::Enter => open_menu = true,
                KeyCode::Char('l') | KeyCode::Right => {
                    if let Some(entry) = list.selected_entry() {
                        join = Some((list.server, entry.channel.clone()));
                    }
                }
                KeyCode::Char('y') => {
                    if let Some(entry) = list.selected_entry() {
                        copied = Some(entry.channel.clone());
                    }
                }
                KeyCode::Char('r') => refresh = true,
                KeyCode::Char('q') => close = true,
                _ => {}
            }
        }
        if close {
            self.list = None;
            return;
        }
        if let Some(text) = copied {
            self.copy(text, "channel name");
        }
        if open_menu {
            self.open_list_channel_menu();
        }
        if let Some((sid, channel)) = join {
            self.join_channel(sid, channel);
        }
        if refresh {
            self.refresh_list();
        }
    }

    pub fn open_channel_list(&mut self, args: &str) {
        let Some(bid) = self.active else {
            self.toast("no buffer selected");
            return;
        };
        let sid = self.buffers[bid].server;
        let Some((name, demo, local, connected, has_tx)) = self.server(sid).map(|server| {
            (
                server.display().to_string(),
                server.demo,
                server.local,
                server.connected,
                server.tx.is_some(),
            )
        }) else {
            return;
        };
        if local {
            self.toast("not connected — use :connect <host>");
            return;
        }
        if !demo && (!connected || !has_tx) {
            self.toast("not connected");
            return;
        }
        self.request_list(sid, &name, args.trim(), demo);
    }

    fn request_list(&mut self, sid: usize, name: &str, args: &str, demo: bool) {
        let mut list = ChannelList::new(sid, name, args);
        if demo {
            for entry in crate::demo::channel_list() {
                list.add(entry);
            }
            list.finish();
            let count = list.entries.len();
            self.list = Some(list);
            self.toast(format!("Demo channel list: {count} channels"));
            return;
        }
        let line = if args.is_empty() {
            "LIST".to_string()
        } else {
            format!("LIST {args}")
        };
        if !self.send_silent(sid, line) {
            self.toast("not connected");
            return;
        }
        self.list = Some(list);
        self.toast("listing channels…");
    }

    fn refresh_list(&mut self) {
        let Some(list) = self.list.as_ref() else {
            return;
        };
        let sid = list.server;
        let name = list.server_name.clone();
        let args = list.args.clone();
        let demo = self.server(sid).map(|server| server.demo).unwrap_or(false);
        self.request_list(sid, &name, &args, demo);
    }

    fn join_channel(&mut self, sid: usize, channel: String) {
        let bid = self.ensure_channel(sid, &channel);
        self.set_active(bid);
        self.send(sid, format!("JOIN {channel}"));
        self.list = None;
    }

    fn open_list_channel_menu(&mut self) {
        let Some(list) = self.list.as_ref() else {
            return;
        };
        let Some(entry) = list.selected_entry().cloned() else {
            self.toast("no channel selected");
            return;
        };
        let sid = list.server;
        let server_name = list.server_name.clone();
        let mut menu = Menu::new(entry.channel.clone());
        menu.info
            .push(format!("{} users · on {server_name}", entry.users));
        if entry.topic.is_empty() {
            menu.info.push("no topic".to_string());
        } else {
            menu.info.push(entry.topic.clone());
        }
        menu.actions.push(MenuAction::Join {
            server: sid,
            channel: entry.channel.clone(),
        });
        menu.actions.push(MenuAction::Copy {
            text: entry.channel.clone(),
            label: "Copy channel name".to_string(),
        });
        if !entry.topic.is_empty() {
            menu.actions.push(MenuAction::Copy {
                text: entry.topic,
                label: "Copy topic".to_string(),
            });
        }
        menu.actions.push(MenuAction::CloseMenu);
        self.menu = Some(menu);
    }

    pub fn start_insert(&mut self) {
        if self.active.is_none() {
            return;
        }
        self.last_focus = self.focus;
        self.mode = Mode::Insert;
    }

    pub fn start_command(&mut self, prefix: &str) {
        self.last_focus = self.focus;
        self.mode = Mode::Command;
        self.input.set_text(prefix);
        self.input.cursor = 1;
    }

    pub fn fill_input(&mut self, text: &str, insert: bool) {
        self.mode = if insert { Mode::Insert } else { Mode::Command };
        self.input.set_text(text);
        if insert {
            self.focus = Focus::Messages;
        }
    }

    pub fn to_normal(&mut self) {
        if self.mode == Mode::Command {
            self.input.clear();
        }
        self.mode = Mode::Normal;
        self.focus = self.last_focus;
    }

    pub fn submit_text(&mut self) {
        let text = self.input.take();
        let command = text.starts_with('/') || text.starts_with(':');
        self.mode = Mode::Normal;
        self.focus = Focus::Messages;
        if text.trim().is_empty() {
            return;
        }
        if command {
            let body = text[1..].trim();
            self.run_command(body);
        } else {
            self.send_chat(&text);
        }
    }

    pub fn complete(&mut self) {
        if self.mode == Mode::Command {
            self.complete_command();
        } else {
            self.complete_nick();
        }
    }

    fn complete_command(&mut self) {
        let text = self.input.text();
        let mut chars: Vec<char> = text.chars().collect();
        if chars.is_empty() || (chars[0] != ':' && chars[0] != '/') {
            return;
        }
        let cursor = self.input.cursor.min(chars.len());
        if cursor < 2 {
            return;
        }
        if chars[1..cursor].iter().any(|c| c.is_whitespace()) {
            return;
        }
        let partial: String = chars[1..cursor].iter().collect();
        let matches: Vec<&str> = COMMANDS
            .iter()
            .filter(|c| c.starts_with(&partial))
            .copied()
            .collect();
        if matches.is_empty() {
            return;
        }
        if matches.len() == 1 {
            let command = matches[0].to_string();
            chars.splice(1..cursor, format!("{command} ").chars());
            let new_text: String = chars.into_iter().collect();
            self.input.set_text(&new_text);
            return;
        }
        let mut common = matches[0].to_string();
        for candidate in matches.iter().skip(1) {
            let len = common
                .chars()
                .zip(candidate.chars())
                .take_while(|(a, b)| a == b)
                .count();
            let byte_len = common
                .char_indices()
                .nth(len)
                .map(|(idx, _)| idx)
                .unwrap_or(common.len());
            common.truncate(byte_len);
        }
        if common.len() > partial.len() {
            chars.splice(1..cursor, common.chars());
            let new_text: String = chars.into_iter().collect();
            self.input.set_text(&new_text);
        }
        let names: Vec<String> = matches.iter().map(|s| s.to_string()).collect();
        self.toast(format!("matches: {}", names.join(" ")));
    }

    fn complete_nick(&mut self) {
        let start = self.input.word_start();
        let prefix = self.input.word_prefix();
        if prefix.is_empty() {
            return;
        }
        let at_start = start == 0;
        let (typed, lead) = match prefix.strip_prefix('@') {
            Some(rest) => (rest.to_string(), "@"),
            None => (prefix.clone(), ""),
        };
        let Some(bid) = self.active else { return };
        let mut candidates: Vec<String> = self.buffers[bid]
            .members()
            .iter()
            .map(|m| m.nick.clone())
            .filter(|nick| util::fold(nick).starts_with(&util::fold(&typed)))
            .collect();
        if candidates.is_empty() {
            return;
        }
        candidates.sort_by_key(|n| util::fold(n));
        candidates.dedup_by(|a, b| util::fold(a) == util::fold(b));
        if candidates.len() == 1 {
            let nick = &candidates[0];
            let suffix = if at_start { ": " } else { " " };
            let replacement = format!("{lead}{nick}{suffix}");
            self.input.replace_word(start, &replacement);
        } else {
            let mut common = candidates[0].clone();
            for nick in candidates.iter().skip(1) {
                let len = common
                    .chars()
                    .zip(nick.chars())
                    .take_while(|(a, b)| util::fold(&a.to_string()) == util::fold(&b.to_string()))
                    .count();
                common = common.chars().take(len).collect();
            }
            if common.len() > typed.len() {
                self.input.replace_word(start, &format!("{lead}{common}"));
            }
            self.toast(format!("matches: {}", candidates.join(" ")));
        }
    }

    // ----------------------------------------------------------- messaging ---

    pub fn send_chat(&mut self, text: &str) {
        let Some(bid) = self.active else { return };
        let (sid, target, is_server) = {
            let Some(buffer) = self.buffers.get(bid) else {
                return;
            };
            (buffer.server, buffer.name().to_string(), buffer.is_server())
        };
        if is_server {
            if self.server(sid).map(|s| s.local || s.demo).unwrap_or(true) {
                self.toast("not connected — use :connect <host>");
            } else {
                self.send(sid, text);
            }
            return;
        }
        let nick = self.nick_of(sid);
        let chunks = util::irc_chunks(text, 420);
        let mut delivered = true;
        for chunk in &chunks {
            if !self.send_silent(sid, format!("PRIVMSG {target} :{chunk}")) {
                delivered = false;
                break;
            }
            let mut message = Message::new(MsgKind::Privmsg, Some(&nick), chunk.as_str());
            message.own = true;
            self.push_msg(bid, message);
        }
        if !delivered {
            self.toast("not connected — message not fully sent");
        }
    }

    pub fn send_action(&mut self, text: &str) {
        let Some(bid) = self.active else { return };
        let (sid, target, is_server) = {
            let Some(buffer) = self.buffers.get(bid) else {
                return;
            };
            (buffer.server, buffer.name().to_string(), buffer.is_server())
        };
        if is_server {
            return;
        }
        let nick = self.nick_of(sid);
        let chunks = util::irc_chunks(text, 410);
        let mut delivered = true;
        for chunk in &chunks {
            if !self.send_silent(sid, format!("PRIVMSG {target} :\u{1}ACTION {chunk}\u{1}")) {
                delivered = false;
                break;
            }
            let mut message = Message::new(MsgKind::Action, Some(&nick), chunk.as_str());
            message.own = true;
            self.push_msg(bid, message);
        }
        if !delivered {
            self.toast("not connected — message not fully sent");
        }
    }

    // ------------------------------------------------------------ commands ---

    pub fn run_command(&mut self, raw: &str) {
        let raw = raw.trim();
        if raw.is_empty() {
            return;
        }
        let (cmd, rest) = match raw.split_once(char::is_whitespace) {
            Some((cmd, rest)) => (cmd.to_ascii_lowercase(), rest.trim()),
            None => (raw.to_ascii_lowercase(), ""),
        };
        let Some(active) = self.active else {
            match cmd.as_str() {
                "connect" | "server" => self.cmd_connect(rest),
                "help" | "h" => self.menu = Some(help_menu()),
                "q" | "quit" | "exit" => self.quit_all(rest),
                _ => self.toast("no buffer selected"),
            }
            return;
        };
        let sid = self.buffers[active].server;
        match cmd.as_str() {
            "q" | "quit" | "exit" => self.quit_all(rest),
            "join" | "j" => self.cmd_join(sid, active, rest),
            "part" | "leave" => self.cmd_part(sid, active, rest),
            "me" => {
                if rest.is_empty() {
                    self.toast("usage: /me <action>");
                } else {
                    self.send_action(rest);
                }
            }
            "msg" | "query" => self.cmd_msg(sid, rest),
            "nick" => {
                if rest.is_empty() || rest.contains(' ') {
                    self.toast("usage: /nick <newnick>");
                } else {
                    if self.server(sid).map(|s| s.demo).unwrap_or(false) {
                        if let Some(server) = self.server_mut(sid) {
                            server.nick = rest.to_string();
                        }
                    }
                    self.send(sid, format!("NICK {rest}"));
                }
            }
            "topic" => self.cmd_topic(sid, active, rest),
            "mode" => {
                if rest.is_empty() {
                    self.toast("usage: /mode <target> <modes>");
                } else {
                    self.send(sid, format!("MODE {rest}"));
                }
            }
            "whois" => {
                let nick = rest.split_whitespace().next().unwrap_or("");
                if nick.is_empty() {
                    self.toast("usage: /whois <nick>");
                } else {
                    if let Some(server) = self.server_mut(sid) {
                        server.whois = Some(Whois::new(nick));
                    }
                    self.send(sid, format!("WHOIS {nick}"));
                }
            }
            "away" => {
                if rest.is_empty() {
                    self.send(sid, "AWAY");
                } else {
                    self.send(sid, format!("AWAY :{rest}"));
                }
            }
            "invite" => {
                let mut parts = rest.split_whitespace();
                let nick = parts.next().unwrap_or("");
                let channel = parts
                    .next()
                    .map(str::to_string)
                    .or_else(|| self.channel_name(active));
                match (nick.is_empty(), channel) {
                    (false, Some(channel)) => self.send(sid, format!("INVITE {nick} {channel}")),
                    _ => self.toast("usage: /invite <nick> [#channel]"),
                }
            }
            "kick" => {
                let Some(channel) = self.channel_name(active) else {
                    self.toast("not in a channel");
                    return;
                };
                let mut parts = rest.splitn(2, char::is_whitespace);
                let nick = parts.next().unwrap_or("");
                let reason = parts.next().unwrap_or("").trim();
                if nick.is_empty() {
                    self.toast("usage: /kick <nick> [reason]");
                } else if reason.is_empty() {
                    self.send(sid, format!("KICK {channel} {nick}"));
                } else {
                    self.send(sid, format!("KICK {channel} {nick} :{reason}"));
                }
            }
            "op" | "deop" | "voice" | "devoice" => {
                let Some(channel) = self.channel_name(active) else {
                    self.toast("not in a channel");
                    return;
                };
                let nick = rest.split_whitespace().next().unwrap_or("");
                if nick.is_empty() {
                    self.toast(format!("usage: /{cmd} <nick>"));
                    return;
                }
                let mode = match cmd.as_str() {
                    "op" => "+o",
                    "deop" => "-o",
                    "voice" => "+v",
                    _ => "-v",
                };
                self.send(sid, format!("MODE {channel} {mode} {nick}"));
            }
            "list" | "channels" => self.open_channel_list(rest),
            "clear" => {
                if let Some(buffer) = self.buffers.get_mut(active) {
                    buffer.messages.clear();
                    buffer.selected = 0;
                    buffer.scroll = 0;
                }
            }
            "close" => self.close_buffer(active),
            "connect" | "server" => self.cmd_connect(rest),
            "disconnect" => self.disconnect_server(sid),
            "reconnect" => self.reconnect(sid),
            "raw" | "quote" => {
                if rest.is_empty() {
                    self.toast("usage: /raw <line>");
                } else {
                    self.send(sid, rest.to_string());
                }
            }
            "help" | "h" => self.menu = Some(help_menu()),
            other => self.toast(format!("unknown command: /{other}")),
        }
    }

    fn cmd_join(&mut self, sid: usize, active: usize, rest: &str) {
        if rest.is_empty() {
            if let Some(channel) = self.channel_name(active) {
                self.send(sid, format!("JOIN {channel}"));
            } else {
                self.toast("usage: /join <#channel>");
            }
            return;
        }
        let channels: Vec<String> = rest
            .split(|c: char| c == ',' || c.is_whitespace())
            .filter(|c| !c.is_empty())
            .map(|c| {
                if self
                    .server(sid)
                    .map(|s| s.is_channel_name(c))
                    .unwrap_or_else(|| c.starts_with('#'))
                {
                    c.to_string()
                } else {
                    format!("#{c}")
                }
            })
            .collect();
        if channels.is_empty() {
            return;
        }
        self.send(sid, format!("JOIN {}", channels.join(",")));
        let mut last = None;
        for channel in &channels {
            let bid = self.ensure_channel(sid, channel);
            last = Some(bid);
            self.push_system(bid, MsgKind::Info, format!("→ Joining {channel}…"));
        }
        if let Some(bid) = last {
            self.set_active(bid);
        }
    }

    fn cmd_part(&mut self, sid: usize, active: usize, rest: &str) {
        let tokens: Vec<&str> = rest.split_whitespace().collect();
        let (channel, reason) = match tokens.first() {
            Some(first)
                if self
                    .server(sid)
                    .map(|s| s.is_channel_name(first))
                    .unwrap_or_else(|| first.starts_with('#')) =>
            {
                (
                    first.to_string(),
                    tokens.get(1..).map(|r| r.join(" ")).unwrap_or_default(),
                )
            }
            _ => match self.channel_name(active) {
                Some(channel) => (channel, rest.to_string()),
                None => {
                    self.toast("usage: /part [#channel] [reason]");
                    return;
                }
            },
        };
        if reason.is_empty() {
            self.send(sid, format!("PART {channel}"));
        } else {
            self.send(sid, format!("PART {channel} :{reason}"));
        }
        if let Some(bid) = self.find_channel(sid, &channel) {
            if let BufferKind::Channel { joined, .. } = &mut self.buffers[bid].kind {
                *joined = false;
            }
            if self.server(sid).map(|s| s.demo).unwrap_or(false) {
                self.push_system(bid, MsgKind::Part, format!("← You left {channel}"));
            }
        }
    }

    fn cmd_msg(&mut self, sid: usize, rest: &str) {
        let mut parts = rest.splitn(2, char::is_whitespace);
        let nick = parts.next().unwrap_or("");
        let text = parts.next().unwrap_or("").trim();
        if nick.is_empty() {
            self.toast("usage: /msg <nick> [text]");
            return;
        }
        let bid = self.ensure_query(sid, nick);
        self.set_active(bid);
        if !text.is_empty() {
            let my_nick = self.nick_of(sid);
            let delivered = self.send_silent(sid, format!("PRIVMSG {nick} :{text}"));
            if delivered {
                let mut message = Message::new(MsgKind::Privmsg, Some(&my_nick), text);
                message.own = true;
                self.push_msg(bid, message);
            } else {
                self.toast("not connected — message not sent");
            }
        }
    }

    fn cmd_topic(&mut self, sid: usize, active: usize, rest: &str) {
        let Some(channel) = self.channel_name(active) else {
            self.toast("not in a channel");
            return;
        };
        if rest.is_empty() {
            self.send(sid, format!("TOPIC {channel}"));
        } else {
            self.send(sid, format!("TOPIC {channel} :{rest}"));
        }
    }

    fn cmd_connect(&mut self, rest: &str) {
        let mut tokens = rest.split_whitespace();
        let spec = tokens.next().unwrap_or("irc.libera.chat");
        let mut explicit_port = None;
        let mut tls = true;
        let mut tls_explicit = false;
        for token in tokens {
            match token.to_ascii_lowercase().as_str() {
                "notls" | "plain" | "plaintext" => {
                    tls = false;
                    tls_explicit = true;
                }
                "tls" | "ssl" => {
                    tls = true;
                    tls_explicit = true;
                }
                other => {
                    if let Ok(port) = other.parse::<u16>() {
                        explicit_port = Some(port);
                    }
                }
            }
        }
        let (host, spec_port) = match spec.rsplit_once(':') {
            Some((host, port)) if !host.contains(':') => match port.parse::<u16>() {
                Ok(port) => (host.to_string(), Some(port)),
                Err(_) => (spec.to_string(), None),
            },
            _ => (spec.to_string(), None),
        };
        let port = explicit_port
            .or(spec_port)
            .unwrap_or(if tls { 6697 } else { 6667 });
        if !tls_explicit && port == 6667 {
            tls = false;
        }
        if host.is_empty() {
            self.toast("usage: /connect <host[:port]>");
            return;
        }
        if let Some(existing) = self
            .servers
            .iter()
            .find(|s| s.host == host && s.port == port)
        {
            let sid = existing.id;
            let connected = existing.connected;
            if let Some(bid) = self.server_buffer(sid) {
                self.set_active(bid);
            }
            if !connected {
                self.reconnect(sid);
            }
            return;
        }
        let nick = self
            .active_buffer()
            .map(|buffer| self.nick_of(buffer.server))
            .unwrap_or_else(|| self.default_nick.clone());
        let cfg = ServerConfig {
            host,
            port,
            tls,
            nick,
            sasl: None,
            pass: None,
            channels: Vec::new(),
        };
        self.start_server(cfg);
    }

    pub fn channel_name(&self, bid: usize) -> Option<String> {
        self.buffers.get(bid).and_then(|b| {
            if let BufferKind::Channel { name, .. } = &b.kind {
                Some(name.clone())
            } else {
                None
            }
        })
    }

    pub fn disconnect_server(&mut self, sid: usize) {
        if let Some(server) = self.server_mut(sid) {
            if let Some(tx) = server.tx.take() {
                let _ = tx.send("QUIT :leaving".to_string());
            }
            server.connected = false;
            server.registered = false;
            server.connecting = false;
        }
        for buffer in &mut self.buffers {
            if buffer.server == sid {
                if let BufferKind::Channel {
                    joined, members, ..
                } = &mut buffer.kind
                {
                    *joined = false;
                    members.clear();
                }
            }
        }
        self.server_msg(sid, MsgKind::Info, "Disconnected");
    }

    pub fn reconnect(&mut self, sid: usize) {
        let connected = self.server(sid).map(|s| s.connected).unwrap_or(false);
        if connected {
            self.disconnect_server(sid);
        }
        let local = self.server(sid).map(|s| s.local).unwrap_or(true);
        if local {
            self.toast("cannot reconnect the local buffer");
            return;
        }
        if let Some(server) = self.server_mut(sid) {
            server.tx = None;
        }
        self.spawn_server(sid);
        self.server_msg(sid, MsgKind::Info, "Reconnecting…");
    }

    pub fn quit_all(&mut self, reason: &str) {
        let reason = if reason.is_empty() {
            "rirc — bye"
        } else {
            reason
        };
        for server in &self.servers {
            if server.local {
                continue;
            }
            if let Some(tx) = &server.tx {
                let _ = tx.send(format!("QUIT :{reason}"));
            }
        }
        self.quit = true;
    }

    // ------------------------------------------------------------- buffers ---

    pub fn next_buffer(&mut self, delta: isize) {
        let order = self.ordered_buffers();
        if order.is_empty() {
            return;
        }
        let current = self
            .active
            .and_then(|a| order.iter().position(|&i| i == a))
            .unwrap_or(0);
        let len = order.len() as isize;
        let next = (current as isize + delta).rem_euclid(len) as usize;
        self.set_active(order[next]);
    }

    pub fn cycle_focus(&mut self, delta: isize) {
        let panels = self.visible_panels();
        if panels.is_empty() {
            return;
        }
        let current = panels.iter().position(|p| *p == self.focus).unwrap_or(0);
        let len = panels.len() as isize;
        let next = (current as isize + delta).rem_euclid(len) as usize;
        self.focus = panels[next];
        if self.focus == Focus::Members {
            let max = self
                .active
                .map(|bid| self.member_order(bid).len())
                .unwrap_or(0);
            self.member_nav = self.member_nav.min(max.saturating_sub(1));
        }
    }

    pub fn visible_panels(&self) -> Vec<Focus> {
        let mut panels = vec![Focus::Messages];
        if self.width >= 64 {
            panels.push(Focus::Channels);
        }
        if self.members_visible() {
            panels.push(Focus::Members);
        }
        panels
    }

    pub fn members_visible(&self) -> bool {
        self.width >= 76
    }

    pub fn sidebar_visible(&self) -> bool {
        self.width >= 64
    }

    pub fn close_buffer(&mut self, idx: usize) {
        if idx >= self.buffers.len() {
            return;
        }
        let server = self.buffers[idx].server;
        let is_server = self.buffers[idx].is_server();
        let active_id = self.active.map(|i| self.buffers[i].id);
        self.active = None;
        self.mode = Mode::Normal;
        self.menu = None;
        if is_server {
            self.disconnect_server(server);
            let local = self.server(server).map(|s| s.local).unwrap_or(false);
            if local {
                if let Some(pos) = self.server_pos(server) {
                    self.servers.remove(pos);
                }
            }
            self.buffers.retain(|b| b.server != server);
        } else {
            self.buffers.remove(idx);
        }
        if let Some(id) = active_id {
            if let Some(pos) = self.buffers.iter().position(|b| b.id == id) {
                self.set_active(pos);
                return;
            }
        }
        if let Some(pos) = self
            .buffers
            .iter()
            .position(|b| b.server == server && b.is_server())
        {
            self.set_active(pos);
        } else if !self.buffers.is_empty() {
            self.set_active(self.buffers.len() - 1);
        } else {
            self.sidebar_nav = 0;
        }
    }

    // ------------------------------------------------------------ movement ---

    pub fn select_msg(&mut self, delta: isize) {
        let Some(buffer) = self.active_buffer_mut() else {
            return;
        };
        if buffer.messages.is_empty() {
            return;
        }
        let max = buffer.messages.len() as isize - 1;
        let next = (buffer.selected as isize + delta).clamp(0, max);
        buffer.selected = next as usize;
    }

    pub fn jump_message(&mut self, end: bool) {
        let Some(buffer) = self.active_buffer_mut() else {
            return;
        };
        if buffer.messages.is_empty() {
            return;
        }
        buffer.selected = if end { buffer.messages.len() - 1 } else { 0 };
    }

    pub fn enter_visual(&mut self) {
        let Some(buffer) = self.active_buffer() else {
            return;
        };
        if buffer.messages.is_empty() {
            return;
        }
        self.mode = Mode::Visual {
            anchor: buffer.selected.min(buffer.messages.len() - 1),
        };
        self.last_focus = Focus::Messages;
    }

    pub fn toggle_visual_end(&mut self) {
        if let Mode::Visual { anchor } = self.mode {
            if let Some(buffer) = self.active_buffer() {
                let cursor = buffer.selected;
                self.mode = Mode::Visual { anchor: cursor };
                if let Some(buffer) = self.active_buffer_mut() {
                    buffer.selected = anchor;
                }
            }
        }
    }

    pub fn selection_bounds(&self) -> Option<(usize, usize)> {
        let buffer = self.active_buffer()?;
        if buffer.messages.is_empty() {
            return None;
        }
        let last = buffer.messages.len() - 1;
        match self.mode {
            Mode::Normal => {
                let idx = buffer.selected.min(last);
                Some((idx, idx))
            }
            Mode::Visual { anchor } => {
                let a = anchor.min(last);
                let b = buffer.selected.min(last);
                Some((a.min(b), a.max(b)))
            }
            _ => None,
        }
    }

    pub fn scroll_view(&mut self, lines: usize, up: bool) {
        if let Some(buffer) = self.active_buffer_mut() {
            if up {
                buffer.scroll = buffer.scroll.saturating_add(lines);
            } else {
                buffer.scroll = buffer.scroll.saturating_sub(lines);
            }
        }
    }

    // -------------------------------------------------------------- menus ---

    pub fn open_menu(&mut self) {
        match self.focus {
            Focus::Channels => self.open_sidebar_menu(),
            Focus::Messages => self.open_message_menu(),
            Focus::Members => self.open_member_menu(),
        }
    }

    pub fn activate_sidebar(&mut self) {
        let entries = self.sidebar_entries();
        match entries.get(self.sidebar_nav).copied() {
            Some(SideEntry::Buffer(bid)) => self.set_active(bid),
            Some(SideEntry::Server(pos)) => {
                if let Some(sid) = self.servers.get(pos).map(|s| s.id) {
                    let console = self.ensure_server_buffer(sid);
                    self.set_active(console);
                }
            }
            None => {}
        }
    }

    pub fn open_query_for_selected_member(&mut self) {
        let Some(bid) = self.active else { return };
        let order = self.member_order(bid);
        if order.is_empty() {
            return;
        }
        self.member_nav = self.member_nav.min(order.len() - 1);
        let member_idx = order[self.member_nav];
        let Some(buffer) = self.buffers.get(bid) else {
            return;
        };
        let Some(member) = buffer.members().get(member_idx) else {
            return;
        };
        let sid = buffer.server;
        let nick = member.nick.clone();
        let query = self.ensure_query(sid, &nick);
        self.set_active(query);
    }

    pub fn open_sidebar_menu(&mut self) {
        let entries = self.sidebar_entries();
        let Some(entry) = entries.get(self.sidebar_nav).copied() else {
            return;
        };
        match entry {
            SideEntry::Server(pos) => self.open_server_menu(pos),
            SideEntry::Buffer(idx) => self.open_buffer_menu(idx),
        }
    }

    fn open_server_menu(&mut self, pos: usize) {
        let Some(server) = self.servers.get(pos) else {
            return;
        };
        let title = server.display().to_string();
        let sid = server.id;
        let mut menu = Menu::new(title.clone());
        let tls = if server.tls { "TLS" } else { "plaintext" };
        menu.info
            .push(format!("{}:{} ({tls})", server.host, server.port));
        menu.info.push(format!(
            "status  {}  ·  nick {}",
            server.status(),
            server.nick
        ));
        if let Some(network) = &server.network {
            menu.info.push(format!("network {network}"));
        }
        let channels = self
            .buffers
            .iter()
            .filter(|b| b.server == sid && b.is_channel())
            .count();
        menu.info.push(format!("{channels} channel buffer(s)"));
        if let Some(error) = &server.last_error {
            menu.info.push(format!("last error: {error}"));
        }

        if self.server(sid).map(|s| s.local).unwrap_or(false) {
            menu.info
                .push("This is the local welcome buffer.".to_string());
            menu.actions.push(MenuAction::Fill {
                buffer: None,
                text: ":connect ".to_string(),
                label: "Connect to a server…".to_string(),
                insert: false,
            });
            menu.actions.push(MenuAction::Help);
            menu.actions.push(MenuAction::Quit);
            menu.actions.push(MenuAction::CloseMenu);
        } else {
            if let Some(console) = self.server_buffer(sid) {
                menu.actions.push(MenuAction::Activate(console));
            }
            menu.actions.push(MenuAction::Fill {
                buffer: self.server_buffer(sid),
                text: ":join ".to_string(),
                label: "Join a channel…".to_string(),
                insert: false,
            });
            menu.actions.push(MenuAction::Fill {
                buffer: self.server_buffer(sid),
                text: ":nick ".to_string(),
                label: "Change nickname…".to_string(),
                insert: false,
            });
            if server.demo {
                menu.info
                    .push("demo sandbox · outbound messages are not sent".to_string());
            }
            if !server.demo {
                if server.connected {
                    menu.actions.push(MenuAction::Disconnect(sid));
                } else {
                    menu.actions.push(MenuAction::Reconnect(sid));
                }
            }
            menu.actions.push(MenuAction::Copy {
                text: format!("{}:{}", server.host, server.port),
                label: "Copy server address".to_string(),
            });
            if server.demo {
                menu.actions.push(MenuAction::Help);
            }
            menu.actions.push(MenuAction::Quit);
        }
        self.menu = Some(menu);
    }

    fn open_buffer_menu(&mut self, bid: usize) {
        let Some(buffer) = self.buffers.get(bid) else {
            return;
        };
        let sid = buffer.server;
        let title = buffer.name().to_string();
        let mut menu = Menu::new(title.clone());
        match &buffer.kind {
            BufferKind::Channel {
                topic,
                topic_by,
                modes,
                members,
                joined,
                name,
            } => {
                let status = if *joined { "joined" } else { "not joined" };
                menu.info.push(format!(
                    "{name} on {} · {status}",
                    self.server(sid).map(|s| s.display()).unwrap_or("?")
                ));
                if topic.is_empty() {
                    menu.info.push("topic   (none)".to_string());
                } else {
                    menu.info.push(format!("topic   {topic}"));
                }
                if let Some(by) = topic_by {
                    menu.info.push(format!("set by  {by}"));
                }
                if modes.is_empty() {
                    menu.info.push(format!("members {}", members.len()));
                } else {
                    menu.info
                        .push(format!("members {} · modes +{}", members.len(), modes));
                }
                menu.actions.push(MenuAction::Activate(bid));
                if !topic.is_empty() {
                    menu.actions.push(MenuAction::Copy {
                        text: topic.clone(),
                        label: "Copy topic".to_string(),
                    });
                }
                if *joined {
                    menu.actions.push(MenuAction::Fill {
                        buffer: Some(bid),
                        text: ":topic ".to_string(),
                        label: "Change topic…".to_string(),
                        insert: false,
                    });
                    menu.actions.push(MenuAction::Part {
                        server: sid,
                        channel: name.clone(),
                    });
                }
                menu.actions.push(MenuAction::ClearBuffer(bid));
                menu.actions.push(MenuAction::CloseBuffer(bid));
            }
            BufferKind::Query { member } => {
                menu.info.push(format!(
                    "private conversation with {} on {}",
                    member.nick,
                    self.server(sid).map(|s| s.display()).unwrap_or("?")
                ));
                if let Some(away) = &member.away {
                    if away.is_empty() {
                        menu.info.push("status   away".to_string());
                    } else {
                        menu.info
                            .push(format!("away     {}", util::sanitize_line(away)));
                    }
                }
                if let Some(account) = &member.account {
                    menu.info
                        .push(format!("account  {}", util::sanitize_line(account)));
                }
                if let Some(host) = &member.host {
                    menu.info
                        .push(format!("host     {}", util::sanitize_line(host)));
                }
                menu.actions.push(MenuAction::Activate(bid));
                menu.actions.push(MenuAction::WhoIs {
                    server: sid,
                    nick: member.nick.clone(),
                });
                menu.actions.push(MenuAction::Fill {
                    buffer: Some(bid),
                    text: String::new(),
                    label: "Write a message…".to_string(),
                    insert: true,
                });
                menu.actions.push(MenuAction::Copy {
                    text: member.nick.clone(),
                    label: "Copy nickname".to_string(),
                });
                menu.actions.push(MenuAction::ClearBuffer(bid));
                menu.actions.push(MenuAction::CloseBuffer(bid));
            }
            BufferKind::Server { title } => {
                menu.info.push("server console".to_string());
                menu.info.push(title.to_string());
                menu.actions.push(MenuAction::Activate(bid));
                menu.actions.push(MenuAction::ClearBuffer(bid));
            }
        }
        self.menu = Some(menu);
    }

    fn open_member_menu(&mut self) {
        let Some(bid) = self.active else { return };
        let is_query = self
            .buffers
            .get(bid)
            .map(|buffer| {
                matches!(
                    &buffer.kind,
                    BufferKind::Query { .. } | BufferKind::Server { .. }
                )
            })
            .unwrap_or(false);
        if is_query {
            self.open_buffer_menu(bid);
            return;
        }
        let order = self.member_order(bid);
        if order.is_empty() {
            return;
        }
        self.member_nav = self.member_nav.min(order.len() - 1);
        let member_idx = order[self.member_nav];
        let Some(buffer) = self.buffers.get(bid) else {
            return;
        };
        let Some(member) = buffer.members().get(member_idx).cloned() else {
            return;
        };
        let sid = buffer.server;
        let channel = buffer.name().to_string();
        let server = self.server(sid);
        let mut menu = Menu::new(member.nick.clone());
        let mut roles = Vec::new();
        for prefix in member.prefixes.chars() {
            if let Some(mode) = server.and_then(|s| s.mode_for_prefix(prefix)) {
                let role = match mode {
                    'q' => "owner",
                    'a' => "admin",
                    'o' => "operator",
                    'h' => "halfop",
                    'v' => "voiced",
                    _ => "member",
                };
                roles.push(role);
            }
        }
        if roles.is_empty() {
            roles.push("member");
        }
        menu.info
            .push(format!("{} in {channel}", roles.join(" · ")));
        if let Some(host) = &member.host {
            menu.info
                .push(format!("host     {}", util::sanitize_line(host)));
        }
        if let Some(account) = &member.account {
            menu.info
                .push(format!("account  {}", util::sanitize_line(account)));
        }
        if let Some(away) = &member.away {
            if away.is_empty() {
                menu.info.push("status   away".to_string());
            } else {
                menu.info
                    .push(format!("away     {}", util::sanitize_line(away)));
            }
        }
        if let Some(realname) = &member.realname {
            menu.info
                .push(format!("realname {}", util::sanitize_line(realname)));
        }

        let is_me = util::fold(&member.nick) == util::fold(&self.nick_of(sid));
        let joined = matches!(&buffer.kind, BufferKind::Channel { joined: true, .. });
        menu.actions.push(MenuAction::Copy {
            text: member.nick.clone(),
            label: "Copy nickname".to_string(),
        });
        if is_me {
            menu.actions.push(MenuAction::CloseMenu);
            self.menu = Some(menu);
            return;
        }
        menu.actions.push(MenuAction::Query {
            server: sid,
            nick: member.nick.clone(),
        });
        menu.actions.push(MenuAction::WhoIs {
            server: sid,
            nick: member.nick.clone(),
        });
        menu.actions.push(MenuAction::Fill {
            buffer: Some(bid),
            text: format!("{}: ", member.nick),
            label: "Mention / reply…".to_string(),
            insert: true,
        });
        let has = |mode: char| {
            server
                .and_then(|s| s.prefix_for_mode(mode))
                .map(|symbol| member.prefixes.contains(symbol))
                .unwrap_or(false)
        };
        if joined {
            if has('o') {
                menu.actions.push(MenuAction::Mode {
                    server: sid,
                    target: channel.clone(),
                    modes: format!("-o {}", member.nick),
                    label: "Remove operator".to_string(),
                });
            } else {
                menu.actions.push(MenuAction::Mode {
                    server: sid,
                    target: channel.clone(),
                    modes: format!("+o {}", member.nick),
                    label: "Give operator".to_string(),
                });
            }
            if has('v') {
                menu.actions.push(MenuAction::Mode {
                    server: sid,
                    target: channel.clone(),
                    modes: format!("-v {}", member.nick),
                    label: "Remove voice".to_string(),
                });
            } else {
                menu.actions.push(MenuAction::Mode {
                    server: sid,
                    target: channel.clone(),
                    modes: format!("+v {}", member.nick),
                    label: "Give voice".to_string(),
                });
            }
            menu.actions.push(MenuAction::Kick {
                server: sid,
                channel: channel.clone(),
                nick: member.nick.clone(),
            });
        }
        menu.actions.push(MenuAction::CloseMenu);
        self.menu = Some(menu);
    }

    fn open_message_menu(&mut self) {
        let Some(bid) = self.active else { return };
        let Some((start, end)) = self.selection_bounds() else {
            self.toast("no message selected");
            return;
        };
        let Some(buffer) = self.buffers.get(bid) else {
            return;
        };
        let in_query = matches!(&buffer.kind, BufferKind::Query { .. });
        let sid = buffer.server;
        let channel = buffer.name().to_string();
        let selected: Vec<Message> = buffer.messages[start..=end].to_vec();
        let count = selected.len();
        let mut menu = if count == 1 {
            Menu::new("message")
        } else {
            Menu::new(format!("{count} messages"))
        };
        if count == 1 {
            let msg = &selected[0];
            let who = msg.nick.clone().unwrap_or_else(|| "server".to_string());
            menu.info.push(format!(
                "[{}] {who} · {}",
                util::clock(msg.ts),
                msg.kind.label()
            ));
            menu.info.push(format!("in {channel}"));
            menu.info.push(String::new());
            menu.info.push(msg.text.clone());
        } else {
            menu.info.push(format!("{count} messages in {channel}"));
            menu.info
                .push(format!("from [{}]", util::clock(selected[0].ts)));
            menu.info
                .push(format!("to   [{}]", util::clock(selected[count - 1].ts)));
            menu.info.push(String::new());
            menu.info.push(selected[0].yank());
            menu.info.push("…".to_string());
            menu.info.push(selected[count - 1].yank());
        }

        let plain = selected
            .iter()
            .map(Message::yank)
            .collect::<Vec<_>>()
            .join("\n");
        let stamped = selected
            .iter()
            .map(Message::yank_stamped)
            .collect::<Vec<_>>()
            .join("\n");
        menu.actions.push(MenuAction::Copy {
            text: plain.clone(),
            label: if count == 1 {
                "Copy message".to_string()
            } else {
                format!("Copy {count} messages")
            },
        });
        menu.actions.push(MenuAction::Copy {
            text: stamped,
            label: "Copy with timestamps".to_string(),
        });

        if count == 1 {
            let msg = &selected[0];
            if let Some(nick) = msg.nick.clone() {
                if !msg.own {
                    menu.actions.push(MenuAction::Fill {
                        buffer: Some(bid),
                        text: if in_query {
                            String::new()
                        } else {
                            format!("{nick}: ")
                        },
                        label: "Reply / mention".to_string(),
                        insert: true,
                    });
                    if let Some(query) = self.find_query(sid, &nick) {
                        menu.actions.push(MenuAction::Activate(query));
                    } else {
                        menu.actions.push(MenuAction::Query {
                            server: sid,
                            nick: nick.clone(),
                        });
                    }
                    menu.actions.push(MenuAction::WhoIs { server: sid, nick });
                }
            }
        }
        menu.actions.push(MenuAction::CloseMenu);
        self.menu = Some(menu);
    }

    pub fn open_help(&mut self) {
        self.menu = Some(help_menu());
    }

    pub fn open_quit_menu(&mut self) {
        let count = self
            .servers
            .iter()
            .filter(|s| !s.local && s.connected)
            .count();
        let mut menu = Menu::new("quit");
        menu.info.push(if count == 0 {
            "No active connections.".to_string()
        } else {
            format!("Disconnect from {count} server(s) and exit rirc?")
        });
        menu.info.push(String::new());
        menu.actions.push(MenuAction::Quit);
        menu.actions.push(MenuAction::CloseMenu);
        self.menu = Some(menu);
    }

    pub fn run_action(&mut self, action: MenuAction) {
        match action {
            MenuAction::Activate(bid) => self.set_active(bid),
            MenuAction::CloseBuffer(bid) => self.close_buffer(bid),
            MenuAction::ClearBuffer(bid) => {
                if let Some(buffer) = self.buffers.get_mut(bid) {
                    buffer.messages.clear();
                    buffer.selected = 0;
                    buffer.scroll = 0;
                }
            }
            MenuAction::Copy { text, label } => self.copy(text, &label),
            MenuAction::Join { server, channel } => self.join_channel(server, channel),
            MenuAction::Part { server, channel } => {
                self.send(server, format!("PART {channel}"));
                if let Some(bid) = self.find_channel(server, &channel) {
                    if let BufferKind::Channel { joined, .. } = &mut self.buffers[bid].kind {
                        *joined = false;
                    }
                }
            }
            MenuAction::WhoIs { server, nick } => {
                if let Some(s) = self.server_mut(server) {
                    s.whois = Some(Whois::new(nick.clone()));
                }
                self.send(server, format!("WHOIS {nick}"));
                self.toast(format!("whois {nick}…"));
            }
            MenuAction::Query { server, nick } => {
                let bid = self.ensure_query(server, &nick);
                self.set_active(bid);
            }
            MenuAction::Fill {
                buffer,
                text,
                label: _,
                insert,
            } => {
                if let Some(target) = buffer {
                    self.set_active(target);
                }
                self.fill_input(&text, insert);
            }
            MenuAction::Mode {
                server,
                target,
                modes,
                label,
            } => {
                self.send(server, format!("MODE {target} {modes}"));
                self.toast(label);
            }
            MenuAction::Kick {
                server,
                channel,
                nick,
            } => {
                self.send(server, format!("KICK {channel} {nick}"));
                self.toast(format!("kicked {nick}"));
            }
            MenuAction::Reconnect(sid) => self.reconnect(sid),
            MenuAction::Disconnect(sid) => self.disconnect_server(sid),
            MenuAction::Quit => self.quit_all(""),
            MenuAction::Help => self.menu = Some(help_menu()),
            MenuAction::CloseMenu => {}
        }
    }

    pub fn copy(&mut self, text: String, label: &str) {
        if text.is_empty() {
            self.toast("nothing to copy");
            return;
        }
        let bytes = text.len();
        crate::clipboard::copy(&text);
        self.toast(format!("Copied {label} ({bytes} bytes)"));
    }

    pub fn yank_current(&mut self) {
        let Some(bid) = self.active else { return };
        let Some(buffer) = self.buffers.get(bid) else {
            return;
        };
        if buffer.messages.is_empty() {
            return;
        }
        let idx = buffer.selected.min(buffer.messages.len() - 1);
        let text = buffer.messages[idx].yank();
        self.copy(text, "message");
    }

    pub fn yank_selection(&mut self) {
        let Some((start, end)) = self.selection_bounds() else {
            self.toast("nothing selected");
            return;
        };
        let Some(buffer) = self.active_buffer() else {
            return;
        };
        let multi = end > start;
        let lines: Vec<String> = (start..=end)
            .filter_map(|i| buffer.messages.get(i))
            .map(|m| if multi { m.yank_stamped() } else { m.yank() })
            .collect();
        let count = lines.len();
        let text = lines.join("\n");
        self.copy(
            text,
            &format!(
                "{count} {}",
                if count == 1 { "message" } else { "messages" }
            ),
        );
        self.mode = Mode::Normal;
        self.focus = Focus::Messages;
    }

    // --------------------------------------------------------------- mouse ---

    pub fn on_mouse(&mut self, event: MouseEvent) {
        if self.menu.is_some() {
            return;
        }
        if self.list.is_some() {
            if let Some(list) = self.list.as_mut() {
                match event.kind {
                    MouseEventKind::ScrollDown => list.move_selection(3),
                    MouseEventKind::ScrollUp => list.move_selection(-3),
                    _ => {}
                }
            }
            return;
        }
        match event.kind {
            MouseEventKind::ScrollDown => self.wheel(3, false),
            MouseEventKind::ScrollUp => self.wheel(3, true),
            MouseEventKind::Down(MouseButton::Left) => self.click(event.column, event.row),
            _ => {}
        }
    }

    fn wheel(&mut self, step: usize, up: bool) {
        if self.mode.wants_input() {
            self.scroll_view(step, up);
            return;
        }
        let delta = if up { -(step as isize) } else { step as isize };
        match self.focus {
            Focus::Channels => {
                let len = self.sidebar_entries().len();
                self.sidebar_nav = (self.sidebar_nav as isize + delta)
                    .clamp(0, len.saturating_sub(1) as isize)
                    as usize;
            }
            Focus::Members => {
                let len = self
                    .active
                    .map(|bid| self.member_order(bid).len())
                    .unwrap_or(0);
                self.member_nav = (self.member_nav as isize + delta)
                    .clamp(0, len.saturating_sub(1) as isize)
                    as usize;
            }
            Focus::Messages => self.select_msg(delta),
        }
    }

    fn click(&mut self, x: u16, y: u16) {
        if self.mode == Mode::Command {
            self.input.clear();
        }
        let in_rect = |rect: Rect| {
            x >= rect.x
                && x < rect.x.saturating_add(rect.width)
                && y >= rect.y
                && y < rect.y.saturating_add(rect.height)
        };
        if self.sidebar_visible() && in_rect(self.hit.sidebar_rect) {
            if let Some((_, idx)) = self.hit.sidebar.iter().find(|(row, _)| *row == y) {
                self.sidebar_nav = *idx;
                self.mode = Mode::Normal;
                self.activate_sidebar();
            }
            return;
        }
        if self.members_visible() && in_rect(self.hit.members_rect) {
            if let Some((_, idx)) = self.hit.members.iter().find(|(row, _)| *row == y) {
                self.mode = Mode::Normal;
                self.focus = Focus::Members;
                self.member_nav = *idx;
            }
            return;
        }
        if in_rect(self.hit.messages_rect) {
            let found = self
                .hit
                .messages
                .iter()
                .find(|(row, _)| *row == y)
                .map(|(_, idx)| *idx);
            if let Some(idx) = found {
                self.mode = Mode::Normal;
                self.focus = Focus::Messages;
                if let Some(buffer) = self.active_buffer_mut() {
                    buffer.selected = idx;
                    buffer.scroll = 0;
                }
            }
        }
    }

    pub fn on_paste(&mut self, text: &str) {
        if self.mode.wants_input() {
            let cleaned = text.replace(['\r', '\n'], " ");
            self.input.insert_str(&cleaned);
        }
    }
}

fn is_numeric(command: &str) -> bool {
    command.len() == 3 && command.chars().all(|c| c.is_ascii_digit())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use std::sync::mpsc::channel;

    fn demo_app() -> (App, tokio::runtime::Runtime) {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let (tx, _rx) = channel();
        let cfg = Config {
            demo: true,
            ..Default::default()
        };
        let app = App::new(cfg, Some(rt.handle().clone()), tx);
        (app, rt)
    }

    fn press(app: &mut App, code: KeyCode) {
        app.handle_key(KeyEvent::new(code, KeyModifiers::NONE));
    }

    fn line(app: &mut App, raw: &str) {
        let sid = app.active_buffer().unwrap().server;
        let token = app.servers[0].token;
        app.on_net(NetEvent::Line {
            server: sid,
            token,
            line: raw.to_string(),
        });
    }

    #[test]
    fn modes_and_menus_round_trip() {
        let (mut app, _rt) = demo_app();
        press(&mut app, KeyCode::Char('v'));
        assert!(matches!(app.mode, Mode::Visual { .. }));
        press(&mut app, KeyCode::Esc);
        assert_eq!(app.mode, Mode::Normal);

        press(&mut app, KeyCode::Enter);
        assert!(app.menu.is_some());
        press(&mut app, KeyCode::Esc);
        assert!(app.menu.is_none());

        press(&mut app, KeyCode::Char('i'));
        assert_eq!(app.mode, Mode::Insert);
        for c in "hello".chars() {
            press(&mut app, KeyCode::Char(c));
        }
        assert_eq!(app.input.text(), "hello");
        press(&mut app, KeyCode::Esc);
        assert_eq!(app.mode, Mode::Normal);
        assert_eq!(app.input.text(), "hello");

        press(&mut app, KeyCode::Char(':'));
        assert_eq!(app.mode, Mode::Command);
        press(&mut app, KeyCode::Char('q'));
        press(&mut app, KeyCode::Enter);
        assert!(app.quit);
    }

    #[test]
    fn incoming_lines_update_buffers() {
        let (mut app, _rt) = demo_app();
        let bid = app.active.unwrap();

        line(&mut app, ":zed!u@h JOIN #rust");
        assert!(app.buffers[bid].members().iter().any(|m| m.nick == "zed"));

        line(&mut app, ":zed!u@h PRIVMSG #rust :vimmer: are you there?");
        let last = app.buffers[bid].messages.last().unwrap();
        assert!(last.highlight);
        assert!(last.text.contains("are you there"));

        line(&mut app, ":zed!u@h NICK zed2");
        assert!(app.buffers[bid].members().iter().any(|m| m.nick == "zed2"));
        assert!(!app.buffers[bid].members().iter().any(|m| m.nick == "zed"));

        line(&mut app, ":zed2!u@h PART #rust");
        assert!(!app.buffers[bid].members().iter().any(|m| m.nick == "zed2"));

        line(&mut app, ":carol!u@h MODE #rust +o bob");
        let bob = app.buffers[bid]
            .members()
            .iter()
            .find(|m| m.nick == "bob")
            .unwrap();
        assert!(bob.prefixes.contains('@'));

        line(&mut app, ":srv 332 vimmer #rust :A brand new topic");
        assert_eq!(app.buffers[bid].topic(), Some("A brand new topic"));

        line(
            &mut app,
            ":srv 005 vimmer PREFIX=(qaohv)~&@%+ CHANTYPES=# NETWORK=TestNet :are supported",
        );
        assert_eq!(app.servers[0].network.as_deref(), Some("TestNet"));

        line(&mut app, ":srv 353 vimmer = #rust :+newbie @carol");
        let newbie = app.buffers[bid]
            .members()
            .iter()
            .find(|m| m.nick == "newbie")
            .expect("names list member");
        assert!(newbie.prefixes.contains('+'));
        let carol = app.buffers[bid]
            .members()
            .iter()
            .find(|m| m.nick == "carol")
            .unwrap();
        assert!(carol.prefixes.contains('@'));
    }

    #[test]
    fn commands_create_buffers_and_echo() {
        let (mut app, _rt) = demo_app();
        app.run_command("join #newchan");
        let sid = app.servers[0].id;
        assert!(app.find_channel(sid, "#newchan").is_some());
        app.run_command("me waves at the crowd");
        let last = app.buffers[app.active.unwrap()].messages.last().unwrap();
        assert_eq!(last.kind, MsgKind::Action);
        app.run_command("nick someone");
        assert_eq!(app.servers[0].nick, "someone");
    }

    #[test]
    fn list_browser_opens_in_demo_mode() {
        let (mut app, _rt) = demo_app();
        app.run_command("list");
        let list = app.list.as_ref().expect("channel list");
        assert!(!list.entries.is_empty());
        assert!(!list.loading);
        assert_eq!(list.server, app.servers[0].id);
    }

    #[test]
    fn list_numerics_fill_the_overlay() {
        let (mut app, _rt) = demo_app();
        let sid = app.servers[0].id;
        app.list = Some(ChannelList::new(sid, "TestNet", ""));

        line(&mut app, ":srv 321 vimmer Channel :Users Name");
        line(&mut app, ":srv 322 vimmer #one 42 :First channel");
        line(&mut app, ":srv 322 vimmer #two 7 :");
        let list = app.list.as_ref().unwrap();
        assert_eq!(list.entries.len(), 2);
        assert_eq!(list.entries[0].channel, "#one");
        assert_eq!(list.entries[0].users, 42);
        assert_eq!(list.entries[1].topic, "");
        assert!(list.loading);

        line(&mut app, ":srv 323 vimmer :End of /LIST");
        assert!(!app.list.as_ref().unwrap().loading);

        let list = app.list.as_mut().unwrap();
        list.filter = "two".to_string();
        list.normalize_selection();
        assert_eq!(list.selected, 1);
    }

    #[test]
    fn list_round_trips_over_a_socket() {
        use std::io::{BufRead, BufReader, Write};
        use std::net::TcpListener;
        use std::time::{Duration, Instant};

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let mock = std::thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            socket
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut reader = BufReader::new(socket.try_clone().unwrap());
            let mut line = String::new();
            loop {
                line.clear();
                match reader.read_line(&mut line) {
                    Ok(0) => return,
                    Ok(_) => {}
                    Err(_) => return,
                }
                let line = line.trim_end();
                if line.starts_with("PING") {
                    let _ = writeln!(socket, "PONG :x");
                }
                if line.starts_with("LIST") {
                    break;
                }
            }
            for response in [
                ":srv 321 tester Channel :Users Name",
                ":srv 322 tester #rust 42 :The Rust language",
                ":srv 322 tester #vim 7 :Editor",
                ":srv 323 tester :End of /LIST",
            ] {
                let _ = writeln!(socket, "{response}");
            }
            let _ = socket.flush();
        });

        let rt = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .unwrap();
        let (tx, rx) = channel();
        let mut app = App::new(Config::default(), Some(rt.handle().clone()), tx);
        app.start_server(ServerConfig {
            host: "127.0.0.1".to_string(),
            port,
            tls: false,
            nick: "tester".to_string(),
            sasl: None,
            pass: None,
            channels: Vec::new(),
        });

        let deadline = Instant::now() + Duration::from_secs(5);
        let mut connected = false;
        while Instant::now() < deadline && !connected {
            match rx.recv_timeout(Duration::from_millis(200)) {
                Ok(event) => {
                    if matches!(event, NetEvent::Connected { .. }) {
                        connected = true;
                    }
                    app.on_net(event);
                }
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                Err(_) => break,
            }
        }
        assert!(connected, "did not connect to the mock server");

        app.run_command("list");
        assert!(app.list.is_some());

        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            let finished = app.list.as_ref().map(|list| !list.loading).unwrap_or(false);
            if finished {
                break;
            }
            match rx.recv_timeout(Duration::from_millis(200)) {
                Ok(event) => app.on_net(event),
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                Err(_) => break,
            }
        }

        let list = app.list.as_ref().expect("channel list");
        assert!(!list.loading, "LIST did not finish");
        assert_eq!(list.entries.len(), 2);
        assert_eq!(list.entries[0].channel, "#rust");
        assert_eq!(list.entries[0].users, 42);
        assert_eq!(list.entries[1].topic, "Editor");
        let _ = mock.join();
    }

    #[test]
    fn list_keys_filter_and_join() {
        let (mut app, _rt) = demo_app();
        app.run_command("list");

        press(&mut app, KeyCode::Char('f'));
        for c in "rust".chars() {
            press(&mut app, KeyCode::Char(c));
        }
        let list = app.list.as_ref().unwrap();
        assert!(list.filtering);
        assert_eq!(list.filter, "rust");
        assert!(list.filtered().len() >= 2);

        press(&mut app, KeyCode::Esc);
        assert!(!app.list.as_ref().unwrap().filtering);
        press(&mut app, KeyCode::Esc);
        assert!(app.list.as_ref().unwrap().filter.is_empty());
        press(&mut app, KeyCode::Esc);
        assert!(app.list.is_none());

        app.run_command("list");
        press(&mut app, KeyCode::Char('j'));
        let channel = app
            .list
            .as_ref()
            .and_then(|list| list.selected_entry())
            .map(|entry| entry.channel.clone())
            .expect("selected channel");
        press(&mut app, KeyCode::Char('l'));
        assert!(app.list.is_none());
        let sid = app.servers[0].id;
        assert!(app.find_channel(sid, &channel).is_some());
    }

    #[test]
    #[ignore = "requires outbound internet access; run sparingly (networks may ban automated probes)"]
    fn registers_with_libera() {
        use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

        let rt = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .unwrap();
        let (tx, rx) = channel();
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs()
            % 100_000;
        let nick = format!("rircprobe{suffix}");
        let cfg = Config {
            nick: nick.clone(),
            ..Default::default()
        };
        let mut app = App::new(cfg, Some(rt.handle().clone()), tx);
        app.start_server(ServerConfig {
            host: "irc.libera.chat".to_string(),
            port: 6697,
            tls: true,
            nick,
            sasl: None,
            pass: None,
            channels: Vec::new(),
        });

        let deadline = Instant::now() + Duration::from_secs(30);
        while Instant::now() < deadline {
            let left = deadline.saturating_duration_since(Instant::now());
            match rx.recv_timeout(left.min(Duration::from_millis(500))) {
                Ok(event) => app.on_net(event),
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                Err(_) => break,
            }
            if app
                .servers
                .iter()
                .any(|server| !server.local && server.registered)
            {
                break;
            }
        }
        let server = app
            .servers
            .iter()
            .find(|server| !server.local)
            .expect("real server");
        assert!(
            server.registered,
            "did not register; last error: {:?}",
            server.last_error
        );
        assert!(
            server.caps_done,
            "CAP negotiation did not complete (requested={})",
            server.cap_requested
        );

        for server in &app.servers {
            if let Some(tx) = &server.tx {
                let _ = tx.send("QUIT :registration test done".to_string());
            }
        }
    }
}
