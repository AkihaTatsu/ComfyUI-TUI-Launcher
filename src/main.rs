//! Binary entry point for the ComfyUI TUI launcher.
//!
//! Initialises configuration, the schema, the i18n catalogue, and the session
//! log, then drives the ratatui event loop until the user quits or asks to
//! launch ComfyUI / activate a virtualenv.

mod app;
mod core;
mod screens;
mod widgets;

use anyhow::Result;
use crossterm::event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyEventKind};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen, SetTitle,
};
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;
use std::io::stdout;
use std::time::{Duration, Instant};

const EVENT_BATCH_LIMIT: usize = 256;
const HOUSEKEEPING_INTERVAL: Duration = Duration::from_millis(33);

fn latest_resize(events: &[Event]) -> Option<(u16, u16)> {
    events.iter().rev().find_map(|event| match event {
        Event::Resize(width, height) => Some((*width, *height)),
        _ => None,
    })
}

/// Installs a panic hook that restores the terminal to a cooked state before
/// the default handler runs, so a crash does not leave the user stranded in
/// the alternate screen with raw mode enabled.
fn install_panic_hook() {
    let original = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = disable_raw_mode();
        let _ = execute!(stdout(), LeaveAlternateScreen, DisableMouseCapture);
        original(info);
    }));
}

fn main() -> Result<()> {
    install_panic_hook();

    let cfg = crate::core::config::Config::load_or_init()?;
    crate::core::i18n::init(&cfg.general.language);
    let (schema, _) = crate::core::schema::load_or_init()?;

    // Open the on-disk session log under the launcher's logs dir so every
    // subsequent `log_bus::push` is mirrored to a file that outlives the
    // process.
    let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S");
    let session_log = crate::core::paths::logs_dir().join(format!("session-{stamp}.log"));
    if let Err(e) = crate::core::log_bus::init_file(&session_log) {
        eprintln!(
            "warning: cannot open session log {}: {}",
            session_log.display(),
            e
        );
    } else {
        crate::core::log_bus::push(
            "launcher",
            format!("session log: {}", session_log.display()),
        );
    }

    // Move any legacy `extensions_cache.json` out of the config dir into the
    // cache dir, removing the stale copy. Best-effort.
    {
        let old = crate::core::paths::config_dir().join("extensions_cache.json");
        let new = crate::core::paths::cache_dir().join("extensions_cache.json");
        if old.is_file() && !new.is_file() {
            let _ = crate::core::paths::ensure_cache_dir();
            let _ = std::fs::rename(&old, &new);
        } else if old.is_file() {
            let _ = std::fs::remove_file(&old);
        }
    }

    if !crate::core::python::git_available() {
        eprintln!("{}", crate::core::i18n::t("tutorial_need_git"));
        std::process::exit(2);
    }

    let mut app = crate::app::App::new(cfg, schema);

    enable_raw_mode()?;
    execute!(
        stdout(),
        EnterAlternateScreen,
        EnableMouseCapture,
        SetTitle(crate::core::i18n::t("app_title")),
    )?;
    let backend = CrosstermBackend::new(stdout());
    let mut term = Terminal::new(backend)?;

    let result = run_loop(&mut term, &mut app);

    disable_raw_mode()?;
    execute!(stdout(), LeaveAlternateScreen, DisableMouseCapture)?;

    if let Some(venv) = app.should_activate.clone() {
        crate::core::process::activate_env_and_exit(&venv);
    }
    if app.should_launch {
        app.do_launch();
    }
    result
}

fn run_loop(
    term: &mut Terminal<CrosstermBackend<std::io::Stdout>>,
    app: &mut crate::app::App,
) -> Result<()> {
    let (initial_w, initial_h) = crossterm::terminal::size()?;
    let mut was_too_small =
        initial_w < crate::app::MIN_VIEWPORT_WIDTH || initial_h < crate::app::MIN_VIEWPORT_HEIGHT;
    app.set_viewport_size(initial_w, initial_h);
    let mut next_tick = Instant::now();
    let mut dirty = true;
    let mut redraw_until = Instant::now();
    let mut log_revision = crate::core::log_bus::revision();

    loop {
        let now = Instant::now();
        let wait = next_tick.saturating_duration_since(now);
        let mut events = Vec::new();
        if event::poll(wait)? {
            events.push(event::read()?);
            while events.len() < EVENT_BATCH_LIMIT && event::poll(Duration::ZERO)? {
                events.push(event::read()?);
            }
        }
        if !events.is_empty() {
            dirty = true;
            // Deferred buttons deliberately fire on a later tick. Keep a
            // short redraw grace period so their pressed/focused state and
            // resulting popup are both observable.
            redraw_until = Instant::now() + Duration::from_millis(250);
        }

        // Resize notifications are explicitly allowed to arrive in bursts.
        // Use the newest dimensions and render once after draining the batch.
        let latest_resize = latest_resize(&events);
        let mut recovered = false;
        if let Some((w, h)) = latest_resize {
            let too_small =
                w < crate::app::MIN_VIEWPORT_WIDTH || h < crate::app::MIN_VIEWPORT_HEIGHT;
            recovered = was_too_small && !too_small;
            was_too_small = too_small;
            app.set_viewport_size(w, h);
        }

        for e in events {
            match e {
                Event::Key(k) if k.kind == KeyEventKind::Press => app.on_key(k),
                // Mouse coordinates gathered during a resize burst refer to
                // an indeterminate layout. Drop them instead of dispatching
                // against stale hit rectangles.
                Event::Mouse(m) if latest_resize.is_none() => app.on_mouse(m),
                _ => {}
            }
        }

        dirty |= app.tick();
        let current_log_revision = crate::core::log_bus::revision();
        if current_log_revision != log_revision {
            log_revision = current_log_revision;
            dirty = true;
        }
        dirty |= recovered || latest_resize.is_some() || Instant::now() < redraw_until;
        if latest_resize.is_some() {
            term.autoresize()?;
        }
        if recovered {
            term.clear()?;
        }
        if dirty {
            term.draw(|f| app.draw(f))?;
            dirty = false;
        }
        next_tick = Instant::now() + HOUSEKEEPING_INTERVAL;

        if app.should_quit || app.should_launch {
            return Ok(());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resize_bursts_use_the_newest_dimensions() {
        let events = [
            Event::Resize(1, 1),
            Event::FocusGained,
            Event::Resize(80, 24),
            Event::Resize(120, 40),
        ];
        assert_eq!(latest_resize(&events), Some((120, 40)));
    }
}
