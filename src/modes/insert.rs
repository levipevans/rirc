//! TEXT / COMMAND mode: the line editor. Enter submits, Esc returns to normal.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::app::App;

pub fn handle(app: &mut App, key: KeyEvent) {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    match key.code {
        KeyCode::Enter => app.submit_text(),
        KeyCode::Esc => app.to_normal(),
        KeyCode::Backspace => app.input.backspace(),
        KeyCode::Delete => app.input.delete(),
        KeyCode::Left => app.input.left(),
        KeyCode::Right => app.input.right(),
        KeyCode::Home => app.input.home(),
        KeyCode::End => app.input.end(),
        KeyCode::Up => app.input.history_prev(),
        KeyCode::Down => app.input.history_next(),
        KeyCode::Tab => app.complete(),
        KeyCode::Char('c') if ctrl => app.to_normal(),
        KeyCode::Char('w') if ctrl => app.input.delete_word(),
        KeyCode::Char('u') if ctrl => app.input.clear_before(),
        KeyCode::Char('a') if ctrl => app.input.home(),
        KeyCode::Char('e') if ctrl => app.input.end(),
        KeyCode::Char(c) if !ctrl && !key.modifiers.contains(KeyModifiers::ALT) => {
            app.input.insert(c)
        }
        _ => {}
    }
}
