//! NORMAL mode: vim-ish navigation and actions, no text entry.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::app::{App, Focus};

pub fn handle(app: &mut App, key: KeyEvent) {
    if key.modifiers.contains(KeyModifiers::CONTROL) {
        match key.code {
            KeyCode::Char('n') => app.next_buffer(1),
            KeyCode::Char('p') => app.next_buffer(-1),
            KeyCode::Char('d') if app.focus == Focus::Messages => app.select_msg(10),
            KeyCode::Char('u') if app.focus == Focus::Messages => app.select_msg(-10),
            KeyCode::Char('c') => app.open_quit_menu(),
            _ => {}
        }
        return;
    }

    match key.code {
        KeyCode::Char(':') => app.start_command(":"),
        KeyCode::Char('/') => app.start_command("/"),
        KeyCode::Char('i') | KeyCode::Char('a') => app.start_insert(),
        KeyCode::Tab => app.cycle_focus(1),
        KeyCode::BackTab => app.cycle_focus(-1),
        KeyCode::Char('?') => app.open_help(),
        KeyCode::Char('Q') => app.open_quit_menu(),
        _ => panel(app, key),
    }
}

fn panel(app: &mut App, key: KeyEvent) {
    match app.focus {
        Focus::Messages => messages(app, key),
        Focus::Channels => channels(app, key),
        Focus::Members => members(app, key),
    }
}

fn messages(app: &mut App, key: KeyEvent) {
    match key.code {
        KeyCode::Char('j') | KeyCode::Down => app.select_msg(1),
        KeyCode::Char('k') | KeyCode::Up => app.select_msg(-1),
        KeyCode::Char('g') | KeyCode::Home => app.jump_message(false),
        KeyCode::Char('G') | KeyCode::End => app.jump_message(true),
        KeyCode::PageDown => app.select_msg(15),
        KeyCode::PageUp => app.select_msg(-15),
        KeyCode::Enter => app.open_menu(),
        KeyCode::Char('v') => app.enter_visual(),
        KeyCode::Char('y') => app.yank_current(),
        _ => {}
    }
}

fn channels(app: &mut App, key: KeyEvent) {
    let len = app.sidebar_entries().len();
    match key.code {
        KeyCode::Char('j') | KeyCode::Down => {
            if len > 0 {
                app.sidebar_nav = (app.sidebar_nav + 1).min(len - 1);
            }
        }
        KeyCode::Char('k') | KeyCode::Up => {
            app.sidebar_nav = app.sidebar_nav.saturating_sub(1);
        }
        KeyCode::Char('g') | KeyCode::Home => app.sidebar_nav = 0,
        KeyCode::Char('G') | KeyCode::End => app.sidebar_nav = len.saturating_sub(1),
        KeyCode::Enter => app.open_menu(),
        KeyCode::Char('l') | KeyCode::Right => app.activate_sidebar(),
        KeyCode::Char('c') => app.open_menu(),
        _ => {}
    }
}

fn members(app: &mut App, key: KeyEvent) {
    let len = app
        .active
        .map(|bid| app.member_order(bid).len())
        .unwrap_or(0);
    match key.code {
        KeyCode::Char('j') | KeyCode::Down => {
            if len > 0 {
                app.member_nav = (app.member_nav + 1).min(len - 1);
            }
        }
        KeyCode::Char('k') | KeyCode::Up => {
            app.member_nav = app.member_nav.saturating_sub(1);
        }
        KeyCode::Char('g') | KeyCode::Home => app.member_nav = 0,
        KeyCode::Char('G') | KeyCode::End => app.member_nav = len.saturating_sub(1),
        KeyCode::Enter => app.open_menu(),
        KeyCode::Char('l') | KeyCode::Right => app.open_query_for_selected_member(),
        KeyCode::Char('y') => {
            // Copy the highlighted nick without opening a menu.
            if let Some(bid) = app.active {
                let order = app.member_order(bid);
                if let Some(&idx) = order.get(app.member_nav) {
                    if let Some(member) = app.buffers[bid].members().get(idx) {
                        let nick = member.nick.clone();
                        app.copy(nick, "nickname");
                    }
                }
            }
        }
        _ => {}
    }
}
