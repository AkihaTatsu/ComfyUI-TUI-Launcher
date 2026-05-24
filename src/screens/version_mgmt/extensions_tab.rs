//! Installed extensions tab.
//!
//! Lists extensions under `custom_nodes/`, supports per-row actions
//! (change version, update, enable / disable, uninstall, open URL), and
//! offers update-all and reinstall-all bulk operations.

use super::{core_tab, TaskKind, TaskRequest, TaskResult, LIST_MAX_NUM};
use crate::app::FlashKind;
use crate::core::config::Config;
use crate::core::paths::ComfyDirs;
use crate::core::{clipboard, env, git, i18n, opener, pip, theme};
use crate::widgets::input::Input;
use crate::widgets::popup;
use crate::widgets::popup::confirm::Confirm;
use crate::widgets::popup::notice::{Notice, NoticeCopy, NoticeOutcome};
use crate::widgets::table::{Column, Table};
use crossterm::event::KeyCode;
use ratatui::layout::Rect;
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;
use std::cell::Cell;
use std::path::{Path, PathBuf};

/// Modal popup state listing one extension's commit history so the user
/// can switch its version.
pub struct VersionPicker {
    /// Path of the extension.
    pub ext_path: PathBuf,
    /// Display name of the extension.
    pub ext_name: String,
    /// Loaded commit list, newest first.
    pub commits: Vec<git::Commit>,
    /// Short SHA of the extension's current `HEAD`.
    pub current: Option<String>,
    /// Selected row.
    pub selected: usize,
    /// Scroll offset.
    pub scroll: usize,
    /// Row count requested for the current `commits` snapshot.
    pub limit: usize,
    /// Whether the last load returned fewer rows than requested.
    pub end_reached: bool,
    /// Number of data rows displayed per frame.
    pub visible_rows: Cell<usize>,
}

impl VersionPicker {
    /// Clamps the scroll offset so the selected row is on-screen.
    pub fn ensure_visible(&mut self) {
        let v = self.visible_rows.get().max(1);
        if self.selected < self.scroll {
            self.scroll = self.selected;
        }
        let max_off = self.selected.saturating_sub(v - 1);
        if self.scroll < max_off {
            self.scroll = max_off;
        }
        if self.scroll > self.selected {
            self.scroll = self.selected;
        }
    }
}

/// Action menu opened by Enter on an extension row.
pub struct ActionsMenu {
    /// Path of the extension.
    pub ext_path: PathBuf,
    /// Display name of the extension.
    pub ext_name: String,
    /// Remote origin URL of the extension.
    pub remote: String,
    /// Whether the extension is currently disabled.
    pub disabled: bool,
    /// Whether the extension is managed by git (has a remote).
    pub managed: bool,
    /// Selected row index inside the popup.
    pub selected: usize,
}

impl ActionsMenu {
    /// Returns the rows in display order as `(i18n_label_key, enabled)`
    /// pairs. The menu always renders every entry so the layout stays
    /// stable; disabled rows are still drawn but Enter on them is a no-op.
    fn items(&self) -> [(&'static str, bool); 5] {
        [
            ("btn_update_to_latest", self.managed),
            ("btn_change_version", self.managed),
            ("btn_open_url", !self.remote.is_empty()),
            (
                if self.disabled {
                    "btn_enable"
                } else {
                    "btn_disable"
                },
                true,
            ),
            ("btn_uninstall", true),
        ]
    }
}

/// One installed extension row.
#[derive(Clone)]
pub struct Extension {
    /// Display name with the `.disabled` suffix stripped.
    pub name: String,
    /// On-disk path; may carry the `.disabled` suffix.
    pub path: PathBuf,
    /// Whether the extension is currently disabled.
    pub disabled: bool,
    /// Whether the extension is managed by git (has a remote).
    pub managed: bool,
    /// Remote origin URL.
    pub remote: String,
    /// Current branch.
    pub branch: String,
    /// Short SHA of the current `HEAD`.
    pub head: String,
    /// Date of the current `HEAD` commit.
    pub head_date: String,
    /// Number of commits the configured upstream is ahead of local `HEAD`.
    ///
    /// Non-zero values mark rows as having available updates. Computed
    /// without contacting the network, so the value reflects whatever
    /// the last fetch produced.
    pub behind: u32,
}

/// Which control on the Extensions tab currently holds focus.
#[derive(Copy, Clone, PartialEq, Eq)]
pub enum ExtFocus {
    /// Search input.
    Search,
    /// Update All button.
    UpdateAll,
    /// Reinstall All button.
    ReinstallAll,
    /// Extensions table.
    Table,
}

/// Installed extensions tab state.
pub struct ExtensionsTab {
    /// All loaded extensions.
    pub items: Vec<Extension>,
    /// Index into the filtered view; see [`filtered_indices`](Self::filtered_indices).
    pub selected: usize,
    /// Scroll offset into the filtered view.
    pub scroll: usize,
    /// Confirmation popup.
    pub confirm: Option<Confirm>,
    /// Notice popup.
    pub notice: Option<Notice>,
    /// Pending delete target, awaiting confirmation.
    pub pending_delete: Option<PathBuf>,
    /// Path the current data was loaded for.
    pub loaded_for: Option<PathBuf>,
    /// Active version picker popup.
    pub version_picker: Option<VersionPicker>,
    /// Active actions popup.
    pub actions_menu: Option<ActionsMenu>,
    /// Row count requested for the current `items` snapshot.
    pub limit: usize,
    /// Whether the last load returned fewer rows than requested.
    pub end_reached: bool,
    /// Number of data rows displayed per frame.
    pub visible_rows: Cell<usize>,
    /// Search input filtering the table.
    pub search: Input,
    /// Currently focused control.
    pub focus: ExtFocus,
    /// Last focused column in row 0 (Search/UpdateAll/ReinstallAll), restored
    /// when navigating back up from the table.
    pub last_row0_focus: ExtFocus,
    /// Flash message awaiting promotion to the application banner.
    pub pending_flash: Option<(FlashKind, String)>,
    /// Update All bulk-action button.
    pub btn_update_all: crate::widgets::button::Button,
    /// Reinstall All bulk-action button.
    pub btn_reinstall_all: crate::widgets::button::Button,
}

impl ExtensionsTab {
    /// Drains and returns the pending flash message, if any.
    pub fn take_flash(&mut self) -> Option<(FlashKind, String)> {
        self.pending_flash.take()
    }

    /// Whether any text input widget currently has keyboard focus.
    pub fn text_input_focused(&self) -> bool {
        self.focus == ExtFocus::Search
    }

