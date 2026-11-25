//! Context menus (also used for the help and quit overlays).

#[derive(Clone, Debug)]
pub enum MenuAction {
    Activate(usize),
    CloseBuffer(usize),
    ClearBuffer(usize),
    Copy {
        text: String,
        label: String,
    },

    Part {
        server: usize,
        channel: String,
    },
    Join {
        server: usize,
        channel: String,
    },
    WhoIs {
        server: usize,
        nick: String,
    },
    Query {
        server: usize,
        nick: String,
    },
    /// Prefill the input line; `insert` selects text mode, otherwise command mode.
    Fill {
        buffer: Option<usize>,
        text: String,
        label: String,
        insert: bool,
    },
    Mode {
        server: usize,
        target: String,
        modes: String,
        label: String,
    },
    Kick {
        server: usize,
        channel: String,
        nick: String,
    },
    Reconnect(usize),
    Disconnect(usize),
    PluginReload,
    PluginLogs,
    Quit,
    Help,
    CloseMenu,
}

impl MenuAction {
    pub fn label(&self) -> String {
        match self {
            MenuAction::Activate(_) => "Open buffer".to_string(),
            MenuAction::CloseBuffer(_) => "Close buffer".to_string(),
            MenuAction::ClearBuffer(_) => "Clear messages".to_string(),
            MenuAction::Copy { label, .. } => label.clone(),
            MenuAction::Part { channel, .. } => format!("Leave {channel}"),
            MenuAction::Join { channel, .. } => format!("Join {channel}"),
            MenuAction::WhoIs { nick, .. } => format!("Whois {nick}"),
            MenuAction::Query { nick, .. } => format!("Message {nick}"),
            MenuAction::Fill { label, .. } => label.clone(),
            MenuAction::Mode { label, .. } => label.clone(),
            MenuAction::Kick { nick, .. } => format!("Kick {nick}"),
            MenuAction::Reconnect(_) => "Reconnect".to_string(),
            MenuAction::Disconnect(_) => "Disconnect".to_string(),
            MenuAction::PluginReload => "Reload plugins".to_string(),
            MenuAction::PluginLogs => "Plugin logs".to_string(),
            MenuAction::Quit => "Quit rirc".to_string(),
            MenuAction::Help => "Keyboard help".to_string(),
            MenuAction::CloseMenu => "Close menu".to_string(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct Menu {
    pub title: String,
    pub info: Vec<String>,
    pub actions: Vec<MenuAction>,
    pub selected: usize,
}

impl Menu {
    pub fn new(title: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            info: Vec::new(),
            actions: Vec::new(),
            selected: 0,
        }
    }

    pub fn info_text(&self) -> String {
        self.info.join("\n")
    }
}

pub const HELP: &str = "\
MODES      Esc normal · i/a text · : command · / slash command
NORMAL     j/k move · Tab panel · Enter menu · l open · g/G ends
           v visual · y yank · Ctrl-n/p buffer · Ctrl-d/u page
           ? help · Q quit · :q quit · :connect host
VISUAL     j/k extend · o swap ends · y yank · Enter menu
TEXT       Enter send · Tab complete nick · ↑/↓ history · Esc normal
LIST       /list · j/k move · Enter menu · l join · f filter · r refresh
PLUGINS    drop .py in plugins/ · /plugins · /plugins reload · /plugins logs
PANELS     Tab cycles messages → buffers → members; click works too
MOUSE      click a buffer/member/message · wheel moves the focused list
CLIPBOARD  yanks go to the system clipboard and OSC 52
COMMANDS   /join /part /me /msg /nick /topic /mode /whois /away /list
           /invite /kick /op /deop /voice /devoice /clear /close
           /reconnect /disconnect /raw /help /plugins
TIPS       Enter on a channel, member or message opens its menu.
           Select several messages with v then y to copy the block.";

pub fn help_menu() -> Menu {
    let mut menu = Menu::new("keyboard help");
    for line in HELP.lines() {
        menu.info.push(line.to_string());
    }
    menu
}
