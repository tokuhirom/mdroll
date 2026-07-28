//! Yanking text to the system clipboard.
//!
//! Local sessions use `arboard`. When `SSH_CONNECTION` is set, or when
//! `arboard` fails, the text goes out as an OSC 52 sequence instead, which the
//! terminal emulator picks up on the far end of the connection.

use anyhow::Result;
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use std::io::Write;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Route {
    System,
    Osc52,
}

impl Route {
    pub fn label(self) -> &'static str {
        match self {
            Route::System => "clipboard",
            Route::Osc52 => "clipboard (OSC 52)",
        }
    }
}

/// Whether this session looks remote. OSC 52 is the only thing that crosses an
/// SSH connection, so prefer it there without even trying the local clipboard.
pub fn is_remote() -> bool {
    std::env::var_os("SSH_CONNECTION").is_some() || std::env::var_os("SSH_TTY").is_some()
}

/// The OSC 52 sequence that asks the terminal to set the clipboard.
pub fn osc52(text: &str) -> String {
    format!("\x1b]52;c;{}\x07", STANDARD.encode(text))
}

pub fn copy<W: Write>(out: &mut W, text: &str) -> Result<Route> {
    if !is_remote()
        && let Ok(mut clipboard) = arboard::Clipboard::new()
        && clipboard.set_text(text.to_string()).is_ok()
    {
        return Ok(Route::System);
    }
    out.write_all(osc52(text).as_bytes())?;
    out.flush()?;
    Ok(Route::Osc52)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn osc52_wraps_base64_in_the_right_envelope() {
        assert_eq!(osc52("hi"), "\x1b]52;c;aGk=\x07");
    }

    #[test]
    fn osc52_handles_multibyte_text() {
        let seq = osc52("日本語");
        assert!(seq.starts_with("\x1b]52;c;"));
        assert!(seq.ends_with('\x07'));
        let payload = seq.trim_start_matches("\x1b]52;c;").trim_end_matches('\x07');
        assert_eq!(String::from_utf8(STANDARD.decode(payload).unwrap()).unwrap(), "日本語");
    }
}
