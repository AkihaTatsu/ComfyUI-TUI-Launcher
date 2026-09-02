//! Clipboard writes for local desktops and remote terminal sessions.
//!
//! A normal desktop uses the native clipboard. SSH, Mosh and terminal
//! multiplexers write to the outer terminal instead, avoiding remote X11 and
//! Wayland connections. tmux is the sole special case because its secure
//! default rejects OSC 52 written directly by pane applications.

use std::cell::RefCell;
use std::io::{self, IsTerminal, Write};
use std::process::Command;

use base64::engine::general_purpose::STANDARD;
use base64::Engine as _;

use crate::app::FlashKind;
use crate::core::i18n;

const TERMINAL_SESSION_ENV_VARS: [&str; 8] = [
    "SSH_CONNECTION",
    "SSH_CLIENT",
    "SSH_TTY",
    "MOSH_CONNECTION",
    "TMUX",
    "ZELLIJ",
    "STY",
    "HERDR_ENV",
];

thread_local! {
    // X11 and Wayland serve clipboard contents from the process that owns the
    // selection, so keep the handle alive for the TUI thread's lifetime.
    static SYSTEM_CLIPBOARD: RefCell<Option<arboard::Clipboard>> = const { RefCell::new(None) };
}

/// Write `text` to the clipboard appropriate for the current session.
///
/// Native clipboard errors in a local session fall back to OSC 52. Remote and
/// multiplexed sessions never initialize the native clipboard, preventing a
/// stale or forwarded `$DISPLAY` from blocking an SSH session.
pub fn copy(text: &str) -> Result<(), String> {
    if is_terminal_session() {
        return copy_terminal(text);
    }

    copy_system(text).or_else(|system_error| {
        copy_terminal(text).map_err(|terminal_error| {
            format!(
                "system clipboard failed: {system_error}; terminal clipboard fallback failed: {terminal_error}"
            )
        })
    })
}

fn is_terminal_session() -> bool {
    is_terminal_session_with(env_var_is_nonempty)
}

fn is_terminal_session_with(mut is_set: impl FnMut(&str) -> bool) -> bool {
    TERMINAL_SESSION_ENV_VARS.iter().any(|name| is_set(name))
}

fn env_var_is_nonempty(name: &str) -> bool {
    std::env::var_os(name).is_some_and(|value| !value.is_empty())
}

fn copy_system(text: &str) -> Result<(), String> {
    SYSTEM_CLIPBOARD.with(|slot| {
        let mut slot = slot.borrow_mut();
        if slot.is_none() {
            *slot = Some(arboard::Clipboard::new().map_err(|error| error.to_string())?);
        }

        let result = slot
            .as_mut()
            .expect("clipboard was initialized above")
            .set_text(text.to_owned())
            .map_err(|error| error.to_string());

        // Recreate the platform handle after a display server restart or a
        // transient backend failure instead of retaining a poisoned handle.
        if result.is_err() {
            *slot = None;
        }
        result
    })
}

fn copy_terminal(text: &str) -> Result<(), String> {
    if !io::stdout().is_terminal() {
        return Err(
            "terminal clipboard is unavailable because standard output is not a terminal"
                .to_string(),
        );
    }

    if env_var_is_nonempty("TMUX") {
        return copy_tmux(text);
    }

    let stdout = io::stdout();
    let mut output = stdout.lock();
    write_osc52(&mut output, text)
        .map_err(|error| format!("failed to send terminal clipboard request: {error}"))
}

fn copy_tmux(text: &str) -> Result<(), String> {
    let output = tmux_clipboard_command(text)
        .output()
        .map_err(|error| format!("failed to run tmux clipboard command: {error}"))?;

    if output.status.success() {
        return Ok(());
    }

    let status = output.status.code().map_or_else(
        || "terminated by signal".to_string(),
        |code| code.to_string(),
    );
    let detail = String::from_utf8_lossy(&output.stderr);
    let detail = detail.trim();
    if detail.is_empty() {
        Err(format!(
            "tmux clipboard command failed with status {status}; tmux 3.2 or newer with OSC 52 support is required"
        ))
    } else {
        Err(format!(
            "tmux clipboard command failed with status {status}: {detail}"
        ))
    }
}

