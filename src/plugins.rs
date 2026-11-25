//! Out-of-process Python plugin host.
//!
//! Plugins are trusted. The process boundary exists so a buggy plugin cannot
//! take down the terminal UI or corrupt client state, and so release binaries
//! do not need to link against a specific Python installation.

use std::collections::{HashMap, VecDeque};
use std::env;
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Receiver, SyncSender, TryRecvError, TrySendError};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use chrono::Local;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::util;

const HOST_SCRIPT: &str = include_str!("plugin_host.py");
const MAX_LOGS: usize = 500;
const HOST_QUEUE: usize = 512;

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct PluginInfo {
    pub name: String,
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub path: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct PluginCommand {
    pub name: String,
    pub plugin: String,
    #[serde(default)]
    pub help: String,
    #[serde(default)]
    pub usage: String,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum HostMessage {
    Ready {
        #[serde(default)]
        python: String,
        #[serde(default)]
        plugins: Vec<PluginInfo>,
        #[serde(default)]
        commands: Vec<PluginCommand>,
    },
    Command {
        plugin: String,
        name: String,
        #[serde(default)]
        help: String,
        #[serde(default)]
        usage: String,
    },
    Action {
        #[serde(default)]
        plugin: String,
        action: String,
        #[serde(default)]
        data: Value,
    },
    Notify {
        #[serde(default)]
        plugin: String,
        text: String,
        #[serde(default)]
        level: String,
    },
    Log {
        #[serde(default)]
        plugin: Option<String>,
        text: String,
    },
    Error {
        #[serde(default)]
        plugin: Option<String>,
        text: String,
    },
}

enum HostRx {
    Message(HostMessage),
    Stderr(String),
    Protocol(String),
    Eof,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PluginStatus {
    Disabled,
    Idle,
    Starting,
    Running,
    Error(String),
}

pub enum PluginEvent {
    Ready,
    Action {
        plugin: String,
        action: String,
        data: Value,
    },
    Notify {
        text: String,
        level: String,
    },
    Exited(String),
}

#[derive(Clone, Debug)]
struct PythonCmd {
    program: String,
    args: Vec<String>,
}

impl PythonCmd {
    fn command(&self) -> Command {
        let mut command = Command::new(&self.program);
        command.args(&self.args);
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x0800_0000);
        }
        command
    }

    fn display(&self) -> String {
        if self.args.is_empty() {
            self.program.clone()
        } else {
            format!("{} {}", self.program, self.args.join(" "))
        }
    }
}

pub struct PluginManager {
    enabled: bool,
    pub dirs: Vec<PathBuf>,
    pub data_dir: PathBuf,
    pub status: PluginStatus,
    pub python_version: Option<String>,
    pub plugins: Vec<PluginInfo>,
    pub commands: HashMap<String, PluginCommand>,
    pub logs: VecDeque<String>,
    rx: Option<Receiver<HostRx>>,
    stdin_tx: Option<SyncSender<String>>,
    child: Option<Child>,
    host_path: PathBuf,
    last_drop_log: Option<Instant>,
}

impl PluginManager {
    pub fn new(enabled: bool, extra_dirs: &[String]) -> Self {
        let dirs = plugin_dirs(extra_dirs);
        if enabled {
            if let Some(config) = config_dir() {
                let _ = fs::create_dir_all(config.join("plugins"));
            }
        }
        Self::with_paths(enabled, dirs, plugin_data_dir())
    }

    pub fn with_paths(enabled: bool, dirs: Vec<PathBuf>, data_dir: PathBuf) -> Self {
        let host_path = env::temp_dir().join(format!(
            "rirc-plugin-host-{}-{}.py",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|value| value.as_millis())
                .unwrap_or(0)
        ));
        let mut manager = Self {
            enabled,
            dirs,
            data_dir,
            status: if enabled {
                PluginStatus::Idle
            } else {
                PluginStatus::Disabled
            },
            python_version: None,
            plugins: Vec::new(),
            commands: HashMap::new(),
            logs: VecDeque::new(),
            rx: None,
            stdin_tx: None,
            child: None,
            host_path,
            last_drop_log: None,
        };
        if enabled {
            manager.start_host();
        }
        manager
    }

    pub fn is_ready(&self) -> bool {
        self.status == PluginStatus::Running
    }

    pub fn command(&self, name: &str) -> Option<&PluginCommand> {
        self.commands.get(name)
    }

    pub fn command_names(&self) -> Vec<&str> {
        let mut names: Vec<&str> = self.commands.keys().map(String::as_str).collect();
        names.sort_unstable();
        names
    }

    pub fn commands_sorted(&self) -> Vec<PluginCommand> {
        let mut commands: Vec<PluginCommand> = self.commands.values().cloned().collect();
        commands.sort_by(|a, b| a.name.cmp(&b.name));
        commands
    }

    pub fn status_text(&self) -> String {
        match &self.status {
            PluginStatus::Disabled => "disabled".to_string(),
            PluginStatus::Idle => "idle (no plugin scripts found)".to_string(),
            PluginStatus::Starting => "starting…".to_string(),
            PluginStatus::Running => match &self.python_version {
                Some(version) => format!(
                    "running · {} plugin(s), {} command(s) · Python {}",
                    self.plugins.len(),
                    self.commands.len(),
                    version
                ),
                None => format!(
                    "running · {} plugin(s), {} command(s)",
                    self.plugins.len(),
                    self.commands.len()
                ),
            },
            PluginStatus::Error(error) => format!("error: {error}"),
        }
    }

    pub fn log_line(&mut self, text: impl Into<String>) {
        let text = format!("[{}] {}", Local::now().format("%H:%M:%S"), text.into());
        self.logs.push_back(text);
        while self.logs.len() > MAX_LOGS {
            self.logs.pop_front();
        }
    }

    pub fn reload(&mut self) {
        if !self.enabled {
            self.log_line("plugins are disabled");
            return;
        }
        self.stop_host();
        self.plugins.clear();
        self.commands.clear();
        self.python_version = None;
        self.log_line("reloading plugins…");
        self.start_host();
    }

    pub fn shutdown(&mut self) {
        self.stop_host();
    }

    pub fn poll(&mut self) -> Vec<PluginEvent> {
        let mut events = Vec::new();
        let mut eof = false;
        loop {
            let received = match self.rx.as_ref() {
                Some(rx) => rx.try_recv(),
                None => break,
            };
            match received {
                Ok(HostRx::Message(message)) => {
                    self.handle_host_message(message, &mut events);
                }
                Ok(HostRx::Stderr(line)) => {
                    self.log_line(format!("[python] {line}"));
                }
                Ok(HostRx::Protocol(line)) => {
                    self.log_line(format!("[protocol] {line}"));
                }
                Ok(HostRx::Eof) => {
                    eof = true;
                    break;
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    eof = true;
                    break;
                }
            }
        }
        if eof && self.child.is_some() {
            self.host_exited("plugin host exited unexpectedly");
            events.push(PluginEvent::Exited(
                "plugin host exited unexpectedly".to_string(),
            ));
        }
        events
    }

    pub fn send_event(&mut self, event: &str, data: Value) {
        self.send_value(json!({
            "type": "event",
            "event": event,
            "data": data,
        }));
    }

    pub fn send_state(&mut self, data: Value) {
        if !self.is_ready() {
            return;
        }
        self.send_value(json!({
            "type": "state",
            "data": data,
        }));
    }

    pub fn dispatch_command(&mut self, name: &str, args: &str, data: Value) {
        if !self.is_ready() {
            return;
        }
        self.send_value(json!({
            "type": "command",
            "name": name,
            "args": args,
            "data": data,
        }));
    }

    fn send_value(&mut self, value: Value) {
        let Some(tx) = self.stdin_tx.clone() else {
            return;
        };
        let line = value.to_string();
        match tx.try_send(line) {
            Ok(()) => {}
            Err(TrySendError::Full(_)) => {
                let now = Instant::now();
                if self
                    .last_drop_log
                    .map(|last| now.duration_since(last) >= Duration::from_secs(10))
                    .unwrap_or(true)
                {
                    self.last_drop_log = Some(now);
                    self.log_line("plugin host is busy; dropped an event");
                }
            }
            Err(TrySendError::Disconnected(_)) => {
                self.log_line("plugin host stdin closed");
            }
        }
    }

    fn handle_host_message(&mut self, message: HostMessage, events: &mut Vec<PluginEvent>) {
        match message {
            HostMessage::Ready {
                python,
                plugins,
                commands,
            } => {
                self.status = PluginStatus::Running;
                self.python_version = if python.is_empty() {
                    None
                } else {
                    Some(python)
                };
                self.plugins = plugins;
                self.commands = commands
                    .into_iter()
                    .map(|command| (command.name.clone(), command))
                    .collect();
                self.log_line(format!(
                    "plugin host ready: {} plugin(s), {} command(s)",
                    self.plugins.len(),
                    self.commands.len()
                ));
                events.push(PluginEvent::Ready);
            }
            HostMessage::Command {
                plugin,
                name,
                help,
                usage,
            } => {
                let name = name.to_ascii_lowercase();
                if let Some(existing) = self.commands.get(&name) {
                    if existing.plugin != plugin {
                        self.log_line(format!(
                            "command /{name} from {plugin} conflicts with {}; keeping the first",
                            existing.plugin
                        ));
                        return;
                    }
                }
                self.commands.insert(
                    name.clone(),
                    PluginCommand {
                        name,
                        plugin,
                        help,
                        usage,
                    },
                );
            }
            HostMessage::Action {
                plugin,
                action,
                data,
            } => events.push(PluginEvent::Action {
                plugin,
                action,
                data,
            }),
            HostMessage::Notify {
                plugin,
                text,
                level,
            } => {
                if !plugin.is_empty() {
                    self.log_line(format!("[{plugin}] {text}"));
                }
                events.push(PluginEvent::Notify { text, level });
            }
            HostMessage::Log { plugin, text } => {
                let prefix = plugin.unwrap_or_else(|| "host".to_string());
                self.log_line(format!("[{prefix}] {text}"));
            }
            HostMessage::Error { plugin, text } => {
                let prefix = plugin.unwrap_or_else(|| "host".to_string());
                self.log_line(format!("[{prefix}] ERROR: {text}"));
            }
        }
    }

    fn start_host(&mut self) {
        if !self.enabled {
            return;
        }
        self.status = PluginStatus::Starting;
        if !has_plugins(&self.dirs) {
            self.status = PluginStatus::Idle;
            self.log_line("no plugin scripts found");
            return;
        }
        let Some(python) = find_python() else {
            let error = "Python 3 not found (set RIRC_PYTHON)".to_string();
            self.status = PluginStatus::Error(error.clone());
            self.log_line(error);
            return;
        };
        if fs::write(&self.host_path, HOST_SCRIPT).is_err() {
            let error = "could not write plugin host script".to_string();
            self.status = PluginStatus::Error(error.clone());
            self.log_line(error);
            return;
        }
        if let Err(error) = fs::create_dir_all(&self.data_dir) {
            self.log_line(format!("could not create plugin data directory: {error}"));
        }

        let mut command = python.command();
        command
            .arg("-u")
            .arg(&self.host_path)
            .arg("--data-dir")
            .arg(&self.data_dir)
            .env("PYTHONIOENCODING", "utf-8")
            .env("PYTHONUTF8", "1")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        for dir in &self.dirs {
            command.arg("--plugin-dir").arg(dir);
        }
        let mut child = match command.spawn() {
            Ok(child) => child,
            Err(error) => {
                let message = format!("could not start {}: {error}", python.display());
                self.status = PluginStatus::Error(message.clone());
                self.log_line(message);
                return;
            }
        };
        let Some(mut stdin) = child.stdin.take() else {
            self.log_line("plugin host did not open stdin");
            let _ = child.kill();
            return;
        };
        let Some(stdout) = child.stdout.take() else {
            self.log_line("plugin host did not open stdout");
            let _ = child.kill();
            return;
        };
        let Some(stderr) = child.stderr.take() else {
            self.log_line("plugin host did not open stderr");
            let _ = child.kill();
            return;
        };

        let (host_tx, host_rx) = mpsc::channel();
        self.rx = Some(host_rx);
        let (stdin_tx, stdin_rx) = mpsc::sync_channel::<String>(HOST_QUEUE);
        self.stdin_tx = Some(stdin_tx);

        thread::spawn(move || {
            for line in stdin_rx {
                if writeln!(stdin, "{line}").is_err() {
                    break;
                }
                if stdin.flush().is_err() {
                    break;
                }
            }
        });

        let reader_tx = host_tx.clone();
        thread::spawn(move || {
            let reader = BufReader::new(stdout);
            for line in reader.lines() {
                match line {
                    Ok(line) => {
                        if line.trim().is_empty() {
                            continue;
                        }
                        match serde_json::from_str::<HostMessage>(&line) {
                            Ok(message) => {
                                if reader_tx.send(HostRx::Message(message)).is_err() {
                                    return;
                                }
                            }
                            Err(error) => {
                                let _ = reader_tx.send(HostRx::Protocol(format!(
                                    "invalid plugin host output ({error}): {}",
                                    util::truncate(&line, 500)
                                )));
                            }
                        }
                    }
                    Err(error) => {
                        let _ = reader_tx.send(HostRx::Protocol(format!(
                            "plugin host stdout error: {error}"
                        )));
                        break;
                    }
                }
            }
            let _ = reader_tx.send(HostRx::Eof);
        });

        thread::spawn(move || {
            let reader = BufReader::new(stderr);
            for line in reader.lines().map_while(Result::ok) {
                if host_tx
                    .send(HostRx::Stderr(util::truncate(&line, 1000)))
                    .is_err()
                {
                    break;
                }
            }
        });

        self.child = Some(child);
        self.log_line(format!(
            "starting plugin host with {} ({})",
            python.display(),
            self.dirs
                .iter()
                .map(|dir| dir.display().to_string())
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }

    fn stop_host(&mut self) {
        if let Some(tx) = self.stdin_tx.take() {
            let _ = tx.try_send("{\"type\":\"shutdown\"}".to_string());
        }
        if let Some(mut child) = self.child.take() {
            let deadline = Instant::now() + Duration::from_millis(250);
            loop {
                match child.try_wait() {
                    Ok(Some(_)) => break,
                    Ok(None) if Instant::now() < deadline => {
                        thread::sleep(Duration::from_millis(10));
                    }
                    _ => {
                        let _ = child.kill();
                        let _ = child.wait();
                        break;
                    }
                }
            }
        }
        self.rx = None;
        if self.enabled {
            self.status = PluginStatus::Idle;
        }
    }

    fn host_exited(&mut self, reason: &str) {
        self.stdin_tx = None;
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
        self.rx = None;
        self.status = PluginStatus::Error(reason.to_string());
        self.commands.clear();
        self.log_line(reason);
    }
}

impl Drop for PluginManager {
    fn drop(&mut self) {
        self.stop_host();
        let _ = fs::remove_file(&self.host_path);
    }
}

fn plugin_dirs(extra: &[String]) -> Vec<PathBuf> {
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Ok(exe) = env::current_exe() {
        if let Some(parent) = exe.parent() {
            candidates.push(parent.join("plugins"));
        }
    }
    if let Ok(cwd) = env::current_dir() {
        candidates.push(cwd.join("plugins"));
    }
    if let Some(config) = config_dir() {
        candidates.push(config.join("plugins"));
    }
    if let Some(paths) = env::var_os("RIRC_PLUGIN_DIR") {
        candidates.extend(env::split_paths(&paths));
    }
    for dir in extra {
        if !dir.trim().is_empty() {
            candidates.push(PathBuf::from(dir));
        }
    }

    let mut out: Vec<PathBuf> = Vec::new();
    for path in candidates {
        let normalized = fs::canonicalize(&path).unwrap_or(path);
        if !out.contains(&normalized) {
            out.push(normalized);
        }
    }
    out
}

fn config_dir() -> Option<PathBuf> {
    #[cfg(target_os = "windows")]
    {
        env::var_os("APPDATA")
            .filter(|value| !value.is_empty())
            .map(|value| PathBuf::from(value).join("rirc"))
    }
    #[cfg(target_os = "macos")]
    {
        env::var_os("HOME")
            .filter(|value| !value.is_empty())
            .map(|value| {
                PathBuf::from(value)
                    .join("Library")
                    .join("Application Support")
                    .join("rirc")
            })
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        env::var_os("XDG_CONFIG_HOME")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .or_else(|| {
                env::var_os("HOME")
                    .filter(|value| !value.is_empty())
                    .map(|value| PathBuf::from(value).join(".config"))
            })
            .map(|value| value.join("rirc"))
    }
    #[cfg(not(any(unix, target_os = "windows", target_os = "macos")))]
    {
        None
    }
}

fn plugin_data_dir() -> PathBuf {
    config_dir()
        .unwrap_or_else(|| env::temp_dir().join("rirc"))
        .join("plugin-data")
}

fn has_plugins(dirs: &[PathBuf]) -> bool {
    dirs.iter().any(|dir| {
        fs::read_dir(dir)
            .map(|entries| {
                entries.filter_map(Result::ok).any(|entry| {
                    let path = entry.path();
                    path.extension().map(|ext| ext == "py").unwrap_or(false)
                        && !entry.file_name().to_string_lossy().starts_with('_')
                })
            })
            .unwrap_or(false)
    })
}

#[cfg(test)]
pub fn python_available() -> bool {
    find_python().is_some()
}

fn find_python() -> Option<PythonCmd> {
    if let Some(value) = env::var_os("RIRC_PYTHON") {
        if !value.is_empty() {
            let command = PythonCmd {
                program: value.to_string_lossy().into_owned(),
                args: Vec::new(),
            };
            if can_run(&command) {
                return Some(command);
            }
        }
    }

    let candidates: Vec<PythonCmd> = if cfg!(windows) {
        vec![
            PythonCmd {
                program: "py".to_string(),
                args: vec!["-3".to_string()],
            },
            PythonCmd {
                program: "python".to_string(),
                args: Vec::new(),
            },
            PythonCmd {
                program: "python3".to_string(),
                args: Vec::new(),
            },
        ]
    } else {
        vec![
            PythonCmd {
                program: "python3".to_string(),
                args: Vec::new(),
            },
            PythonCmd {
                program: "python".to_string(),
                args: Vec::new(),
            },
        ]
    };
    candidates.into_iter().find(can_run)
}

fn can_run(python: &PythonCmd) -> bool {
    let mut command = python.command();
    command
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let Ok(mut child) = command.spawn() else {
        return false;
    };
    let deadline = Instant::now() + Duration::from_secs(1);
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return status.success(),
            Ok(None) => {}
            Err(_) => return false,
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return false;
        }
        thread::sleep(Duration::from_millis(25));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn wait_events(
        manager: &mut PluginManager,
        timeout: Duration,
        mut done: impl FnMut(&[PluginEvent]) -> bool,
    ) -> Vec<PluginEvent> {
        let deadline = Instant::now() + timeout;
        let mut collected = Vec::new();
        while Instant::now() < deadline {
            let mut events = manager.poll();
            if !events.is_empty() {
                collected.append(&mut events);
                if done(&collected) {
                    return collected;
                }
            }
            thread::sleep(Duration::from_millis(20));
        }
        collected
    }

    #[test]
    fn python_host_loads_dispatches_and_acts() {
        if find_python().is_none() {
            return;
        }
        let root = env::temp_dir().join(format!(
            "rirc-plugin-test-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|value| value.as_millis())
                .unwrap_or(0)
        ));
        let plugin_dir = root.join("plugins");
        let data_dir = root.join("data");
        fs::create_dir_all(&plugin_dir).unwrap();
        fs::write(
            plugin_dir.join("tester.py"),
            r##"
PLUGIN = {"name": "tester", "version": "1.0", "description": "test plugin"}

def setup(api):
    @api.command("echo", help="echo arguments")
    def echo(api, args):
        api.notify("echo:" + args)
        api.send("from-plugin", target="#test")

def on_start(api):
    api.log("started", api.active().get("buffer", "?"))

def on_message(api, message):
    if message.get("text") == "ping":
        api.notify("message:" + message.get("nick", "?"))
"##,
        )
        .unwrap();

        let mut manager =
            PluginManager::with_paths(true, vec![plugin_dir.clone()], data_dir.clone());
        let ready = wait_events(&mut manager, Duration::from_secs(15), |events| {
            events
                .iter()
                .any(|event| matches!(event, PluginEvent::Ready))
        });
        assert!(
            ready
                .iter()
                .any(|event| matches!(event, PluginEvent::Ready)),
            "plugin host did not become ready: {:?}",
            manager.logs
        );
        assert!(manager.command("echo").is_some());
        assert_eq!(manager.plugins.len(), 1);
        assert_eq!(manager.plugins[0].name, "tester");

        manager.send_state(json!({
            "servers": [{"id": 0, "name": "TestNet", "nick": "tester", "connected": true}],
            "buffers": [{"server": 0, "name": "#test", "kind": "channel", "joined": true}],
            "active": {
                "server": 0,
                "server_name": "TestNet",
                "buffer": "#test",
                "target": "#test",
                "reply_target": "#test",
                "kind": "channel",
                "is_channel": true,
                "nick": "tester"
            },
            "users": [],
            "messages": []
        }));
        let _ = wait_events(&mut manager, Duration::from_millis(500), |_| false);
        assert!(
            manager
                .logs
                .iter()
                .any(|line| line.contains("started #test")),
            "on_start did not run: {:?}",
            manager.logs
        );

        manager.dispatch_command(
            "echo",
            "hello",
            json!({"server": 0, "buffer": "#test", "target": "#test", "reply_target": "#test"}),
        );
        let events = wait_events(&mut manager, Duration::from_secs(5), |events| {
            events.iter().any(
                |event| matches!(event, PluginEvent::Notify { text, .. } if text == "echo:hello"),
            )
        });
        assert!(events.iter().any(|event| {
            matches!(event, PluginEvent::Notify { text, .. } if text == "echo:hello")
        }));
        assert!(events.iter().any(|event| match event {
            PluginEvent::Action {
                plugin,
                action,
                data,
            } => {
                plugin == "tester"
                    && action == "send_message"
                    && data.get("target").and_then(Value::as_str) == Some("#test")
            }
            _ => false,
        }));

        manager.send_event(
            "message",
            json!({
                "server": 0,
                "buffer": "#test",
                "target": "#test",
                "reply_target": "#test",
                "nick": "alice",
                "text": "ping",
                "kind": "message"
            }),
        );
        let events = wait_events(&mut manager, Duration::from_secs(5), |events| {
            events.iter().any(|event| {
                matches!(event, PluginEvent::Notify { text, .. } if text == "message:alice")
            })
        });
        assert!(events.iter().any(|event| {
            matches!(event, PluginEvent::Notify { text, .. } if text == "message:alice")
        }));

        drop(manager);
        let _ = fs::remove_dir_all(&root);
    }
}
