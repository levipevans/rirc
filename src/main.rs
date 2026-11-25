//! rirc — a tiny, vim-flavoured IRCv3 client.

mod app;
mod clipboard;
mod config;
mod demo;
mod input;
mod irc;
mod list;
mod menu;
mod model;
mod modes;
mod plugins;
mod ui;
mod util;

use std::io;
use std::time::Duration;

use crossterm::cursor::{Hide, Show};
use crossterm::event::{
    self, DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture,
    Event, KeyEventKind,
};
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use crossterm::{execute, ExecutableCommand};
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;

use crate::app::App;
use crate::irc::NetEvent;
use tokio::sync::mpsc::{self, error::TryRecvError, Receiver};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut cfg = match config::parse_args(args) {
        Ok(cfg) => cfg,
        Err(error) => {
            eprintln!("rirc: {error}\n\n{}", config::USAGE);
            std::process::exit(2);
        }
    };
    if cfg.help {
        println!("{}", config::USAGE);
        return;
    }
    if cfg.version {
        println!("rirc {}", env!("CARGO_PKG_VERSION"));
        return;
    }
    if cfg.snapshot {
        cfg.demo = true;
        cfg.plugins_enabled = false;
    }

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .worker_threads(2)
        .build()
        .expect("failed to start async runtime");
    let (net_tx, mut net_rx) = mpsc::channel(1024);
    let mut app = App::new(cfg.clone(), Some(runtime.handle().clone()), net_tx);

    if cfg.snapshot {
        for (w, h) in [(120u16, 38u16), (100, 30), (80, 24)] {
            println!("── {w}×{h} ──");
            print!("{}", ui::render_to_string(&mut app, w, h));
            println!();
        }

        // Popup: channel actions.
        app.focus = app::Focus::Channels;
        app.sidebar_nav = 2;
        app.open_menu();
        println!("── channel menu · 100×30 ──");
        print!("{}", ui::render_to_string(&mut app, 100, 30));
        println!();

        // Popup: message actions.
        app.menu = None;
        app.focus = app::Focus::Messages;
        app.open_menu();
        println!("── message menu · 100×30 ──");
        print!("{}", ui::render_to_string(&mut app, 100, 30));
        println!();

        // Visual selection of the last four messages.
        app.menu = None;
        if let Some(bid) = app.active {
            let len = app.buffers[bid].messages.len();
            app.mode = app::Mode::Visual {
                anchor: len.saturating_sub(4),
            };
            app.buffers[bid].selected = len.saturating_sub(1);
        }
        println!("── visual mode · 120×38 ──");
        print!("{}", ui::render_to_string(&mut app, 120, 38));
        println!();

        // Text entry.
        app.mode = app::Mode::Insert;
        app.focus = app::Focus::Messages;
        app.input.set_text("alice: this TUI is lovely");
        println!("── text mode · 100×30 ──");
        print!("{}", ui::render_to_string(&mut app, 100, 30));
        println!();

        // A query keeps the third column, showing the peer.
        app.mode = app::Mode::Normal;
        app.input.clear();
        if let Some(query) = app
            .buffers
            .iter()
            .position(|buffer| matches!(&buffer.kind, crate::model::BufferKind::Query { .. }))
        {
            app.active = Some(query);
            app.focus = app::Focus::Messages;
            if let Some(buffer) = app.buffers.get_mut(query) {
                buffer.unread = 0;
                buffer.mentions = 0;
                buffer.selected = buffer.messages.len().saturating_sub(1);
            }
        }
        println!("── query · 100×30 ──");
        print!("{}", ui::render_to_string(&mut app, 100, 30));
        println!();

        // Channel list (/list), then the same list filtered.
        app.run_command("list");
        println!("── /list · 100×30 ──");
        print!("{}", ui::render_to_string(&mut app, 100, 30));
        println!();
        if let Some(list) = app.list.as_mut() {
            list.filter = "rust".to_string();
            list.normalize_selection();
        }
        println!("── /list filtered · 100×30 ──");
        print!("{}", ui::render_to_string(&mut app, 100, 30));
        println!();
        return;
    }

    install_panic_hook();
    if let Err(error) = run(&mut app, &mut net_rx) {
        eprintln!("rirc: {error}");
        std::process::exit(1);
    }
    // Give the connection tasks a moment to flush QUIT before the runtime dies.
    app.plugins.shutdown();
    drop(app);
    std::thread::sleep(Duration::from_millis(150));
}

fn install_panic_hook() {
    let original = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = disable_raw_mode();
        let mut stdout = io::stdout();
        let _ = execute!(
            stdout,
            LeaveAlternateScreen,
            DisableMouseCapture,
            DisableBracketedPaste,
            Show
        );
        original(info);
    }));
}

fn setup_terminal() -> io::Result<Terminal<CrosstermBackend<io::Stdout>>> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(
        stdout,
        EnterAlternateScreen,
        EnableMouseCapture,
        EnableBracketedPaste,
        Hide
    )?;
    stdout.execute(crossterm::terminal::Clear(
        crossterm::terminal::ClearType::All,
    ))?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;
    terminal.clear()?;
    Ok(terminal)
}

fn restore_terminal(terminal: &mut Terminal<CrosstermBackend<io::Stdout>>) {
    let _ = disable_raw_mode();
    let _ = execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture,
        DisableBracketedPaste,
        Show
    );
    let _ = terminal.show_cursor();
}

fn run(app: &mut App, net_rx: &mut Receiver<NetEvent>) -> io::Result<()> {
    let mut terminal = match setup_terminal() {
        Ok(terminal) => terminal,
        Err(error) => {
            let _ = disable_raw_mode();
            return Err(error);
        }
    };
    let result = event_loop(app, net_rx, &mut terminal);
    restore_terminal(&mut terminal);
    result
}

fn event_loop(
    app: &mut App,
    net_rx: &mut Receiver<NetEvent>,
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
) -> io::Result<()> {
    loop {
        let mut processed = 0usize;
        while processed < 512 {
            match net_rx.try_recv() {
                Ok(event) => {
                    app.on_net(event);
                    processed += 1;
                }
                Err(TryRecvError::Empty) | Err(TryRecvError::Disconnected) => break,
            }
        }
        app.tick();
        app.poll_plugins();
        terminal.draw(|frame| ui::draw(frame, app))?;

        if app.quit {
            return Ok(());
        }

        if event::poll(Duration::from_millis(60))? {
            match event::read()? {
                Event::Key(key) if key.kind != KeyEventKind::Release => app.handle_key(key),
                Event::Mouse(mouse) => app.on_mouse(mouse),
                Event::Paste(text) => app.on_paste(&text),
                Event::Resize(_, _) => {}
                _ => {}
            }
        }
    }
}
