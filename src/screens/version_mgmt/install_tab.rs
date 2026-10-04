//! Install New extensions tab.
//!
//! Combines a URL-driven install field with the searchable ComfyUI-Manager
//! extension catalog.

use super::{RepositoryAccess, TaskKind, TaskRequest, TaskResult, LIST_MAX_NUM};
use crate::app::FlashKind;
use crate::core::config::Config;
use crate::core::extension_registry::{self, InstallStatus, RegistryEntry};
use crate::core::paths::ComfyDirs;
use crate::core::{clipboard, env, git, i18n, opener, pip};
use crate::widgets::button::{Button, ButtonKind};
use crate::widgets::focus_grid::{FocusGrid, RowKind};
use crate::widgets::input::Input;
use crate::widgets::popup;
use crate::widgets::popup::notice::{Notice, NoticeCopy, NoticeOutcome};
use crate::widgets::table::{Column, Table};
use crossterm::event::KeyCode;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::Frame;
use std::cell::Cell;
use std::path::PathBuf;

/// Actions popup shown for a catalog row.
pub struct InstallActionsMenu {
    /// Catalog entry the popup belongs to.
    pub entry: RegistryEntry,
    /// Currently selected row index inside the popup.
    pub selected: usize,
    /// Whether the entry is already installed locally.
    ///
    /// When `true`, an "Update to Latest" row is prepended and the
    /// Install row is disabled.
    pub installed: bool,
    /// On-disk path of the already-installed extension, when known.
    pub installed_path: Option<PathBuf>,
}

impl InstallActionsMenu {
    /// Returns the visible rows as `(i18n_label_key, enabled)` pairs.
    ///
    /// Installed entries gain a leading "Update to Latest" row and the
    /// Install row is disabled.
    fn items(&self) -> Vec<(&'static str, bool)> {
        let mut rows: Vec<(&'static str, bool)> = Vec::new();
        if self.installed {
            rows.push(("btn_update_to_latest", self.installed_path.is_some()));
        }
        rows.push(("btn_install", !self.installed));
        rows.push(("btn_open_url", !self.entry.reference.is_empty()));
        rows
    }
}

/// Install New extensions tab state.
pub struct InstallTab {
    /// URL input.
    pub url: Input,
    /// Search input filtering the catalog.
    pub search: Input,
    /// Shared focus and catalog-selection state for the three-row layout.
    pub grid: FocusGrid,
    /// Cached catalog entries.
    pub catalog: Vec<RegistryEntry>,
    /// Whether `catalog` has been loaded.
    pub catalog_loaded: bool,
    /// Number of catalog rows displayed per frame.
    pub catalog_visible_rows: Cell<usize>,
    /// Actions popup for the selected catalog entry.
    pub actions_menu: Option<InstallActionsMenu>,
    /// Notice popup.
    pub notice: Option<Notice>,
    /// Flash message awaiting promotion to the application banner.
    pub pending_flash: Option<(FlashKind, String)>,
    /// Persistent Install button next to the URL input.
    pub btn_install: Button,
}

impl InstallTab {
    /// Drains and returns the pending flash message, if any.
    pub fn take_flash(&mut self) -> Option<(FlashKind, String)> {
        self.pending_flash.take()
    }

    /// Whether any text input widget currently has keyboard focus.
    pub fn text_input_focused(&self) -> bool {
        (self.grid.row() == 0 && self.grid.col() == 0) || self.grid.row() == 1
    }

    fn copy_to_clipboard(&mut self, s: String) {
        self.pending_flash = Some(clipboard::copy_with_flash(&s));
    }

    /// Constructs a fresh Install tab, seeding the catalog from the cache
    /// when one exists so the tab is usable instantly on first visit.
    pub fn new() -> Self {
        let cached = extension_registry::load_cache();
        let (catalog, loaded) = match cached {
            Some(v) => (v, true),
            None => (Vec::new(), false),
        };
        let mut grid = FocusGrid::new(vec![RowKind::Fixed(2), RowKind::Fixed(1), RowKind::List]);
        grid.set_focus(2, 0);
        Self {
            url: Input::default().placeholder("placeholder_install_url"),
            search: Input::default().placeholder("placeholder_search"),
            grid,
            catalog,
            catalog_loaded: loaded,
            catalog_visible_rows: Cell::new(0),
            actions_menu: None,
            notice: None,
            pending_flash: None,
            btn_install: Button::new(ButtonKind::Primary),
        }
    }