    fn copy_to_clipboard(&mut self, s: String) {
        self.pending_flash = Some(clipboard::copy_with_flash(&s));
    }

    /// Constructs a fresh Extensions tab.
    pub fn new() -> Self {
        Self {
            items: vec![],
            selected: 0,
            scroll: 0,
            confirm: None,
            notice: None,
            pending_delete: None,
            loaded_for: None,
            pending_flash: None,
            version_picker: None,
            actions_menu: None,
            limit: LIST_MAX_NUM,
            end_reached: false,
            visible_rows: Cell::new(0),
            search: Input::default().placeholder("placeholder_search"),
            focus: ExtFocus::Table,
            last_row0_focus: ExtFocus::Search,
            btn_update_all: crate::widgets::button::Button::new(
                crate::widgets::button::ButtonKind::Primary,
            ),
            btn_reinstall_all: crate::widgets::button::Button::new(
                crate::widgets::button::ButtonKind::Primary,
            ),
        }
    }

    /// Polled by `VersionMgmt::tick` once per frame. Returns the deferred
    /// TaskRequest if either action button OR a popup button is ready to
    /// fire (the Button widget's own `pending` slot decides — gives one
    /// full frame of focus highlight before the request runs).
    /// Polled by `VersionMgmt::tick` to drain deferred button presses.
    pub fn poll_button_action(&mut self, cfg: &Config) -> Option<TaskRequest> {
        // Confirm popup (Uninstall y/n).
        if let Some(c) = &mut self.confirm {
            match c.tick() {
                Some(true) => {
                    self.confirm = None;
                    let path = self.pending_delete.take();
                    if let Some(p) = path {
                        let name = p
                            .file_name()
                            .and_then(|s| s.to_str())
                            .unwrap_or("?")
                            .to_string();
                        let root = PathBuf::from(&cfg.general.comfyui_dir);
                        return Some(uninstall_request(p, root, name));
                    }
                }
                Some(false) => {
                    self.confirm = None;
                    self.pending_delete = None;
                }
                None => {}
            }
        }
        // Notice popup (URL-fallback "Cannot open browser" with Copy URL).
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
        if self.btn_update_all.poll_fire() {
            let items = self.items.clone();
            let env_vars = env::build(&cfg.network);
            let python = cfg.general.python.clone();
            let root = PathBuf::from(&cfg.general.comfyui_dir);
            return Some(update_all_request(items, root, env_vars, python));
        }
        if self.btn_reinstall_all.poll_fire() {
            let items = self.items.clone();
            let env_vars = env::build(&cfg.network);
            let python = cfg.general.python.clone();
            let root = PathBuf::from(&cfg.general.comfyui_dir);
            return Some(reinstall_all_request(items, root, env_vars, python));
        }
        None
    }

    /// Map each visible row back to an index in `self.items` using a
    /// case-insensitive substring match on name or remote URL.
    /// Returns indices of `items` matching the current search input.
    pub fn filtered_indices(&self) -> Vec<usize> {
        let needle = self.search.value.to_lowercase();
        if needle.is_empty() {
            return (0..self.items.len()).collect();
        }
        self.items
            .iter()
            .enumerate()
            .filter(|(_, e)| {
                e.name.to_lowercase().contains(&needle) || e.remote.to_lowercase().contains(&needle)
            })
            .map(|(i, _)| i)
            .collect()
    }

    /// Returns the real `items` index for the currently selected row.
    fn current_real_idx(&self) -> Option<usize> {
        self.filtered_indices().get(self.selected).copied()
    }

    /// Clamps the scroll offset so the selected row is on-screen.
    pub fn ensure_visible(&mut self) {
        let v = self.visible_rows.get().max(1);
        if self.selected < self.scroll {
            self.scroll = self.selected;
        }
        let max_off = self.selected.saturating_sub(v - 1);
        if self.scroll < max_off {
            self.scroll = max_off;
        }
        if self.scroll > self.selected {
            self.scroll = self.selected;
        }
    }

    /// Renders the tab into `area`.
    pub fn render(&self, f: &mut Frame, area: Rect, _cfg: &Config, body_active: bool) {
        let v = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(3), Constraint::Min(0)])
            .split(area);

        // With no popup the body owns focus; otherwise defocus
        // everything beneath the popup so only the popup looks active.
        let popup_open = self.actions_menu.is_some()
            || self.version_picker.is_some()
            || self.confirm.is_some()
            || self.notice.is_some();
        let active = body_active && !popup_open;

