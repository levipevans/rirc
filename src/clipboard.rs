//! Clipboard integration: native helper + OSC 52 (works over ssh/tmux).

use std::io::Write;
use std::process::{Command, Stdio};

use crate::util;

fn pipe_to(program: &str, args: &[&str], text: &str) -> bool {
    let mut child = match Command::new(program)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    {
        Ok(child) => child,
        Err(_) => return false,
    };
    if let Some(mut stdin) = child.stdin.take() {
        if stdin.write_all(text.as_bytes()).is_err() {
            let _ = child.wait();
            return false;
        }
    }
    child.wait().map(|status| status.success()).unwrap_or(false)
}

fn copy_system(text: &str) -> bool {
    #[cfg(target_os = "windows")]
    {
        pipe_to("clip", &[], text)
    }
    #[cfg(target_os = "macos")]
    {
        pipe_to("pbcopy", &[], text)
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        pipe_to("wl-copy", &[], text)
            || pipe_to("xclip", &["-selection", "clipboard"], text)
            || pipe_to("xsel", &["--clipboard", "--input"], text)
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos", unix)))]
    {
        let _ = text;
        false
    }
}

fn copy_osc52(text: &str) {
    let capac = match std::env::var("OSC52") {
        Ok(value) if !value.is_empty() => value,
        _ => "c".to_string(),
    };
    let encoded = util::base64(text.as_bytes());
    let sequence = format!("\x1b]52;{capac};{encoded}\x07");
    let payload = if std::env::var_os("TMUX").is_some() {
        format!("\x1bPtmux;\x1b{sequence}\x1b\\")
    } else {
        sequence
    };
    let mut stdout = std::io::stdout();
    let _ = stdout.write_all(payload.as_bytes());
    let _ = stdout.flush();
}

/// Copy to every channel we can; returns whether the native clipboard worked.
pub fn copy(text: &str) -> bool {
    let native = copy_system(text);
    copy_osc52(text);
    native
}