    /// Polled by `VersionMgmt::tick`.
    ///
    /// Returns a deferred install request when the Install button has
    /// rendered its highlight frame, and drains the notice popup's
    /// deferred-fire pipeline along the way.
    pub fn poll_button_action(&mut self, cfg: &Config) -> Option<TaskRequest> {
        if let Some(n) = &mut self.notice {
            match n.tick() {
                Some(NoticeOutcome::Close) => {
                    self.notice = None;
                }
                Some(NoticeOutcome::Copy(s)) => {
                    self.notice = None;
                    self.copy_to_clipboard(s);
                }
                None => {}
            }
        }
        if self.btn_install.poll_fire() {
            return self.do_url_install(cfg);
        }
        None
    }

    /// Clamps the catalog scroll offset so the selected row is visible.
    pub fn ensure_visible(&mut self) {
        self.grid.set_list_len(self.filtered().len());
        self.grid
            .set_visible_rows(self.catalog_visible_rows.get().max(1));
        self.grid.ensure_visible();
    }

    /// Filters catalog rows using a case-insensitive substring match on
    /// the title or description.
    fn filtered(&self) -> Vec<&RegistryEntry> {
        let needle = self.search.value.to_lowercase();
        if needle.is_empty() {
            return self.catalog.iter().collect();
        }
        self.catalog
            .iter()
            .filter(|e| {
                e.title.to_lowercase().contains(&needle)
                    || e.description.to_lowercase().contains(&needle)
            })
            .collect()
    }

    /// Returns `[x]` when the extension is present on disk and `[ ]`
    /// otherwise; disabled extensions still count as present.
    fn installed_marker(s: InstallStatus) -> &'static str {
        match s {
            InstallStatus::Installed | InstallStatus::Disabled => "[x]",
            InstallStatus::NotInstalled => "[ ]",
        }
    }

