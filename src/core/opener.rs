//! Open URLs in the platform's default browser.

use std::process::Command;

/// Opens `url` in the platform's default browser on a best-effort basis.
///
/// Returns `false` on a pure-TTY environment (no `$DISPLAY` or
/// `$WAYLAND_DISPLAY` on Linux) so the caller can surface the URL through
/// other means.
pub fn open_url(url: &str) -> bool {
    if url.is_empty() {
        return false;
    }
    #[cfg(target_os = "linux")]
    {
        if std::env::var_os("DISPLAY").is_none() && std::env::var_os("WAYLAND_DISPLAY").is_none() {
            return false;
        }
        return Command::new("xdg-open").arg(url).spawn().is_ok();
    }
    #[cfg(target_os = "macos")]
    {
        return Command::new("open").arg(url).spawn().is_ok();
    }
    #[cfg(target_os = "windows")]
    {
        return Command::new("cmd")
            .args(["/C", "start", "", url])
            .spawn()
            .is_ok();
    }
    #[allow(unreachable_code)]
    false
}
