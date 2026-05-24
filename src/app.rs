//! Application state and the top-level event loop driver.
//!
//! Owns the active configuration, the schema, every screen, and the
//! transient banners. `tick`, `draw`, `on_key`, and `on_mouse` are the
//! four entry points called by `main.rs` once per loop iteration.

use crate::core::config::Config;
use crate::core::schema::Schema;
use crate::core::{env, i18n, log_bus, process, schema, theme};
use crate::screens::{
    comfy_settings::ComfySettings, launcher_logs::LauncherLogs,
    launcher_settings::LauncherSettings, main_launcher::MainLauncher, tutorial::Tutorial,
    version_mgmt::VersionMgmt,
};
use crate::widgets::menu::{Menu, MenuEntry};
use crossterm::event::{KeyCode, KeyEvent, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;
use std::cell::Cell;

/// Builds Zellij-style key-hint lines, wrapping to additional rows when
/// the terminal is too narrow to fit the strip on one line.
///
/// Each entry renders as a `KEY` chip in reverse video followed by a
/// space and the description in the base style.
fn hint_lines(items: &[(&str, String)], max_w: u16) -> Vec<Line<'static>> {
    let max_w = max_w.max(1) as usize;
    let mut out: Vec<Line<'static>> = Vec::new();
    let mut cur: Vec<Span<'static>> = Vec::new();
    let mut cur_w: usize = 0;
    for (key, desc) in items {
        let chip = format!(" {key} ");
        let chip_w = crate::core::text::width(&chip);
        let desc_w = crate::core::text::width(desc);
        // " " separator between items, single chip+space+desc per item.
        let sep_w = if cur.is_empty() { 0 } else { 1 };
        let needed = sep_w + chip_w + 1 + desc_w;
        if !cur.is_empty() && cur_w + needed > max_w {
            out.push(Line::from(std::mem::take(&mut cur)));
            cur_w = 0;
        }
        if !cur.is_empty() {
            cur.push(Span::raw(" "));
            cur_w += 1;
        }
        cur.push(Span::styled(chip, theme::focused()));
        cur.push(Span::raw(" "));
        cur.push(Span::styled(desc.clone(), theme::base()));
        cur_w += chip_w + 1 + desc_w;
    }
    if !cur.is_empty() {
        out.push(Line::from(cur));
    }
    if out.is_empty() {
        out.push(Line::from(""));
    }
    out
}

/// Width of the left menu pane in cells.
const MENU_W: u16 = 28;

/// Which screen is currently displayed in the body pane.
#[derive(Copy, Clone, PartialEq, Eq)]
pub enum Screen {
    /// Main launcher screen.
    Main,
    /// ComfyUI settings editor.
    ComfySettings,
    /// Version management.
    VersionMgmt,
    /// Launcher general preferences.
    LauncherSettings,
    /// Launcher log viewer.
    LauncherLogs,
    /// About screen.
    About,
}