    /// Renders the tab into `area`.
    pub fn render(
        &self,
        f: &mut Frame,
        area: Rect,
        cfg: &Config,
        installed: &[super::extensions_tab::Extension],
        body_active: bool,
    ) {
        let v = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(3), // URL + Install
                Constraint::Length(3), // Search
                Constraint::Min(0),    // Catalog
            ])
            .split(area);

        let popup_open = self.actions_menu.is_some() || self.notice.is_some();
        let active = body_active && !popup_open;

        // URL + Install
        let install_w = (crate::core::text::width(&i18n::t("btn_install")) as u16)
            .saturating_add(6)
            .max(12);
        let url_h = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Min(0), Constraint::Length(install_w)])
            .split(v[0]);
        self.url.render(
            f,
            url_h[0],
            self.grid.row() == 0 && self.grid.col() == 0 && active,
        );
        self.btn_install.render(
            f,
            url_h[1],
            &i18n::t("btn_install"),
            self.grid.row() == 0 && self.grid.col() == 1 && active,
        );

        // Search
        self.search.render(f, v[1], self.grid.row() == 1 && active);

        // Catalog
        self.catalog_visible_rows
            .set((v[2].height as usize).saturating_sub(3));
        let filtered = self.filtered();
        // Installed column is a fixed-width 3-char marker plus the header
        // word "Installed" (i18n may make this wider) → reserve 11.
        let installed_w: u16 = (crate::core::text::width(&i18n::t("label_installed")) as u16)
            .saturating_add(2)
            .max(11);
        let side: u16 = installed_w + 28 /*Name*/ + 3 /*gaps*/ + 2 /*border*/;
        let desc_w = v[2].width.saturating_sub(side).max(20);
        let cols = vec![
            Column {
                title: i18n::t("label_installed"),
                width: installed_w,
            },
            Column {
                title: i18n::t("label_name"),
                width: 28,
            },
            Column {
                title: i18n::t("label_description"),
                width: desc_w,
            },
        ];
        Table {
            columns: &cols,
            row_count: filtered.len(),
            selected: self.grid.list_selected(),
            scroll: self.grid.list_scroll(),
        }
        .render(
            f,
            v[2],
            |i| {
                let e = filtered[i];
                let st = extension_registry::status_for(e, installed);
                vec![
                    Self::installed_marker(st).to_string(),
                    e.title.clone(),
                    e.description.clone(),
                ]
            },
            active && self.grid.row() == 2,
        );

        if let Some(am) = &self.actions_menu {
            self.render_actions(f, area, am);
        }
        if let Some(n) = &self.notice {
            n.render(f, area);
        }
        let _ = cfg;
    }

    /// Actions popup for the selected catalog entry. Delegates the visual
    /// rendering to the shared [`popup::menu::PopupMenu`] widget; the
    /// per-row dispatch logic stays on `InstallTab`.
    fn render_actions(&self, f: &mut Frame, area: Rect, am: &InstallActionsMenu) {
        let title = i18n::t_args("popup_actions_title", &[("name", &am.entry.title)]);
        let items: Vec<popup::menu::MenuItem> = am
            .items()
            .into_iter()
            .map(|(label_key, enabled)| popup::menu::MenuItem { label_key, enabled })
            .collect();
        popup::menu::PopupMenu::new(title, items, am.selected).render(f, area);
    }

    /// Attempts to handle a Left arrow within this tab.
    /// Returns `true` if the key was consumed.
    pub fn on_left(&mut self) -> bool {
        if self.grid.row() == 0 && self.grid.col() == 0 && !self.url.at_start() {
            self.url.on_key(KeyCode::Left);
            return true;
        }
        if self.grid.row() == 1 && !self.search.at_start() {
            self.search.on_key(KeyCode::Left);
            return true;
        }
        self.grid.move_left()
    }

    /// Attempts to handle a Right arrow within this tab.
    /// Returns `true` if the key was consumed.
    pub fn on_right(&mut self) -> bool {
        if self.grid.row() == 0 && self.grid.col() == 0 && !self.url.at_end() {
            self.url.on_key(KeyCode::Right);
            return true;
        }
        if self.grid.row() == 1 && !self.search.at_end() {
            self.search.on_key(KeyCode::Right);
            return true;
        }
        self.grid.move_right()
    }

    /// Handles a key event.
    pub fn on_key(
        &mut self,
        code: KeyCode,
        cfg: &Config,
        installed: &[super::extensions_tab::Extension],
    ) -> Option<TaskRequest> {
        // Notice popup — Tab/L/R toggles button focus; Enter activates the
        // focused button; Esc closes. Copy button writes the payload to the
        // clipboard and queues a top-right flash.
        if let Some(n) = &mut self.notice {
            // Esc closes immediately; Enter arms the focused button (Copy
            // outcome arrives via `poll_button_action` → `n.tick()`).
            if matches!(n.on_key(code), Some(NoticeOutcome::Close)) {
                self.notice = None;
            }
            return None;
        }
        // Actions popup: intercept before everything else.
        if let Some(am) = &mut self.actions_menu {
            let n = am.items().len();
            match code {
                KeyCode::Esc => {
                    self.actions_menu = None;
                    return None;
                }
                KeyCode::Up => {
                    am.selected = if am.selected == 0 {
                        n - 1
                    } else {
                        am.selected - 1
                    };
                    return None;
                }
                KeyCode::Down => {
                    am.selected = (am.selected + 1) % n;
                    return None;
                }
                KeyCode::PageUp => {
                    am.selected = 0;
                    return None;
                }
                KeyCode::PageDown => {
                    if n > 0 {
                        am.selected = n - 1;
                    }
                    return None;
                }
                KeyCode::Enter => {
                    let items = am.items();
                    let (key, enabled) = items[am.selected];
                    if !enabled {
                        return None;
                    }
                    return self.dispatch_action(key, cfg);
                }
                _ => return None,
            }
        }
        let n = self.filtered().len();
        self.grid.set_list_len(n);
        self.grid
            .set_visible_rows(self.catalog_visible_rows.get().max(1));

        // Page keys navigate the same page topology as the arrow keys.  Keep
        // this ahead of text-input handling so Input cannot swallow them.
        match code {
            KeyCode::PageUp => {
                self.grid.page_up();
                return None;
            }
            KeyCode::PageDown => {
                self.grid.page_down();
                return None;
            }
            _ => {}
        }

        // Text inputs intercept editing keys only after page navigation.
        match (self.grid.row(), self.grid.col()) {
            (0, 0) => match code {
                KeyCode::Tab => self.grid.set_focus(0, 1),
                KeyCode::BackTab => self.grid.set_focus(2, 0),
                KeyCode::Enter => return self.do_url_install(cfg),
                KeyCode::Up => self.grid.move_up(),
                KeyCode::Down => self.grid.move_down(),
                k if !matches!(k, KeyCode::Left | KeyCode::Right) => self.url.on_key(k),
                _ => {}
            },
            (0, 1) => match code {
                KeyCode::Tab => self.grid.set_focus(1, 0),
                KeyCode::BackTab => self.grid.set_focus(0, 0),
                KeyCode::Enter => return self.do_url_install(cfg),
                KeyCode::Up => self.grid.move_up(),
                KeyCode::Down => self.grid.move_down(),
                _ => {}
            },
            (1, _) => match code {
                KeyCode::Tab => self.grid.set_focus(2, 0),
                KeyCode::Enter => {
                    self.grid.set_focus(2, 0);
                    self.grid.set_list_selected(0);
                    self.grid.set_list_scroll(0);
                }
                KeyCode::BackTab => self.grid.set_focus(0, 1),
                KeyCode::Up => self.grid.move_up(),
                KeyCode::Down => self.grid.move_down(),
                k if !matches!(k, KeyCode::Left | KeyCode::Right) => {
                    self.search.on_key(k);
                    self.grid.set_list_selected(0);
                    self.grid.set_list_scroll(0);
                }
                _ => {}
            },
            (2, _) => match code {
                KeyCode::Tab => self.grid.set_focus(0, 0),
                KeyCode::BackTab => self.grid.set_focus(1, 0),
                KeyCode::Up if n > 0 => self.grid.move_up(),
                KeyCode::Down if n > 0 => self.grid.move_down(),
                KeyCode::Enter => return self.do_catalog_install(cfg, installed),
                KeyCode::Char('r') | KeyCode::Char('R') => {
                    let env_vars = env::build(&cfg.network);
                    return Some(fetch_registry_request(env_vars));
                }
                _ => {}
            },
            _ => {}
        }
        None
    }

    /// Handles a mouse event.
    pub fn on_mouse(
        &mut self,
        m: crossterm::event::MouseEvent,
        area: Rect,
        cfg: &Config,
        installed: &[super::extensions_tab::Extension],
    ) -> Option<TaskRequest> {
        if !matches!(
            m.kind,
            crossterm::event::MouseEventKind::Down(crossterm::event::MouseButton::Left)
        ) {
            return None;
        }
        if let Some(n) = &mut self.notice {
            // Click outside closes immediately; OK / Copy arm Buttons.
            if matches!(n.on_mouse(m, area), Some(NoticeOutcome::Close)) {
                self.notice = None;
            }
            return None;
        }
        if let Some(am_ref) = &self.actions_menu {
            let h = (am_ref.items().len() as u16)
                .saturating_add(2)
                .min(area.height);
            let r = popup::center(area, 50, h);
            let inside = m.column >= r.x
                && m.column < r.x + r.width
                && m.row >= r.y
                && m.row < r.y + r.height;
            if !inside {
                self.actions_menu = None;
                return None;
            }
            if m.column > r.x
                && m.column < r.x + r.width.saturating_sub(1)
                && m.row > r.y
                && m.row < r.y + r.height.saturating_sub(1)
            {
                let idx = (m.row - (r.y + 1)) as usize;
                if let Some(am) = &mut self.actions_menu {
                    let items = am.items();
                    if idx < items.len() {
                        if idx != am.selected {
                            am.selected = idx;
                            return None;
                        }
                        let (key, enabled) = items[idx];
                        if !enabled {
                            return None;
                        }
                        return self.dispatch_action(key, cfg);
                    }
                }
            }
            return None;
        }
        let v = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(3),
                Constraint::Length(3),
                Constraint::Min(0),
            ])
            .split(area);
        let install_w = (crate::core::text::width(&i18n::t("btn_install")) as u16)
            .saturating_add(6)
            .max(12);
        let url_h = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Min(0), Constraint::Length(install_w)])
            .split(v[0]);

        let inside = |r: Rect| {
            m.column >= r.x && m.column < r.x + r.width && m.row >= r.y && m.row < r.y + r.height
        };

        if inside(url_h[1]) {
            self.grid.set_focus(0, 1);
            self.btn_install.click();
            return None;
        }
        if inside(url_h[0]) {
            self.grid.set_focus(0, 0);
            return None;
        }
        if inside(v[1]) {
            self.grid.set_focus(1, 0);
            return None;
        }
        if !inside(v[2]) {
            return None;
        }

        // Catalog click: select then activate on the second click.
        if m.row < v[2].y + 2 {
            return None;
        }
        let rel = (m.row - (v[2].y + 2)) as usize;
        let filtered = self.filtered();
        let idx = self.grid.list_scroll() + rel;
        if idx >= filtered.len() {
            return None;
        }
        if self.grid.row() != 2 || idx != self.grid.list_selected() {
            self.grid.set_focus(2, 0);
            self.grid.set_list_selected(idx);
            self.ensure_visible();
            return None;
        }
        self.do_catalog_install(cfg, installed)
    }

    /// Handles a wheel-scroll event on the catalog.
    pub fn scroll(&mut self, delta: i32) {
        let n = self.filtered().len();
        self.grid.set_list_len(n);
        self.grid
            .set_visible_rows(self.catalog_visible_rows.get().max(1));
        self.grid.scroll(delta);
    }

    fn do_url_install(&mut self, cfg: &Config) -> Option<TaskRequest> {
        let url = self.url.value.trim().to_string();
        if !is_valid_git_url(&url) {
            self.pending_flash = Some((FlashKind::Error, i18n::t("install_invalid_url")));
            return None;
        }
        let root = PathBuf::from(&cfg.general.comfyui_dir);
        let dest_dir = ComfyDirs::new(&root).custom_nodes();
        let name = url
            .rsplit('/')
            .next()
            .unwrap_or("ext")
            .trim_end_matches(".git")
            .to_string();
        let dest = dest_dir.join(&name);
        let env_vars = env::build(&cfg.network);
        let python = cfg.general.python.clone();
        self.url.value.clear();
        self.url.cursor = 0;
        Some(install_request(url, dest, root, env_vars, python))
    }

    /// Open the per-row Actions popup for the currently-selected catalog
    /// entry, instead of installing directly. Mirrors the Extensions tab.
    fn do_catalog_install(
        &mut self,
        _cfg: &Config,
        installed: &[super::extensions_tab::Extension],
    ) -> Option<TaskRequest> {
        let filtered = self.filtered();
        let entry = filtered.get(self.grid.list_selected())?;
        let st = extension_registry::status_for(entry, installed);
        let is_installed = !matches!(st, InstallStatus::NotInstalled);
        // For an installed entry, find the local path by URL match (same
        // normalisation rule as `status_for`) so `Update to Latest` knows
        // which dir to operate on.
        let installed_path = if is_installed {
            let want = normalize_url(&entry.reference);
            installed
                .iter()
                .find(|e| e.managed && normalize_url(&e.remote) == want)
                .map(|e| e.path.clone())
        } else {
            None
        };
        self.actions_menu = Some(InstallActionsMenu {
            entry: (*entry).clone(),
            selected: 0,
            installed: is_installed,
            installed_path,
        });
        None
    }

    /// Execute the selected action in the install-tab actions menu.
    fn dispatch_action(&mut self, key: &'static str, cfg: &Config) -> Option<TaskRequest> {
        let am = self.actions_menu.take()?;
        match key {
            "btn_update_to_latest" => {
                let path = am.installed_path?;
                let name = path
                    .file_name()
                    .and_then(|s| s.to_str())
                    .unwrap_or("ext")
                    .to_string();
                let root = PathBuf::from(&cfg.general.comfyui_dir);
                let env_vars = env::build(&cfg.network);
                let python = cfg.general.python.clone();
                Some(super::extensions_tab::update_one_request(
                    path, name, root, env_vars, python,
                ))
            }
            "btn_install" => {
                let url = am.entry.reference.clone();
                let root = PathBuf::from(&cfg.general.comfyui_dir);
                let dest_dir = ComfyDirs::new(&root).custom_nodes();
                let name = url
                    .rsplit('/')
                    .next()
                    .unwrap_or("ext")
                    .trim_end_matches(".git")
                    .to_string();
                let dest = dest_dir.join(&name);
                let env_vars = env::build(&cfg.network);
                let python = cfg.general.python.clone();
                Some(install_request(url, dest, root, env_vars, python))
            }
            "btn_open_url" => {
                if !opener::open_url(&am.entry.reference) {
                    let url = am.entry.reference.clone();
                    self.notice = Some(Notice::new(
                        i18n::t("btn_open_url"),
                        i18n::t_args("popup_url_shown", &[("url", &url)]),
                        Some(NoticeCopy {
                            label_key: "btn_copy_url",
                            payload: url,
                            focused: false,
                        }),
                    ));
                }
                None
            }
            _ => None,
        }
    }
}

