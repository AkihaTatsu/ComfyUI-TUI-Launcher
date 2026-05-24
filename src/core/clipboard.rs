//! System clipboard access for the launcher.
//!
//! All clipboard interaction goes through this module so the per-call
//! `arboard::Clipboard` lifecycle (recommended by the upstream docs for
//! Linux/Wayland safety) and the localised success / failure flash
//! formatting live in exactly one place.

use crate::app::FlashKind;
use crate::core::i18n;

/// Write `text` to the system clipboard.
///
/// Returns the underlying error message on failure (no clipboard
/// provider, no `$DISPLAY`, etc.) so the caller can surface a localised
/// flash. A fresh `arboard::Clipboard` is created per call because
/// `arboard`'s docs warn against holding the handle long-term on Linux.
pub fn copy(text: &str) -> Result<(), String> {
    let mut cb = arboard::Clipboard::new().map_err(|e| e.to_string())?;
    cb.set_text(text.to_string()).map_err(|e| e.to_string())
}

/// Copy `text` and return a ready-to-display flash describing the result.
///
/// Callers stash the returned pair in their own `pending_flash` slot; the
/// App's per-frame drain surfaces it as the top-right banner.
pub fn copy_with_flash(text: &str) -> (FlashKind, String) {
    match copy(text) {
        Ok(()) => (FlashKind::Info, i18n::t("popup_copied")),
        Err(e) => (
            FlashKind::Error,
            format!("{} {e}", i18n::t("popup_copy_failed")),
        ),
    }
}