/// Top-level application state.
pub struct App {
    /// Active configuration (auto-saved on changes).
    pub cfg: Config,
    /// Parsed settings schema.
    pub schema: Schema,
    /// Left-side navigation menu.
    pub menu: Menu,
    /// Active screen displayed in the body pane.
    pub screen: Screen,
    /// State for the main launcher screen.
    pub main: MainLauncher,
    /// State for the ComfyUI settings screen.
    pub comfy: ComfySettings,
    /// State for the version management screen.
    pub version: VersionMgmt,
    /// State for the launcher settings screen.
    pub launcher: LauncherSettings,
    /// State for the launcher logs screen.
    pub logs: LauncherLogs,
    /// First-run tutorial, when active.
    pub tutorial: Option<Tutorial>,
    /// Flag set when the application should exit cleanly.
    pub should_quit: bool,
    /// Flag set when the application should launch ComfyUI after exit.
    pub should_launch: bool,
    /// Virtualenv root to activate after the run loop exits, consumed by
    /// `main.rs`.
    pub should_activate: Option<std::path::PathBuf>,
    /// Whether keyboard navigation operates on the menu (`true`) or on
    /// the body (`false`).
    pub focus_menu: bool,
    /// Cached body inner rectangle from the last render, used by mouse
    /// hit-tests.
    body_inner: Cell<Rect>,
    /// Cached menu rectangle from the last render.
    menu_rect: Cell<Rect>,
    /// Timestamp of the most recently consumed wheel-scroll event.
    ///
    /// Used to coalesce the burst of events that a single physical wheel
    /// notch produces on terminals with smooth-scrolling mice.
    last_scroll: Cell<std::time::Instant>,
    /// Transient banner shown for three seconds after being set.
    ///
    /// Renders on top of `flash_permanent` while still alive.
    pub flash_temp: Option<(String, FlashKind, std::time::Instant)>,
    /// Sticky banner shown until cleared.
    ///
    /// Suppressed at render time while a transient banner is active.
    pub flash_permanent: Option<(String, FlashKind)>,
}

/// Visual style for a flash banner.
#[derive(Copy, Clone, PartialEq, Eq)]
pub enum FlashKind {
    /// Informational banner styled with the accent colour.
    Info,
    /// Error banner styled with the danger colour.
    Error,
}

impl App {
    /// Constructs the application from a loaded configuration and schema.
    pub fn new(cfg: Config, schema: Schema) -> Self {
        let need_tutorial = cfg.general.comfyui_dir.is_empty()
            || !crate::core::paths::ComfyDirs::new(&cfg.general.comfyui_dir).is_valid()
            || cfg.general.python.is_empty();
        let tutorial = if need_tutorial {
            Some(Tutorial::new(&cfg))
        } else {
            None
        };
        let menu = Menu {
            primary: vec![
                MenuEntry {
                    label_key: "menu_main",
                },
                MenuEntry {
                    label_key: "menu_settings",
                },
                MenuEntry {
                    label_key: "menu_version",
                },
            ],
            secondary: vec![
                MenuEntry {
                    label_key: "menu_general",
                },
                MenuEntry {
                    label_key: "menu_logs",
                },
                MenuEntry {
                    label_key: "menu_about",
                },
            ],
            selected: 0,
        };
        Self {
            cfg,
            schema,
            menu,
            screen: Screen::Main,
            main: MainLauncher::new(),
            comfy: ComfySettings::new(),
            version: VersionMgmt::new(),
            launcher: LauncherSettings::new(),
            logs: LauncherLogs::new(),
            tutorial,
            should_quit: false,
            should_launch: false,
            should_activate: None,
            focus_menu: true,
            body_inner: Cell::new(Rect::default()),
            menu_rect: Cell::new(Rect::default()),
            // Seed in the past so the first real wheel event passes the
            // coalescing window.
            last_scroll: Cell::new(std::time::Instant::now() - std::time::Duration::from_secs(1)),
            flash_temp: None,
            flash_permanent: None,
        }
    }

    /// Sets the transient informational banner.
    ///
    /// Always overwrites any existing transient banner, and renders on top
    /// of the permanent banner while still alive.
    pub fn set_flash(&mut self, msg: impl Into<String>) {
        self.set_flash_temp(msg, FlashKind::Info);
    }

    /// Sets the transient error banner.
    pub fn set_flash_error(&mut self, msg: impl Into<String>) {
        self.set_flash_temp(msg, FlashKind::Error);
    }

    fn set_flash_temp(&mut self, msg: impl Into<String>, kind: FlashKind) {
        let expires = std::time::Instant::now() + std::time::Duration::from_secs(3);
        self.flash_temp = Some((msg.into(), kind, expires));
    }

    /// Sets the sticky banner shown until cleared or replaced.
    ///
    /// Suppressed at render time while a transient banner is active.
    pub fn set_flash_permanent(&mut self, msg: impl Into<String>, kind: FlashKind) {
        self.flash_permanent = Some((msg.into(), kind));
    }