fn install_request(
    url: String,
    dest: PathBuf,
    root: PathBuf,
    env_vars: std::collections::HashMap<String, String>,
    python: String,
) -> TaskRequest {
    let title = i18n::t_args("task_install", &[("url", &url)]);
    TaskRequest {
        title,
        then: TaskKind::None,
        is_refresh: false,
        changes_repository: true,
        repository_access: RepositoryAccess::Exclusive,
        ext_scan_root: None,
        work: Box::new(move |tx| {
            super::warn_if_storage_low(&tx, &dest);
            if !git::clone(&url, &dest, env_vars.clone()).unwrap_or(false) {
                return super::TaskOutcome::failure(
                    &url,
                    "git clone",
                    "clone failed; dependency installation was skipped; see the task log",
                );
            }
            let mut failures = Vec::new();
            if let Err(error) =
                pip::install_custom_node(std::path::Path::new(&python), &root, &dest, env_vars)
            {
                failures.push(super::ItemFailure::new(
                    &url,
                    error.stage.label(),
                    error.detail("extension was cloned, but post-install processing failed"),
                ));
            }
            if let Some(ext) = super::extensions_tab::read_one_local(&dest) {
                let _ = tx.send(TaskResult::ExtRowAdd { ext });
            } else {
                failures.push(super::ItemFailure::new(
                    &url,
                    "read state",
                    "extension was cloned, but its state could not be read",
                ));
            }
            if failures.is_empty() {
                super::TaskOutcome::Success
            } else {
                super::TaskOutcome::PartialFailure(failures)
            }
        }),
    }
}