fn tmux_clipboard_command(text: &str) -> Command {
    let mut command = Command::new("tmux");
    command.args(["set-buffer", "-w", "--"]).arg(text);
    command
}

fn write_osc52<W: Write>(output: &mut W, text: &str) -> io::Result<()> {
    let payload = STANDARD.encode(text.as_bytes());
    output.write_all(b"\x1b]52;c;")?;
    output.write_all(payload.as_bytes())?;
    output.write_all(b"\x07")?;
    output.flush()
}

/// Copy `text` and return a ready-to-display flash describing the result.
pub fn copy_with_flash(text: &str) -> (FlashKind, String) {
    match copy(text) {
        Ok(()) => (FlashKind::Info, i18n::t("popup_copied")),
        Err(error) => (
            FlashKind::Error,
            format!("{} {error}", i18n::t("popup_copy_failed")),
        ),
    }
}

#[cfg(test)]
mod tests {
    use std::io::{self, Write};

    use super::*;

    #[test]
    fn remote_and_multiplexer_variables_select_the_terminal() {
        for expected in TERMINAL_SESSION_ENV_VARS {
            assert!(is_terminal_session_with(|name| name == expected));
        }
        assert!(!is_terminal_session_with(|_| false));
    }

    #[test]
    fn tmux_command_preserves_argument_boundaries() {
        let command = tmux_clipboard_command("-leading\n复制");
        assert_eq!(command.get_program(), "tmux");
        let args: Vec<_> = command.get_args().collect();
        assert_eq!(args, ["set-buffer", "-w", "--", "-leading\n复制"]);
    }

    /// Run this public-entrypoint probe inside an attached multiplexer and
    /// inspect its outer PTY for the complete OSC 52 payload.
    #[cfg(unix)]
    #[test]
    #[ignore = "requires an attached terminal multiplexer and an outer PTY"]
    fn terminal_clipboard_integration_probe() {
        assert!(is_terminal_session());
        copy("comfyui-multiplexer-integration-复制").unwrap();
    }

    #[test]
    fn osc52_encodes_ascii_unicode_newlines_and_empty_text() {
        let cases = [
            ("hello", "\x1b]52;c;aGVsbG8=\x07"),
            ("复制\n", "\x1b]52;c;5aSN5Yi2Cg==\x07"),
            ("", "\x1b]52;c;\x07"),
        ];

        for (text, expected) in cases {
            let mut output = Vec::new();
            write_osc52(&mut output, text).unwrap();
            assert_eq!(output, expected.as_bytes());
        }
    }

    struct FailingWriter {
        fail_write: bool,
        fail_flush: bool,
    }

    impl Write for FailingWriter {
        fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
            if self.fail_write {
                Err(io::Error::new(io::ErrorKind::BrokenPipe, "write failed"))
            } else {
                Ok(buffer.len())
            }
        }

        fn flush(&mut self) -> io::Result<()> {
            if self.fail_flush {
                Err(io::Error::new(io::ErrorKind::BrokenPipe, "flush failed"))
            } else {
                Ok(())
            }
        }
    }

    #[test]
    fn osc52_propagates_write_and_flush_errors() {
        let write_error = write_osc52(
            &mut FailingWriter {
                fail_write: true,
                fail_flush: false,
            },
            "value",
        )
        .unwrap_err();
        assert_eq!(write_error.to_string(), "write failed");

        let flush_error = write_osc52(
            &mut FailingWriter {
                fail_write: false,
                fail_flush: true,
            },
            "value",
        )
        .unwrap_err();
        assert_eq!(flush_error.to_string(), "flush failed");
    }
}