    /// Clears the sticky banner. Idempotent.
    pub fn clear_flash_permanent(&mut self) {
        self.flash_permanent = None;
    }

    /// Performs per-frame housekeeping that needs `&mut self`.
    ///
    /// Called from the main loop before `draw`. Polls background task
    /// channels, drains screen-level flash messages into the banner
    /// slots, and expires the transient banner when its three-second
    /// window elapses.
    pub fn tick(&mut self) {
        // Swap the i18n catalogue when the user picks a different
        // language. This is the single language-change detector in the
        // application.
        if i18n::current_lang() != self.cfg.general.language {
            i18n::init(&self.cfg.general.language);
            let _ = crossterm::execute!(
                std::io::stdout(),
                crossterm::terminal::SetTitle(i18n::t("app_title"))
            );
        }
        self.version.tick(&self.cfg);
        // Poll the Button widget's deferred-fire pipeline on the main
        // screen.
        let act = self.main.poll_button_action(&self.cfg);
        if !matches!(act, crate::screens::main_launcher::MainAction::None) {
            self.apply_main_action(act);
        }
        // Drain one-shot screen flashes into the transient banner slot.
        if let Some((k, m)) = self.version.take_flash() {
            match k {
                FlashKind::Info => self.set_flash(m),
                FlashKind::Error => self.set_flash_error(m),
            }
        }
        // Mirror background-refresh progress into the permanent banner
        // slot; it clears automatically when the refresh finishes.
        match self.version.permanent_flash() {
            Some((k, m)) => self.set_flash_permanent(m, k),
            None => self.clear_flash_permanent(),
        }
        self.logs.tick();
        if let Some(t) = &mut self.tutorial {
            t.tick(&mut self.cfg);
            if t.is_done() {
                self.tutorial = None;
            }
        }
        // Expire transient banners; permanent banners never expire by time.
        if let Some((_, _, exp)) = &self.flash_temp {
            if *exp < std::time::Instant::now() {
                self.flash_temp = None;
            }
        }
    }