/// Builds a background task that fetches the official extension catalog
/// from GitHub.
pub fn fetch_registry_request(env_vars: std::collections::HashMap<String, String>) -> TaskRequest {
    TaskRequest {
        title: i18n::t("task_registry_fetch"),
        then: TaskKind::None,
        is_refresh: true,
        changes_repository: false,
        repository_access: RepositoryAccess::None,
        ext_scan_root: None,
        work: Box::new(move |tx| {
            match extension_registry::fetch_blocking(&env_vars) {
                Ok(entries) => {
                    extension_registry::save_cache(&entries);
                    let _ = tx.send(TaskResult::RegistryData { entries });
                }
                Err(e) => {
                    crate::core::log_bus::push("registry", format!("fetch failed: {e}"));
                    if let Some(cached) = extension_registry::load_cache() {
                        let _ = tx.send(TaskResult::RegistryData { entries: cached });
                    } else {
                        return super::TaskOutcome::failure(
                            "extension registry",
                            "fetch",
                            e.to_string(),
                        );
                    }
                }
            }
            super::TaskOutcome::Success
        }),
    }
}

// Reference `LIST_MAX_NUM` so the unused-import warning stays silent
// until limit-driven catalog pagination lands.
const _: usize = LIST_MAX_NUM;

/// Normalises a URL the same way `extension_registry::status_for` does:
/// lower-cases, drops the scheme prefix, the trailing slash, and the
/// `.git` suffix.
fn normalize_url(s: &str) -> String {
    let s = s.trim().to_lowercase();
    let s = s
        .strip_prefix("https://")
        .or_else(|| s.strip_prefix("http://"))
        .map(str::to_string)
        .unwrap_or(s);
    let s = s.strip_suffix('/').map(str::to_string).unwrap_or(s);
    let s = s.strip_suffix(".git").map(str::to_string).unwrap_or(s);
    s
}