        // Top row: search input (flex) + Update All + Reinstall All buttons.
        let upd_w = update_all_button_width();
        let rei_w = reinstall_all_button_width();
        let top = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Min(0),
                Constraint::Length(upd_w),
                Constraint::Length(rei_w),
            ])
            .split(v[0]);
        self.search
            .render(f, top[0], self.focus == ExtFocus::Search && active);
        self.btn_update_all.render(
            f,
            top[1],
            &i18n::t("btn_update_all"),
            self.focus == ExtFocus::UpdateAll && active,
        );
        self.btn_reinstall_all.render(
            f,
            top[2],
            &i18n::t("btn_reinstall_all"),
            self.focus == ExtFocus::ReinstallAll && active,
        );

        let table_area = v[1];
        let side: u16 = 8 /*Enabled*/ + 26 /*Name*/ + 10 /*Branch*/ + 10 /*Version ID*/ + 20 /*Date*/
            + 6 /*one space after each of the 6 cols*/
            + 2 /*table border*/;
        let remote_w = table_area.width.saturating_sub(side).max(20);
        let cols = vec![
            Column {
                title: i18n::t("label_enabled"),
                width: 8,
            },
            Column {
                title: i18n::t("label_name"),
                width: 26,
            },
            Column {
                title: i18n::t("label_remote"),
                width: remote_w,
            },
            Column {
                title: i18n::t("label_branch"),
                width: 10,
            },
            Column {
                title: i18n::t("label_version_id"),
                width: 10,
            },
            Column {
                title: i18n::t("label_date"),
                width: 20,
            },
        ];
        self.visible_rows
            .set((table_area.height as usize).saturating_sub(3));
        let items = &self.items;
        let filtered = self.filtered_indices();
        Table {
            columns: &cols,
            row_count: filtered.len(),
            selected: self.selected,
            scroll: self.scroll,
        }
        .render_styled(
            f,
            table_area,
            |i| {
                let e = &items[filtered[i]];
                vec![
                    (if e.disabled { "no" } else { "yes" }).into(),
                    e.name.clone(),
                    e.remote.clone(),
                    e.branch.clone(),
                    e.head.clone(),
                    e.head_date.clone(),
                ]
            },
            |i| {
                if items[filtered[i]].behind > 0 {
                    Some(
                        ratatui::style::Style::default()
                            .fg(ratatui::style::Color::Yellow)
                            .add_modifier(ratatui::style::Modifier::BOLD),
                    )
                } else {
                    None
                }
            },
            active && self.focus == ExtFocus::Table,
        );

        if let Some(vp) = &self.version_picker {
            self.render_picker(f, area, vp);
        }
        if let Some(am) = &self.actions_menu {
            self.render_actions(f, area, am);
        }
        if let Some(c) = &self.confirm {
            c.render(f, area);
        }
        if let Some(n) = &self.notice {
            n.render(f, area);
        }
    }

    fn render_actions(&self, f: &mut Frame, area: Rect, am: &ActionsMenu) {
        let title = i18n::t_args("popup_actions_title", &[("name", &am.ext_name)]);
        let items: Vec<popup::menu::MenuItem> = am
            .items()
            .into_iter()
            .map(|(label_key, enabled)| popup::menu::MenuItem { label_key, enabled })
            .collect();
        popup::menu::PopupMenu::new(title, items, am.selected).render(f, area);
    }

    fn render_picker(&self, f: &mut Frame, area: Rect, vp: &VersionPicker) {
        let r = popup::center(
            area,
            area.width.saturating_sub(6).min(100),
            area.height.saturating_sub(4).min(24),
        );
        popup::clear_widechar_safe(f, r);
        let block = Block::default()
            .borders(Borders::ALL)
            .border_type(theme::border_type(true))
            .border_style(theme::accent())
            .title(format!(
                " {}: {} ",
                i18n::t("btn_switch_version"),
                vp.ext_name
            ));
        f.render_widget(block, r);
        let inner = Rect {
            x: r.x + 1,
            y: r.y + 1,
            width: r.width - 2,
            height: r.height - 2,
        };
        if vp.commits.is_empty() {
            let lines = vec![Line::from(Span::styled(
                i18n::t("popup_please_wait"),
                theme::base(),
            ))];
            f.render_widget(Paragraph::new(lines), inner);
            return;
        }
        vp.visible_rows
            .set((inner.height as usize).saturating_sub(3));
        // Picker is the active focus while it's open.
        core_tab::render_commit_table(
            f,
            inner,
            &vp.commits,
            vp.current.as_ref(),
            vp.selected,
            vp.scroll,
            true,
        );
    }

    /// Handles a mouse event.
    pub fn on_mouse(
        &mut self,
        m: crossterm::event::MouseEvent,
        area: Rect,
        cfg: &Config,
    ) -> Option<TaskRequest> {
        // 1. Actions menu hit-test (4 fixed rows in a 50x8 centered popup).
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
                // Click outside ≡ Esc.
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
                        // First click selects; second click on the same row
                        // activates (same as Enter).
                        if idx != am.selected {
                            am.selected = idx;
                            return None;
                        }
                        let (key, enabled) = items[idx];
                        if enabled {
                            let root = PathBuf::from(&cfg.general.comfyui_dir);
                            return self.dispatch_action(key, root, cfg);
                        }
                    }
                }
            }
            return None;
        }
        // 2. Version picker hit-test (table inside popup inner).
        if self.version_picker.is_some() {
            let r = popup::center(
                area,
                area.width.saturating_sub(6).min(100),
                area.height.saturating_sub(4).min(24),
            );
            let inside_popup = m.column >= r.x
                && m.column < r.x + r.width
                && m.row >= r.y
                && m.row < r.y + r.height;
            if !inside_popup {
                self.version_picker = None;
                return None;
            }
            let inner = Rect {
                x: r.x + 1,
                y: r.y + 1,
                width: r.width - 2,
                height: r.height - 2,
            };
            // Table content rows start at inner.y + 2 (table border + header).
            if m.column >= inner.x
                && m.column < inner.x + inner.width
                && m.row >= inner.y + 2
                && m.row < inner.y + inner.height
            {
                let rel = (m.row - (inner.y + 2)) as usize;
                if let Some(vp) = &mut self.version_picker {
                    let idx = vp.scroll + rel;
                    if idx < vp.commits.len() {
                        // First click moves the cursor; second click on the
                        // same row checks the commit out.
                        if idx != vp.selected {
                            vp.selected = idx;
                            vp.ensure_visible();
                            return None;
                        }
                        let ext_path = vp.ext_path.clone();
                        let ext_name = vp.ext_name.clone();
                        let rev = vp.commits[idx].short.clone();
                        let root = PathBuf::from(&cfg.general.comfyui_dir);
                        self.version_picker = None;
                        return Some(checkout_ext_request(
                            ext_path,
                            ext_name,
                            rev,
                            root,
                            env::build(&cfg.network),
                            cfg.general.python.clone(),
                        ));
                    }
                }
            }
            return None;
        }
        if let Some(n) = &mut self.notice {
            // Click outside fires immediate Close; OK / Copy arm their
            // Buttons (deferred via `poll_button_action` → `n.tick()`).
            if matches!(n.on_mouse(m, area), Some(NoticeOutcome::Close)) {
                self.notice = None;
            }
            return None;
        }
        if let Some(c) = &mut self.confirm {
            // Click outside fires immediate Cancel; OK / Cancel arm the
            // button (deferred-fire via `poll_action()` in tick).
            if let Some(false) = c.on_mouse(m, area) {
                self.confirm = None;
                self.pending_delete = None;
            }
            return None;
        }
        // Layout used by render: 3-line top row (search + UpdateAll) + table.
        let v = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(3), Constraint::Min(0)])
            .split(area);
        let upd_w = update_all_button_width();
        let rei_w = reinstall_all_button_width();
        let top = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Min(0),
                Constraint::Length(upd_w),
                Constraint::Length(rei_w),
            ])
            .split(v[0]);
        let search_area = top[0];
        let upd_btn_area = top[1];
        let rei_btn_area = top[2];
        let table_area = v[1];

        let inside = |r: Rect| {
            m.column >= r.x && m.column < r.x + r.width && m.row >= r.y && m.row < r.y + r.height
        };

        if inside(upd_btn_area) {
            // Arm the Button's deferred-fire pipeline. The action request
            // is built by `poll_button_action` two frames later, after one
            // full frame of visible focus highlight.
            self.focus = ExtFocus::UpdateAll;
            self.last_row0_focus = ExtFocus::UpdateAll;
            self.btn_update_all.click();
            return None;
        }
        if inside(rei_btn_area) {
            self.focus = ExtFocus::ReinstallAll;
            self.last_row0_focus = ExtFocus::ReinstallAll;
            self.btn_reinstall_all.click();
            return None;
        }
        if inside(search_area) {
            self.focus = ExtFocus::Search;
            self.last_row0_focus = ExtFocus::Search;
            return None;
        }
        self.focus = ExtFocus::Table;
        let top = table_area.y + 2;
        if m.row < top {
            return None;
        }
        let rel = (m.row - top) as usize;
        let view_idx = self.scroll + rel;
        let filtered = self.filtered_indices();
        if view_idx >= filtered.len() {
            return None;
        }
        if view_idx != self.selected {
            self.selected = view_idx;
            return None;
        }
        let ext = self.items[filtered[view_idx]].clone();
        self.actions_menu = Some(ActionsMenu {
            ext_path: ext.path,
            ext_name: ext.name,
            remote: ext.remote,
            disabled: ext.disabled,
            managed: ext.managed,
            selected: 0,
        });
        let _ = cfg;
        None
    }

    /// Handles a wheel-scroll event.
    pub fn scroll(&mut self, delta: i32) {
        let n = self.filtered_indices().len();
        if n == 0 {
            return;
        }
        if delta < 0 {
            if matches!(
                self.focus,
                ExtFocus::Search | ExtFocus::UpdateAll | ExtFocus::ReinstallAll
            ) {
                self.focus = ExtFocus::Table;
                self.selected = n - 1;
            } else if self.selected == 0 {
                self.focus = self.last_row0_focus;
            } else {
                self.selected -= 1;
            }
        } else {
            if matches!(
                self.focus,
                ExtFocus::Search | ExtFocus::UpdateAll | ExtFocus::ReinstallAll
            ) {
                self.focus = ExtFocus::Table;
                self.selected = 0;
                self.scroll = 0;
            } else if self.selected + 1 >= n {
                self.focus = self.last_row0_focus;
            } else {
                self.selected += 1;
            }
        }
        self.ensure_visible();
    }

    /// Attempts to handle a Left arrow within this tab.
    /// Returns `true` if the key was consumed.
    pub fn on_left(&mut self) -> bool {
        match self.focus {
            ExtFocus::Search => {
                if !self.search.at_start() {
                    self.search.on_key(KeyCode::Left);
                    return true;
                }
                false // propagate to tab switch
            }
            ExtFocus::UpdateAll => {
                self.focus = ExtFocus::Search;
                self.last_row0_focus = self.focus;
                true
            }
            ExtFocus::ReinstallAll => {
                self.focus = ExtFocus::UpdateAll;
                self.last_row0_focus = self.focus;
                true
            }
            ExtFocus::Table => false, // single col, propagate
        }
    }

    /// Attempts to handle a Right arrow within this tab.
    /// Returns `true` if the key was consumed.
    pub fn on_right(&mut self) -> bool {
        match self.focus {
            ExtFocus::Search => {
                if !self.search.at_end() {
                    self.search.on_key(KeyCode::Right);
                    return true;
                }
                self.focus = ExtFocus::UpdateAll;
                self.last_row0_focus = self.focus;
                true
            }
            ExtFocus::UpdateAll => {
                self.focus = ExtFocus::ReinstallAll;
                self.last_row0_focus = self.focus;
                true
            }
            ExtFocus::ReinstallAll => false, // at right edge, propagate
            ExtFocus::Table => false,
        }
    }

    /// Handles a key event.
    pub fn on_key(&mut self, code: KeyCode, cfg: &Config) -> Option<TaskRequest> {
        let root = PathBuf::from(&cfg.general.comfyui_dir);

        // Notice popup — Tab/L/R toggles button focus, Enter activates,
        // Esc closes. Copy button writes to clipboard and queues a flash.
        if let Some(n) = &mut self.notice {
            // Esc closes immediately; Enter arms the focused button (Copy
            // outcome arrives via `poll_button_action` → `n.tick()`).
            if matches!(n.on_key(code), Some(NoticeOutcome::Close)) {
                self.notice = None;
            }
            return None;
        }
        // Actions menu: intercept before anything else.
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
                    return self.dispatch_action(key, root, cfg);
                }
                _ => return None,
            }
        }

        // Version-picker popup eats input.
        if let Some(vp) = &mut self.version_picker {
            let n = vp.commits.len();
            match code {
                KeyCode::Esc => {
                    self.version_picker = None;
                    return None;
                }
                KeyCode::Up => {
                    if n == 0 {
                        return None;
                    }
                    if vp.selected == 0 {
                        let path = vp.ext_path.clone();
                        let name = vp.ext_name.clone();
                        let limit = vp.limit;
                        return Some(list_versions_request_n(
                            path,
                            name,
                            env::build(&cfg.network),
                            limit,
                        ));
                    }
                    vp.selected -= 1;
                    vp.ensure_visible();
                    return None;
                }
                KeyCode::Down => {
                    if n == 0 {
                        return None;
                    }
                    if vp.selected + 1 >= n {
                        if !vp.end_reached {
                            let path = vp.ext_path.clone();
                            let name = vp.ext_name.clone();
                            let new_limit = vp.limit.saturating_add(LIST_MAX_NUM);
                            return Some(list_versions_request_n(
                                path,
                                name,
                                env::build(&cfg.network),
                                new_limit,
                            ));
                        }
                        vp.selected = 0;
                        vp.ensure_visible();
                        return None;
                    }
                    vp.selected += 1;
                    vp.ensure_visible();
                    return None;
                }
                KeyCode::PageUp => {
                    vp.selected = 0;
                    vp.ensure_visible();
                    return None;
                }
                KeyCode::PageDown => {
                    if n > 0 {
                        vp.selected = n - 1;
                    }
                    vp.ensure_visible();
                    return None;
                }
                KeyCode::Enter => {
                    if let Some(c) = vp.commits.get(vp.selected) {
                        let ext_path = vp.ext_path.clone();
                        let ext_name = vp.ext_name.clone();
                        let rev = c.short.clone();
                        self.version_picker = None;
                        return Some(checkout_ext_request(
                            ext_path,
                            ext_name,
                            rev,
                            root,
                            env::build(&cfg.network),
                            cfg.general.python.clone(),
                        ));
                    }
                    return None;
                }
                _ => return None,
            }
        }

        if let Some(c) = &mut self.confirm {
            // Esc and Enter route through Confirm::on_key. Esc returns
            // Some(false) → immediate Cancel. Enter arms the focused
            // Button — outcome arrives via `poll_action()` next frame.
            if let Some(false) = c.on_key(code) {
                self.confirm = None;
                self.pending_delete = None;
            }
            return None;
        }

        // Search input has focus: typing edits it; Tab moves to UpdateAll;
        // Enter / Esc returns to Table.
        if self.focus == ExtFocus::Search {
            match code {
                KeyCode::Tab => {
                    self.focus = ExtFocus::UpdateAll;
                    self.last_row0_focus = self.focus;
                }
                KeyCode::BackTab => {
                    self.focus = ExtFocus::Table;
                }
                KeyCode::Enter | KeyCode::Down => {
                    self.last_row0_focus = self.focus;
                    self.focus = ExtFocus::Table;
                    self.selected = 0;
                    self.scroll = 0;
                }
                KeyCode::Up => {
                    let n = self.filtered_indices().len();
                    if n > 0 {
                        self.last_row0_focus = self.focus;
                        self.focus = ExtFocus::Table;
                        self.selected = n - 1;
                        self.ensure_visible();
                    }
                }
                KeyCode::Esc => {
                    self.focus = ExtFocus::Table;
                }
                k if !matches!(k, KeyCode::Left | KeyCode::Right) => {
                    self.search.on_key(k);
                    self.selected = 0;
                    self.scroll = 0;
                }
                _ => {}
            }
            return None;
        }

        // UpdateAll button focused: Enter runs update-all; Tab cycles on.
        if self.focus == ExtFocus::UpdateAll {
            match code {
                KeyCode::Tab => {
                    self.focus = ExtFocus::ReinstallAll;
                    self.last_row0_focus = self.focus;
                    return None;
                }
                KeyCode::BackTab => {
                    self.focus = ExtFocus::Search;
                    self.last_row0_focus = self.focus;
                    return None;
                }
                KeyCode::Esc => {
                    self.focus = ExtFocus::Table;
                    return None;
                }
                KeyCode::Down => {
                    self.last_row0_focus = self.focus;
                    self.focus = ExtFocus::Table;
                    self.selected = 0;
                    self.scroll = 0;
                    return None;
                }
                KeyCode::Up => {
                    let n = self.filtered_indices().len();
                    if n > 0 {
                        self.last_row0_focus = self.focus;
                        self.focus = ExtFocus::Table;
                        self.selected = n - 1;
                        self.ensure_visible();
                    }
                    return None;
                }
                KeyCode::Enter => {
                    let items = self.items.clone();
                    let env_vars = env::build(&cfg.network);
                    let python = cfg.general.python.clone();
                    return Some(update_all_request(items, root, env_vars, python));
                }
                _ => return None,
            }
        }

        // ReinstallAll button focused: Enter runs the force-sync.
        if self.focus == ExtFocus::ReinstallAll {
            match code {
                KeyCode::Tab => {
                    self.focus = ExtFocus::Table;
                    return None;
                }
                KeyCode::BackTab => {
                    self.focus = ExtFocus::UpdateAll;
                    self.last_row0_focus = self.focus;
                    return None;
                }
                KeyCode::Esc => {
                    self.focus = ExtFocus::Table;
                    return None;
                }
                KeyCode::Down => {
                    self.last_row0_focus = self.focus;
                    self.focus = ExtFocus::Table;
                    self.selected = 0;
                    self.scroll = 0;
                    return None;
                }
                KeyCode::Up => {
                    let n = self.filtered_indices().len();
                    if n > 0 {
                        self.last_row0_focus = self.focus;
                        self.focus = ExtFocus::Table;
                        self.selected = n - 1;
                        self.ensure_visible();
                    }
                    return None;
                }
                KeyCode::Enter => {
                    let items = self.items.clone();
                    let env_vars = env::build(&cfg.network);
                    let python = cfg.general.python.clone();
                    return Some(reinstall_all_request(items, root, env_vars, python));
                }
                _ => return None,
            }
        }

        if matches!(code, KeyCode::Tab) {
            self.focus = self.last_row0_focus;
            return None;
        }
        if matches!(code, KeyCode::BackTab) {
            self.focus = ExtFocus::ReinstallAll;
            return None;
        }

        let filtered = self.filtered_indices();
        let n = filtered.len();
        match code {
            KeyCode::Up => {
                if n == 0 {
                    return None;
                }
                if self.selected == 0 {
                    self.focus = self.last_row0_focus;
                } else {
                    self.selected -= 1;
                }
                self.ensure_visible();
                None
            }
            KeyCode::Down => {
                if n == 0 {
                    return None;
                }
                if self.selected + 1 >= n {
                    if !self.end_reached && self.search.value.is_empty() {
                        let new_limit = self.limit.saturating_add(LIST_MAX_NUM);
                        return Some(load_request_with_limit(
                            root,
                            new_limit,
                            env::build(&cfg.network),
                        ));
                    }
                    self.focus = self.last_row0_focus;
                    self.ensure_visible();
                    return None;
                }
                self.selected += 1;
                self.ensure_visible();
                None
            }
            KeyCode::PageUp => {
                self.focus = self.last_row0_focus;
                None
            }
            KeyCode::PageDown => {
                if n > 0 {
                    self.selected = n - 1;
                }
                self.ensure_visible();
                None
            }
            KeyCode::Char('r') | KeyCode::Char('R') => {
                // Re-populate from disk immediately so new / removed dirs are
                // visible without waiting for the background fetch; then kick
                // off the refresh that recomputes the `behind` column.
                let limit = self.limit;
                self.items = scan_local(&root, limit);
                self.end_reached = self.items.len() < limit;
                self.loaded_for = Some(root.clone());
                let n = self.filtered_indices().len();
                if self.selected >= n {
                    self.selected = n.saturating_sub(1);
                }
                self.ensure_visible();
                Some(load_request_with_limit(
                    root,
                    limit,
                    env::build(&cfg.network),
                ))
            }
            KeyCode::Char('d') | KeyCode::Char('D') => {
                if let Some(real) = self.current_real_idx() {
                    let ext = &self.items[real];
                    self.pending_delete = Some(ext.path.clone());
                    self.confirm = Some(Confirm::new(
                        i18n::t("btn_uninstall"),
                        i18n::t_args("popup_confirm_uninstall", &[("name", &ext.name)]),
                        false,
                    ));
                }
                None
            }
            KeyCode::Enter => {
                if let Some(real) = self.current_real_idx() {
                    let ext = &self.items[real];
                    self.actions_menu = Some(ActionsMenu {
                        ext_path: ext.path.clone(),
                        ext_name: ext.name.clone(),
                        remote: ext.remote.clone(),
                        disabled: ext.disabled,
                        managed: ext.managed,
                        selected: 0,
                    });
                }
                None
            }
            KeyCode::Char('u') | KeyCode::Char('U') => self.current_real_idx().and_then(|real| {
                let ext = &self.items[real];
                if !ext.managed {
                    return None;
                }
                Some(update_one_request(
                    ext.path.clone(),
                    ext.name.clone(),
                    root,
                    env::build(&cfg.network),
                    cfg.general.python.clone(),
                ))
            }),
            _ => None,
        }
    }

    /// Execute the selected action in the actions menu.
    fn dispatch_action(
        &mut self,
        key: &'static str,
        root: PathBuf,
        cfg: &Config,
    ) -> Option<TaskRequest> {
        let am = self.actions_menu.take()?;
        match key {
            "btn_update_to_latest" => {
                if !am.managed {
                    return None;
                }
                Some(update_one_request(
                    am.ext_path,
                    am.ext_name,
                    root,
                    env::build(&cfg.network),
                    cfg.general.python.clone(),
                ))
            }
            "btn_change_version" => {
                // Open the version picker; kicks off the load task.
                self.version_picker = Some(VersionPicker {
                    ext_path: am.ext_path.clone(),
                    ext_name: am.ext_name.clone(),
                    commits: vec![],
                    current: None,
                    selected: 0,
                    scroll: 0,
                    limit: LIST_MAX_NUM,
                    end_reached: false,
                    visible_rows: Cell::new(0),
                });
                Some(list_versions_request_n(
                    am.ext_path,
                    am.ext_name,
                    env::build(&cfg.network),
                    LIST_MAX_NUM,
                ))
            }
            "btn_open_url" => {
                if !opener::open_url(&am.remote) {
                    let url = am.remote.clone();
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
            "btn_enable" | "btn_disable" => Some(toggle_enabled_request(
                am.ext_path,
                am.ext_name,
                am.disabled,
                root,
            )),
            "btn_uninstall" => {
                self.pending_delete = Some(am.ext_path.clone());
                self.confirm = Some(Confirm::new(
                    i18n::t("btn_uninstall"),
                    i18n::t_args("popup_confirm_uninstall", &[("name", &am.ext_name)]),
                    false,
                ));
                None
            }
            _ => None,
        }
    }
}

/// Reads local-only information for a single custom-node directory.
///
/// Returns `None` when `p` is not a loadable extension directory (missing
/// `__init__.py`, dotfile, and so on). `behind` is computed against
/// whatever remote refs are cached, purely as a function of current
/// `HEAD` versus upstream. This makes Change Version correct: reverting
/// to an older commit makes `behind` jump to the gap against
/// `origin/HEAD`.
pub fn read_one_local(p: &Path) -> Option<Extension> {
    if !p.is_dir() {
        return None;
    }
    let raw = p
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_string();
    let disabled = raw.ends_with(".disabled");
    let logical = raw.strip_suffix(".disabled").unwrap_or(&raw).to_string();
    if logical.starts_with('.') || logical.starts_with("__") {
        return None;
    }
    if !p.join("__init__.py").is_file() {
        return None;
    }
    let managed = p.join(".git").exists();
    let remote = if managed {
        git::remote_url(p).unwrap_or_default()
    } else {
        String::new()
    };
    let branch = if managed {
        git::current_branch(p).unwrap_or_default()
    } else {
        String::new()
    };
    let head = if managed {
        git::current_commit(p).unwrap_or_default()
    } else {
        String::new()
    };
    let head_date = if managed {
        git::log(p, 1)
            .ok()
            .and_then(|c| c.first().map(|c| c.date.clone()))
            .unwrap_or_default()
    } else {
        String::new()
    };
    let behind = if managed { git::behind_upstream(p) } else { 0 };
    Some(Extension {
        name: logical,
        path: p.to_path_buf(),
        disabled,
        managed,
        remote,
        branch,
        head,
        head_date,
        behind,
    })
}

/// Walks `custom_nodes/` synchronously, filters to loadable nodes, and
/// reads local-only git information for each.
///
/// Does not invoke `git fetch`; the `behind` values reflect whatever was
/// cached at the last fetch. The background
/// [`load_request_with_limit`] is responsible for refreshing the remote.
pub fn scan_local(root: &Path, limit: usize) -> Vec<Extension> {
    let dirs = ComfyDirs::new(root);
    let custom = dirs.custom_nodes();
    let mut out: Vec<Extension> = Vec::new();
    if let Ok(rd) = std::fs::read_dir(&custom) {
        for ent in rd.flatten() {
            if out.len() >= limit {
                break;
            }
            if let Some(ext) = read_one_local(&ent.path()) {
                out.push(ext);
            }
        }
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

/// Builds a background task that refreshes remote information for every
/// managed extension.
///
/// Runs `git fetch` per extension and recomputes `behind_upstream`,
/// emitting per-item Progress for the flash banner. The final `ExtData`
/// replaces the synchronously populated list. Display is not gated on
/// this task because `scan_local` already populated the list at tab
/// entry.
pub fn load_request_with_limit(
    root: PathBuf,
    limit: usize,
    env_vars: std::collections::HashMap<String, String>,
) -> TaskRequest {
    TaskRequest {
        title: i18n::t("task_ext_load"),
        then: TaskKind::None,
        is_refresh: true,
        work: Box::new(move |tx| {
            use std::sync::{
                atomic::{AtomicUsize, Ordering},
                mpsc as inner_mpsc, Arc,
            };
            let dirs = ComfyDirs::new(&root);
            let custom = dirs.custom_nodes();
            // First pass: enumerate cheaply so `total` is known up front
            // for the progress denominator. Second pass: parallel git work.
            let mut entries: Vec<(PathBuf, String, bool, bool)> = Vec::new();
            if let Ok(rd) = std::fs::read_dir(&custom) {
                for ent in rd.flatten() {
                    if entries.len() >= limit {
                        break;
                    }
                    let p = ent.path();
                    if !p.is_dir() {
                        continue;
                    }
                    let raw = p
                        .file_name()
                        .and_then(|s| s.to_str())
                        .unwrap_or("")
                        .to_string();
                    let disabled = raw.ends_with(".disabled");
                    let logical = raw.strip_suffix(".disabled").unwrap_or(&raw).to_string();
                    if logical.starts_with('.') || logical.starts_with("__") {
                        continue;
                    }
                    if !p.join("__init__.py").is_file() {
                        continue;
                    }
                    let managed = p.join(".git").exists();
                    entries.push((p, logical, disabled, managed));
                }
            }
            let total = entries.len();
            let _ = tx.send(TaskResult::Progress { done: 0, total });

            // Parallel git fetch + info collection. Chunk-disjoint partition
            // across SCAN_WORKERS threads; each thread bumps a shared counter
            // and emits Progress. Results stream back via a separate channel.
            let workers = SCAN_WORKERS.min(total).max(1);
            let chunk = total.div_ceil(workers);
            let done = Arc::new(AtomicUsize::new(0));
            let (res_tx, res_rx) = inner_mpsc::channel::<Extension>();
            let mut handles = Vec::with_capacity(workers);
            for c in entries.chunks(chunk) {
                let mine: Vec<_> = c.to_vec();
                let env = env_vars.clone();
                let done = done.clone();
                let progress_tx = tx.clone();
                let res_tx = res_tx.clone();
                handles.push(std::thread::spawn(move || {
                    for (p, logical, disabled, managed) in mine {
                        if managed {
                            let _ = git::fetch(&p, env.clone());
                        }
                        let remote = if managed {
                            git::remote_url(&p).unwrap_or_default()
                        } else {
                            String::new()
                        };
                        let branch = if managed {
                            git::current_branch(&p).unwrap_or_default()
                        } else {
                            String::new()
                        };
                        let head = if managed {
                            git::current_commit(&p).unwrap_or_default()
                        } else {
                            String::new()
                        };
                        let head_date = if managed {
                            git::log(&p, 1)
                                .ok()
                                .and_then(|c| c.first().map(|c| c.date.clone()))
                                .unwrap_or_default()
                        } else {
                            String::new()
                        };
                        let behind = if managed { git::behind_upstream(&p) } else { 0 };
                        let _ = res_tx.send(Extension {
                            name: logical,
                            path: p,
                            disabled,
                            managed,
                            remote,
                            branch,
                            head,
                            head_date,
                            behind,
                        });
                        let now = done.fetch_add(1, Ordering::SeqCst) + 1;
                        let _ = progress_tx.send(TaskResult::Progress { done: now, total });
                    }
                }));
            }
            drop(res_tx);
            let mut out: Vec<Extension> = res_rx.iter().collect();
            for h in handles {
                let _ = h.join();
            }
            out.sort_by(|a, b| a.name.cmp(&b.name));
            let _ = tx.send(TaskResult::ExtData {
                items: out,
                root,
                requested_limit: limit,
            });
        }),
    }
}

/// Cap on parallel git workers for the scan / Update All / Reinstall All
/// tasks. 8 hits the typical sweet spot for git-fetch-dominated workloads.
const SCAN_WORKERS: usize = 8;

pub(super) fn update_one_request(
    path: PathBuf,
    name: String,
    _root: PathBuf,
    env_vars: std::collections::HashMap<String, String>,
    python: String,
) -> TaskRequest {
    let title = i18n::t_args("task_ext_update_one", &[("name", &name)]);
    TaskRequest {
        title,
        then: TaskKind::None,
        is_refresh: false,
        work: Box::new(move |tx| {
            sync_to_upstream(&path, &env_vars);
            if !python.is_empty() {
                let _ = pip::install_requirements(std::path::Path::new(&python), &path, env_vars);
            }
            if let Some(ext) = read_one_local(&path) {
                let _ = tx.send(TaskResult::ExtRowUpdate {
                    old_path: path,
                    ext,
                });
            }
        }),
    }
}

/// Bring a single extension repo to its upstream HEAD reliably — works for
/// detached-HEAD repos (from prior Change Version) and for repos with
/// uncommitted local edits (ComfyUI custom nodes often write config files
/// into their own dirs, which makes `git pull --ff-only` refuse). Strategy:
///   1. `git fetch --all` so refs are current.
///   2. Pick the target ref: upstream of current branch (`@{u}`) if we're on
///      a branch, otherwise whatever was just fetched (`FETCH_HEAD`).
///   3. `git reset --hard <target>` — discards local edits to tracked files;
///      untracked files (user data) are left alone.
fn sync_to_upstream(path: &std::path::Path, env_vars: &std::collections::HashMap<String, String>) {
    let _ = git::fetch(path, env_vars.clone());
    // On a branch with upstream → `@{u}` is well-defined.
    // Detached HEAD → fall back to `origin/HEAD` (the remote default branch,
    // set by `git clone`). Final fallback `FETCH_HEAD` covers the rare repo
    // with no `origin/HEAD` symref. `git::current_branch` now correctly
    // returns None for detached HEAD so this branch is taken there.
    let targets: &[&str] = if git::current_branch(path).is_some() {
        &["@{u}"]
    } else {
        &["origin/HEAD", "FETCH_HEAD"]
    };
    for t in targets {
        if git::reset_hard(path, t, env_vars.clone()).unwrap_or(false) {
            return;
        }
    }
}

fn update_all_request(
    items: Vec<Extension>,
    root: PathBuf,
    env_vars: std::collections::HashMap<String, String>,
    python: String,
) -> TaskRequest {
    let then = TaskKind::ExtLoad(root);
    TaskRequest {
        title: i18n::t("task_ext_update_all"),
        then,
        is_refresh: false,
        work: Box::new(move |tx| {
            use std::sync::{
                atomic::{AtomicUsize, Ordering},
                Arc,
            };
            let managed: Vec<Extension> = items.into_iter().filter(|e| e.managed).collect();
            let total = managed.len();
            let _ = tx.send(TaskResult::Progress { done: 0, total });
            let workers = SCAN_WORKERS.min(total).max(1);
            let chunk = total.div_ceil(workers);
            let done = Arc::new(AtomicUsize::new(0));
            let python = Arc::new(python);
            let mut handles = Vec::with_capacity(workers);
            for c in managed.chunks(chunk) {
                let mine: Vec<Extension> = c.to_vec();
                let env = env_vars.clone();
                let done = done.clone();
                let progress_tx = tx.clone();
                let python = python.clone();
                handles.push(std::thread::spawn(move || {
                    for ext in mine {
                        sync_to_upstream(&ext.path, &env);
                        if !python.is_empty() {
                            let _ = pip::install_requirements(
                                std::path::Path::new(python.as_str()),
                                &ext.path,
                                env.clone(),
                            );
                        }
                        let now = done.fetch_add(1, Ordering::SeqCst) + 1;
                        let _ = progress_tx.send(TaskResult::Progress { done: now, total });
                    }
                }));
            }
            for h in handles {
                let _ = h.join();
            }
        }),
    }
}

/// Background task: fetch + list commits for a single extension up to `limit`.
/// Result is delivered to the open `VersionPicker` via `TaskResult::ExtCommits`.
fn list_versions_request_n(
    path: PathBuf,
    name: String,
    env_vars: std::collections::HashMap<String, String>,
    limit: usize,
) -> TaskRequest {
    let title = i18n::t_args("task_ext_versions", &[("name", &name)]);
    TaskRequest {
        title,
        then: TaskKind::None,
        is_refresh: false,
        work: Box::new(move |tx| {
            let _ = git::fetch(&path, env_vars.clone());
            let _ = git::deepen_until_full(&path, env_vars);
            let commits = git::log_all(&path, limit).unwrap_or_default();
            let current = git::current_commit(&path);
            let _ = tx.send(TaskResult::ExtCommits {
                ext_path: path,
                commits,
                current,
                requested_limit: limit,
            });
        }),
    }
}

fn checkout_ext_request(
    path: PathBuf,
    name: String,
    rev: String,
    _root: PathBuf,
    env_vars: std::collections::HashMap<String, String>,
    python: String,
) -> TaskRequest {
    let title = i18n::t_args("task_ext_checkout", &[("name", &name), ("rev", &rev)]);
    TaskRequest {
        title,
        then: TaskKind::None,
        is_refresh: false,
        work: Box::new(move |tx| {
            let ok = git::checkout(&path, &rev, env_vars.clone()).unwrap_or(false);
            // Verify the HEAD actually moved so a silent failure shows up in
            // the popup tail instead of looking like nothing happened.
            if let Some(head) = git::current_commit(&path) {
                if !head.starts_with(&rev) && !rev.starts_with(&head) {
                    crate::core::log_bus::push(
                        "git",
                        format!("checkout did not reach {rev}; HEAD is still {head}"),
                    );
                }
            }
            if ok && !python.is_empty() {
                let _ = pip::install_requirements(std::path::Path::new(&python), &path, env_vars);
            }
            if let Some(ext) = read_one_local(&path) {
                let _ = tx.send(TaskResult::ExtRowUpdate {
                    old_path: path,
                    ext,
                });
            }
        }),
    }
}

fn uninstall_request(path: PathBuf, _root: PathBuf, name: String) -> TaskRequest {
    let title = i18n::t_args("task_ext_uninstall", &[("name", &name)]);
    TaskRequest {
        title,
        then: TaskKind::None,
        is_refresh: false,
        work: Box::new(move |tx| {
            let _ = std::fs::remove_dir_all(&path);
            let _ = tx.send(TaskResult::ExtRowRemove { path });
        }),
    }
}

/// Rename `<path>` ↔ `<path>.disabled` to (un)hide it from ComfyUI's
/// custom-node loader, which itself skips any entry ending in `.disabled`.
fn toggle_enabled_request(
    path: PathBuf,
    name: String,
    currently_disabled: bool,
    _root: PathBuf,
) -> TaskRequest {
    let title = if currently_disabled {
        i18n::t_args("task_ext_enable", &[("name", &name)])
    } else {
        i18n::t_args("task_ext_disable", &[("name", &name)])
    };
    TaskRequest {
        title,
        then: TaskKind::None,
        is_refresh: false,
        work: Box::new(move |tx| {
            let new_path = if currently_disabled {
                // Strip trailing ".disabled"
                let raw = path.file_name().and_then(|s| s.to_str()).unwrap_or("");
                let stripped = raw.strip_suffix(".disabled").unwrap_or(raw);
                path.with_file_name(stripped)
            } else {
                let raw = path.file_name().and_then(|s| s.to_str()).unwrap_or("");
                path.with_file_name(format!("{raw}.disabled"))
            };
            if let Err(e) = std::fs::rename(&path, &new_path) {
                crate::core::log_bus::push("ext", format!("rename failed: {e}"));
                return;
            }
            if let Some(ext) = read_one_local(&new_path) {
                let _ = tx.send(TaskResult::ExtRowUpdate {
                    old_path: path,
                    ext,
                });
            }
        }),
    }
}

fn update_all_button_width() -> u16 {
    use unicode_width::UnicodeWidthStr;
    i18n::t("btn_update_all").width() as u16 + 4
}

fn reinstall_all_button_width() -> u16 {
    use unicode_width::UnicodeWidthStr;
    i18n::t("btn_reinstall_all").width() as u16 + 4
}

/// Force-sync every managed extension to its upstream HEAD: `git fetch` then
/// `git reset --hard @{u}`. Discards local edits to tracked files; untracked
/// files are left alone (consistent with our destructive checkout policy).
fn reinstall_all_request(
    items: Vec<Extension>,
    root: PathBuf,
    env_vars: std::collections::HashMap<String, String>,
    python: String,
) -> TaskRequest {
    let then = TaskKind::ExtLoad(root);
    TaskRequest {
        title: i18n::t("task_ext_reinstall_all"),
        then,
        is_refresh: false,
        work: Box::new(move |tx| {
            use std::sync::{
                atomic::{AtomicUsize, Ordering},
                Arc,
            };
            let managed: Vec<Extension> = items.into_iter().filter(|e| e.managed).collect();
            let total = managed.len();
            let _ = tx.send(TaskResult::Progress { done: 0, total });
            let workers = SCAN_WORKERS.min(total).max(1);
            let chunk = total.div_ceil(workers);
            let done = Arc::new(AtomicUsize::new(0));
            let python = Arc::new(python);
            let mut handles = Vec::with_capacity(workers);
            for c in managed.chunks(chunk) {
                let mine: Vec<Extension> = c.to_vec();
                let env = env_vars.clone();
                let done = done.clone();
                let progress_tx = tx.clone();
                let python = python.clone();
                handles.push(std::thread::spawn(move || {
                    for ext in mine {
                        sync_to_upstream(&ext.path, &env);
                        if !python.is_empty() {
                            let _ = pip::install_requirements(
                                std::path::Path::new(python.as_str()),
                                &ext.path,
                                env.clone(),
                            );
                        }
                        let now = done.fetch_add(1, Ordering::SeqCst) + 1;
                        let _ = progress_tx.send(TaskResult::Progress { done: now, total });
                    }
                }));
            }
            for h in handles {
                let _ = h.join();
            }
        }),
    }
}