    /// Renders the current frame.
    pub fn draw(&self, f: &mut Frame) {
        let area = f.area();
        // Compute wrapped hint lines first so the status row gets exactly
        // the height it needs, and the body shrinks correspondingly.
        let hints = self.key_hints();
        let hint_lines_v = hint_lines(&hints, area.width);
        let hint_h = (hint_lines_v.len() as u16).max(1);
        let v = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1),
                Constraint::Min(0),
                Constraint::Length(hint_h),
            ])
            .split(area);

        let title = format!(" {} ", i18n::t("app_title"));
        // Banner priority is Transient over Permanent. Within each tier
        // the single slot already encodes last-write-wins.
        let flash_info: Option<(String, FlashKind)> = self
            .flash_temp
            .as_ref()
            .map(|(s, k, _)| (s.clone(), *k))
            .or_else(|| self.flash_permanent.as_ref().map(|(s, k)| (s.clone(), *k)));
        let flash_w = flash_info
            .as_ref()
            .map(|(s, _)| {
                use unicode_width::UnicodeWidthStr;
                (s.width() as u16).saturating_add(2)
            })
            .unwrap_or(0);
        let title_h = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Min(0), Constraint::Length(flash_w)])
            .split(v[0]);
        f.render_widget(
            Paragraph::new(Line::from(title)).style(theme::accent()),
            title_h[0],
        );
        if let Some((s, kind)) = flash_info {
            let style = match kind {
                FlashKind::Info => theme::accent(),
                FlashKind::Error => theme::danger(),
            };
            f.render_widget(
                Paragraph::new(Line::from(format!(" {s} ")))
                    .style(style)
                    .alignment(ratatui::layout::Alignment::Right),
                title_h[1],
            );
        }

        if let Some(t) = &self.tutorial {
            t.render(f, v[1]);
            // Track the rect so the mouse handler can hit-test it.
            self.body_inner.set(v[1]);
        } else {
            let h = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([Constraint::Length(MENU_W), Constraint::Min(0)])
                .split(v[1]);
            self.menu.render(f, h[0], self.focus_menu);
            self.menu_rect.set(h[0]);
            let body = h[1];
            let body_focused = !self.focus_menu;
            let border_style = if body_focused {
                theme::accent()
            } else {
                theme::border()
            };
            let body_block = Block::default()
                .borders(Borders::ALL)
                .border_type(theme::border_type(body_focused))
                .border_style(border_style);
            f.render_widget(body_block, body);
            let inner = Rect {
                x: body.x + 1,
                y: body.y + 1,
                width: body.width.saturating_sub(2),
                height: body.height.saturating_sub(2),
            };
            self.body_inner.set(inner);
            let body_active = !self.focus_menu;
            match self.screen {
                Screen::Main => self
                    .main
                    .render(f, inner, &self.cfg, &self.schema, body_active),
                Screen::ComfySettings => {
                    self.comfy
                        .render(f, inner, &self.schema, &self.cfg, body_active)
                }
                Screen::VersionMgmt => self.version.render(f, inner, &self.cfg, body_active),
                Screen::LauncherSettings => self.launcher.render(f, inner, &self.cfg, body_active),
                Screen::LauncherLogs => self.logs.render(f, inner, body_active),
                Screen::About => crate::screens::about::render(f, inner),
            }
        }

        f.render_widget(Paragraph::new(hint_lines_v), v[2]);
    }

    /// Handles a key event.
    pub fn on_key(&mut self, k: KeyEvent) {
        // Ctrl+C force-quits from anywhere, including text inputs, because
        // the modifier check runs before any screen sees the event.
        let ctrl = k
            .modifiers
            .contains(crossterm::event::KeyModifiers::CONTROL);
        if ctrl && matches!(k.code, KeyCode::Char('c') | KeyCode::Char('C')) {
            self.should_quit = true;
            return;
        }

        if let Some(t) = &mut self.tutorial {
            t.on_key(k.code, &mut self.cfg);
            if t.is_done() {
                self.tutorial = None;
            }
            return;
        }

        // F5 triggers a ComfyUI launch from anywhere.
        if matches!(k.code, KeyCode::F(5)) {
            self.should_launch = true;
            return;
        }

        if self.focus_menu {
            match k.code {
                KeyCode::Up => {
                    self.menu.up();
                    self.sync_screen();
                }
                KeyCode::Down => {
                    self.menu.down();
                    self.sync_screen();
                }
                KeyCode::Enter | KeyCode::Right => {
                    self.focus_menu = false;
                }
                _ => {}
            }
            return;
        }

        // Body focus: Esc returns to the menu unless a screen consumes it.
        if matches!(k.code, KeyCode::Esc) {
            // Screens with popup state (settings, extensions) eat Esc
            // first; otherwise focus returns to the menu.
            let handled = match self.screen {
                Screen::Main => self.main.eat_esc(),
                Screen::ComfySettings => self.comfy.eat_esc(),
                Screen::VersionMgmt => self.version.eat_esc(),
                Screen::LauncherSettings => self.launcher.eat_esc(),
                Screen::LauncherLogs => self.logs.eat_esc(),
                _ => false,
            };
            if !handled {
                self.focus_menu = true;
            }
            return;
        }

        match self.screen {
            Screen::Main => match k.code {
                KeyCode::Up => self.main.up(&self.cfg, &self.schema),
                KeyCode::Down => self.main.down(&self.cfg, &self.schema),
                KeyCode::Left => self.main.left(),
                KeyCode::Right => self.main.right(),
                KeyCode::PageUp => self.main.page_up(&self.cfg, &self.schema),
                KeyCode::PageDown => self.main.page_down(&self.cfg, &self.schema),
                KeyCode::Enter => {
                    let act = self.main.activate(&self.cfg, &self.schema);
                    self.apply_main_action(act);
                }
                _ => {}
            },
            Screen::ComfySettings => {
                self.comfy.on_key(k.code, &self.schema, &mut self.cfg);
                if let Some((k, m)) = self.comfy.take_flash() {
                    match k {
                        FlashKind::Info => self.set_flash(m),
                        FlashKind::Error => self.set_flash_error(m),
                    }
                }
            }
            Screen::VersionMgmt => {
                self.version.on_key(k.code, &self.cfg);
                if let Some((k, m)) = self.version.take_flash() {
                    match k {
                        FlashKind::Info => self.set_flash(m),
                        FlashKind::Error => self.set_flash_error(m),
                    }
                }
            }
            Screen::LauncherSettings => {
                self.launcher.on_key(k.code, &mut self.cfg);
                if let Some((k, m)) = self.launcher.take_flash() {
                    match k {
                        FlashKind::Info => self.set_flash(m),
                        FlashKind::Error => self.set_flash_error(m),
                    }
                }
            }
            Screen::LauncherLogs => {
                let _ = self.logs.on_key(k.code);
                if let Some((k, m)) = self.logs.take_flash() {
                    match k {
                        FlashKind::Info => self.set_flash(m),
                        FlashKind::Error => self.set_flash_error(m),
                    }
                }
            }
            Screen::About => {}
        }
    }

    /// Handles a mouse event.
    pub fn on_mouse(&mut self, m: MouseEvent) {
        if self.tutorial.is_some() {
            // Tutorial owns the full body rect tracked by the app.
            let area = self.body_inner.get();
            if let Some(t) = &mut self.tutorial {
                t.on_mouse(m, area, &mut self.cfg);
                if t.is_done() {
                    self.tutorial = None;
                }
            }
            return;
        }

        // Wheel scrolling works anywhere a list is visible; the menu pane
        // scrolls the menu, the body pane scrolls the active screen.
        match m.kind {
            MouseEventKind::ScrollUp | MouseEventKind::ScrollDown => {
                // Coalesce burst events from a single physical notch. 50ms
                // is enough to swallow smooth-scroll without making the UI
                // feel sticky: sustained human scrolling tops out at about
                // ten notches per second.
                let now = std::time::Instant::now();
                if now.duration_since(self.last_scroll.get()) < std::time::Duration::from_millis(50)
                {
                    return;
                }
                self.last_scroll.set(now);

                let delta: i32 = if matches!(m.kind, MouseEventKind::ScrollUp) {
                    -1
                } else {
                    1
                };
                let menu = self.menu_rect.get();
                if m.column >= menu.x && m.column < menu.x + menu.width {
                    if delta < 0 {
                        self.menu.up();
                    } else {
                        self.menu.down();
                    }
                    self.sync_screen();
                } else {
                    self.screen_scroll(delta);
                }
                return;
            }
            _ => {}
        }

        if !matches!(m.kind, MouseEventKind::Down(MouseButton::Left)) {
            return;
        }

        let menu = self.menu_rect.get();
        // A click on a specific menu entry selects it and hands focus to
        // the body, mirroring the keyboard Enter / Right behaviour. A
        // click on menu whitespace parks focus on the menu without
        // changing tab.
        if m.column >= menu.x
            && m.column < menu.x + menu.width
            && m.row >= menu.y
            && m.row < menu.y + menu.height
        {
            if let Some(idx) = self.menu.hit(menu, m.row) {
                self.menu.selected = idx;
                self.sync_screen();
                self.focus_menu = false;
            } else {
                self.focus_menu = true;
            }
            return;
        }

        // Click anywhere in the body: focus the body and dispatch.
        let inner = self.body_inner.get();
        if inner.width == 0 || inner.height == 0 {
            return;
        }
        if m.column < inner.x
            || m.column >= inner.x + inner.width
            || m.row < inner.y
            || m.row >= inner.y + inner.height
        {
            return;
        }

        self.focus_menu = false;
        match self.screen {
            Screen::Main => {
                let act = self.main.on_mouse(m, inner, &self.cfg, &self.schema);
                self.apply_main_action(act);
            }
            Screen::ComfySettings => {
                self.comfy.on_mouse(m, inner, &self.schema, &mut self.cfg);
                if let Some((k, msg)) = self.comfy.take_flash() {
                    match k {
                        FlashKind::Info => self.set_flash(msg),
                        FlashKind::Error => self.set_flash_error(msg),
                    }
                }
            }
            Screen::VersionMgmt => {
                self.version.on_mouse(m, inner, &self.cfg);
                if let Some((k, msg)) = self.version.take_flash() {
                    match k {
                        FlashKind::Info => self.set_flash(msg),
                        FlashKind::Error => self.set_flash_error(msg),
                    }
                }
            }
            Screen::LauncherSettings => {
                self.launcher.on_mouse(m, inner, &mut self.cfg);
                if let Some((k, msg)) = self.launcher.take_flash() {
                    match k {
                        FlashKind::Info => self.set_flash(msg),
                        FlashKind::Error => self.set_flash_error(msg),
                    }
                }
            }
            Screen::LauncherLogs => {
                self.logs.on_mouse(m, inner);
                if let Some((k, msg)) = self.logs.take_flash() {
                    match k {
                        FlashKind::Info => self.set_flash(msg),
                        FlashKind::Error => self.set_flash_error(msg),
                    }
                }
            }
            Screen::About => {}
        }
    }

    /// Returns the key-hint entries for the current state in the order
    /// Returns whether a text input widget currently has keyboard focus,
    /// meaning printable character keys should not trigger shortcuts.
    fn text_input_focused(&self) -> bool {
        if self.focus_menu {
            return false;
        }
        match self.screen {
            Screen::ComfySettings => self.comfy.view.filter_focused(),
            Screen::LauncherSettings => self.launcher.view.filter_focused(),
            Screen::VersionMgmt => self.version.text_input_focused(),
            _ => false,
        }
    }

    /// navigation, primary actions, secondary actions, then global actions.
    fn key_hints(&self) -> Vec<(&'static str, String)> {
        let mut h: Vec<(&'static str, String)> = Vec::new();
        if self.tutorial.is_some() {
            h.push(("↑↓", i18n::t("hint_pick")));
            h.push(("Enter", i18n::t("hint_next")));
            h.push(("Esc", i18n::t("hint_back")));
            h.push(("F5", i18n::t("hint_launch")));
            h.push(("Ctrl+C", i18n::t("hint_quit")));
            return h;
        }
        if self.focus_menu {
            h.push(("↑↓", i18n::t("hint_menu")));
            h.push(("Enter", i18n::t("hint_open")));
            h.push(("F5", i18n::t("hint_launch")));
            h.push(("Ctrl+C", i18n::t("hint_quit")));
            return h;
        }
        match self.screen {
            Screen::Main => {
                use crate::screens::main_launcher::MainFocus;
                h.push(("↑↓←→", i18n::t("hint_move")));
                h.push(("PgUp/PgDn", i18n::t("hint_top_bottom")));
                let label = if self.main.focus() == MainFocus::List {
                    i18n::t("hint_copy_info")
                } else {
                    i18n::t("hint_apply")
                };
                h.push(("Enter", label));
            }
            Screen::ComfySettings => {
                let tf = self.text_input_focused();
                h.push(("↑↓←→", i18n::t("hint_move")));
                h.push(("PgUp/PgDn", i18n::t("hint_top_bottom")));
                h.push(("Enter", i18n::t("hint_edit")));
                if !tf {
                    h.push(("R", i18n::t("hint_reset")));
                }
            }
            Screen::VersionMgmt => {
                let tf = self.text_input_focused();
                h.push(("↑↓←→", i18n::t("hint_move")));
                h.push(("PgUp/PgDn", i18n::t("hint_top_bottom")));
                match self.version.tab {
                    0 | 1 => {
                        h.push(("Enter", i18n::t("hint_pick_version")));
                        if !tf {
                            h.push(("R", i18n::t("hint_refresh")));
                        }
                    }
                    2 => {
                        h.push(("Enter", i18n::t("hint_pick_version")));
                        if !tf {
                            h.push(("R", i18n::t("hint_refresh")));
                            h.push(("U", i18n::t("hint_update")));
                            h.push(("D", i18n::t("hint_delete")));
                        }
                    }
                    3 => {
                        h.push(("Enter", i18n::t("hint_apply")));
                        if !tf {
                            h.push(("R", i18n::t("hint_refresh")));
                        }
                    }
                    _ => {}
                }
            }
            Screen::LauncherSettings => {
                h.push(("↑↓←→", i18n::t("hint_move")));
                h.push(("PgUp/PgDn", i18n::t("hint_top_bottom")));
                h.push(("Enter", i18n::t("hint_edit")));
            }
            Screen::LauncherLogs => {
                h.push(("↑↓", i18n::t("hint_scroll_logs")));
                h.push(("PgUp/PgDn", i18n::t("hint_page")));
                h.push(("Enter", i18n::t("hint_apply")));
            }
            Screen::About => {}
        }
        h.push(("Esc", i18n::t("hint_back")));
        h.push(("F5", i18n::t("hint_launch")));
        h.push(("Ctrl+C", i18n::t("hint_quit")));
        h
    }

    fn screen_scroll(&mut self, delta: i32) {
        match self.screen {
            Screen::Main => self.main.scroll(delta, &self.cfg, &self.schema),
            Screen::ComfySettings => self.comfy.scroll(delta, &self.schema),
            Screen::VersionMgmt => self.version.scroll(delta, &self.cfg),
            Screen::LauncherSettings => self.launcher.scroll(delta),
            Screen::LauncherLogs => {
                let code = if delta < 0 {
                    KeyCode::Up
                } else {
                    KeyCode::Down
                };
                let _ = self.logs.on_key(code);
            }
            Screen::About => {}
        }
    }

    fn apply_main_action(&mut self, act: crate::screens::main_launcher::MainAction) {
        use crate::screens::main_launcher::MainAction as A;
        match act {
            A::None => {}
            A::Launch => {
                self.should_launch = true;
            }
            A::ActivateVenv(p) => {
                self.should_activate = Some(p);
                self.should_quit = true;
            }
            A::QuitAlreadyActivated => {
                self.should_quit = true;
            }
            A::Flash(FlashKind::Info, msg) => {
                self.set_flash(msg);
            }
            A::Flash(FlashKind::Error, msg) => {
                self.set_flash_error(msg);
            }
        }
    }

    fn sync_screen(&mut self) {
        self.screen = match self.menu.selected {
            0 => Screen::Main,
            1 => Screen::ComfySettings,
            2 => Screen::VersionMgmt,
            3 => Screen::LauncherSettings,
            4 => Screen::LauncherLogs,
            _ => Screen::About,
        };
    }

    /// Replaces this process with ComfyUI and exits.
    pub fn do_launch(self) -> ! {
        let args = schema::build_cli_args(&self.schema, &self.cfg.comfy_settings);
        let env = env::build(&self.cfg.network);
        log_bus::push("launch", format!("execvp python {}", args.join(" ")));
        process::launch_comfyui_and_exit(
            std::path::Path::new(&self.cfg.general.python),
            std::path::Path::new(&self.cfg.general.comfyui_dir),
            args,
            env,
        );
    }
}