/// Permissive Git URL sniffer. Accepts:
///   - Anything ending in `.git` (case-insensitive).
///   - `https://` / `http://` URLs hosted on common Git providers
///     (github.com, gitlab.com, bitbucket.org, codeberg.org, gitee.com,
///     git.sr.ht, sourcehut, gitea-style hosts where the path looks like
///     `<user>/<repo>`).
///   - `git@<host>:<user>/<repo>` SSH shorthand.
///   - `ssh://…`, `git://…` schemes.
///     Rejects empty strings, plain prose, random URLs, ftp / file schemes.
fn is_valid_git_url(s: &str) -> bool {
    let s = s.trim();
    if s.is_empty() {
        return false;
    }
    let lower = s.to_lowercase();
    if lower.ends_with(".git") {
        return true;
    }
    if lower.starts_with("git@") && s.contains(':') {
        return true;
    }
    if lower.starts_with("ssh://") || lower.starts_with("git://") {
        return true;
    }
    if lower.starts_with("https://") || lower.starts_with("http://") {
        const KNOWN_HOSTS: &[&str] = &[
            "github.com",
            "gitlab.com",
            "bitbucket.org",
            "codeberg.org",
            "gitee.com",
            "git.sr.ht",
            "sr.ht",
        ];
        // Strip scheme so we can match host[:port]/...
        let rest = s.split_once("://").map(|x| x.1).unwrap_or("");
        let host = rest.split('/').next().unwrap_or("");
        let host_no_port = host.split(':').next().unwrap_or("");
        if KNOWN_HOSTS
            .iter()
            .any(|h| host_no_port.eq_ignore_ascii_case(h))
        {
            // Require a `<user>/<repo>` path so `https://github.com/` alone fails.
            let path = rest.split_once('/').map(|x| x.1).unwrap_or("");
            let segs: Vec<&str> = path.split('/').filter(|p| !p.is_empty()).collect();
            return segs.len() >= 2;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(title: &str) -> RegistryEntry {
        RegistryEntry {
            title: title.into(),
            author: String::new(),
            reference: format!("https://github.com/example/{title}"),
            description: String::new(),
        }
    }

    #[test]
    fn page_down_works_after_page_up_focuses_url_input() {
        let mut tab = InstallTab::new();
        tab.catalog = vec![entry("one"), entry("two"), entry("three")];
        tab.catalog_visible_rows.set(2);

        tab.on_key(KeyCode::PageUp, &Config::default(), &[]);
        assert_eq!((tab.grid.row(), tab.grid.col()), (0, 0));

        tab.on_key(KeyCode::PageDown, &Config::default(), &[]);
        assert_eq!((tab.grid.row(), tab.grid.list_selected()), (2, 2));
    }
}
