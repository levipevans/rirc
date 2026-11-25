//! VISUAL mode: select a run of messages and act on them as a block.

use crossterm::event::{KeyCode, KeyEvent};

use crate::app::{App, Mode};

pub fn handle(app: &mut App, key: KeyEvent) {
    match key.code {
        KeyCode::Esc | KeyCode::Char('v') => {
            app.mode = Mode::Normal;
            app.focus = crate::app::Focus::Messages;
        }
        KeyCode::Char('y') => app.yank_selection(),
        KeyCode::Char('j') | KeyCode::Down => app.select_msg(1),
        KeyCode::Char('k') | KeyCode::Up => app.select_msg(-1),
        KeyCode::Char('g') | KeyCode::Home => app.jump_message(false),
        KeyCode::Char('G') | KeyCode::End => app.jump_message(true),
        KeyCode::PageDown => app.select_msg(15),
        KeyCode::PageUp => app.select_msg(-15),
        KeyCode::Char('o') => app.toggle_visual_end(),
        KeyCode::Enter => app.open_menu(),
        KeyCode::Char(':') => {
            app.mode = Mode::Normal;
            app.start_command(":");
        }
        KeyCode::Char('/') => {
            app.mode = Mode::Normal;
            app.start_command("/");
        }
        _ => {}
    }
}
