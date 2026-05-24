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
use std::time::Duration;

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
    loop {
        app.tick();
        term.draw(|f| app.draw(f))?;
        if event::poll(Duration::from_millis(150))? {
            match event::read()? {
                Event::Key(k) if k.kind == KeyEventKind::Press => app.on_key(k),
                Event::Mouse(m) => app.on_mouse(m),
                Event::Resize(_, _) => {}
                _ => {}
            }
        }
        if app.should_quit || app.should_launch {
            return Ok(());
        }
    }
}
