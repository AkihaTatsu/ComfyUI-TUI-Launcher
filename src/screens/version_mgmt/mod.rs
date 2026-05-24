//! Version management screen with Core, Extensions, and Install tabs.
//!
//! Coordinates the background work shared across the three tabs and owns
//! the mutation and refresh task queues.

/// Core ComfyUI repository tab.
pub mod core_tab;
/// Installed extensions tab.
pub mod extensions_tab;
/// Install new extensions tab.
pub mod install_tab;

use crate::core::config::Config;
use crate::core::{i18n, log_bus, theme};
use crate::widgets::popup;
use crate::widgets::tabs::Tabs;
use crossterm::event::KeyCode;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};
use ratatui::Frame;
use std::path::PathBuf;
use std::sync::mpsc;
use std::thread;

/// Self-contained description of a long-running task triggered by Version
/// Management.
///
/// The closure runs on a background thread; it may send typed data via
/// the sender, then drops it to signal completion. `then` is an optional
/// follow-up queued automatically once this task finishes (for example a
/// pull chained into a list reload).
pub struct TaskRequest {
    /// Human-readable title shown in the working popup.
    pub title: String,
    /// Background closure to execute.
    pub work: Box<dyn FnOnce(mpsc::Sender<TaskResult>) + Send + 'static>,
    /// Follow-up task queued after this one completes.
    pub then: TaskKind,
    /// Whether this is a read-only refresh task.
    ///
    /// Refresh tasks run in the background with progress in the top-right
    /// banner instead of the blocking working popup. Only one refresh
    /// runs at a time; manual triggers drop silently while busy, and auto
    /// triggers are queued.
    pub is_refresh: bool,
}

/// Identifier for a follow-up task to queue after the current one finishes.
#[derive(Clone)]
pub enum TaskKind {
    /// No follow-up.
    None,
    /// Reload the Core commit list from the given repository path.
    CoreLoad(PathBuf),
    /// Reload the extensions list from the given ComfyUI root.
    ExtLoad(PathBuf),
}

/// Cap on the number of rows fetched in a single load. Additional
/// batches of this size are fetched when navigation crosses the loaded
/// edge.
pub const LIST_MAX_NUM: usize = 1000;

/// Result emitted by a background task.
pub enum TaskResult {
    /// Result of a Core repository load.
    CoreData {
        /// Commit list, newest first.
        commits: Vec<crate::core::git::Commit>,
        /// Release tags pointing at commits in the repository.
        tags: Vec<crate::core::git::TagCommit>,
        /// Short SHA of the current `HEAD`.
        current: Option<String>,
        /// Release tag name when `HEAD` is exactly at one.
        current_tag: Option<String>,
        /// Current branch, or `None` for a detached `HEAD`.
        branch: Option<String>,
        /// Remote origin URL.
        remote: Option<String>,
        /// Repository root.
        root: PathBuf,
        /// Row count requested by the caller.
        requested_limit: usize,
    },
    /// Result of an Extensions list load.
    ExtData {
        /// Loaded extensions.
        items: Vec<extensions_tab::Extension>,
        /// ComfyUI root.
        root: PathBuf,
        /// Row count requested by the caller.
        requested_limit: usize,
    },
    /// Commit list for a single extension, used by the version picker popup.
    ExtCommits {
        /// Extension path.
        ext_path: PathBuf,
        /// Commit list, newest first.
        commits: Vec<crate::core::git::Commit>,
        /// Short SHA of the extension's current `HEAD`.
        current: Option<String>,
        /// Row count requested by the caller.
        requested_limit: usize,
    },
    /// Official extension catalog, used by the Install New tab.
    RegistryData {
        /// Catalog entries.
        entries: Vec<crate::core::extension_registry::RegistryEntry>,
    },
    /// Incremental progress for long-running multi-step tasks.
    Progress {
        /// Number of items completed.
        done: usize,
        /// Total items to process.
        total: usize,
    },
    /// Surgical update of one extension row after a single-entry mutation.
    ///
    /// `old_path` matches the row to be replaced; `ext` is the freshly
    /// read local state. The two paths differ only for enable / disable
    /// where the directory was renamed.
    ExtRowUpdate {
        /// Path of the row to replace.
        old_path: PathBuf,
        /// Freshly read extension state.
        ext: extensions_tab::Extension,
    },
    /// Removes the row whose `path` matches after an uninstall.
    ExtRowRemove {
        /// Path of the row to remove.
        path: PathBuf,
    },
    /// Appends a freshly read extension row after Install New. The list is
    /// re-sorted by name on insert.
    ExtRowAdd {
        /// New extension to append.
        ext: extensions_tab::Extension,
    },
    /// Updates only the Core repository's currently checked-out commit
    /// after a Core Change Version. The commit list itself is left alone.
    CoreHeadUpdate {
        /// Short SHA of the new `HEAD`.
        current: Option<String>,
        /// Release tag name if the new `HEAD` is exactly at one.
        current_tag: Option<String>,
    },
}

struct PendingTask {
    title: String,
    rx: mpsc::Receiver<TaskResult>,
    then: TaskKind,
    progress: Option<(usize, usize)>,
}

/// Version management screen state.
pub struct VersionMgmt {
    /// Active tab index.
    pub tab: usize,
    /// Core tab state.
    pub core: core_tab::CoreTab,
    /// Extensions tab state.
    pub ext: extensions_tab::ExtensionsTab,
    /// Install New tab state.
    pub install: install_tab::InstallTab,
    /// Active mutation task. Drives the centered working popup and locks
    /// input while present.
    pending: Option<PendingTask>,
    /// Active refresh task. Runs in the background and surfaces progress
    /// via the top-right banner without locking input.
    refresh: Option<PendingTask>,
    /// One queued auto-refresh, promoted into `refresh` the moment the
    /// current one finishes. Manual refreshes bypass this slot.
    queued_refresh: Option<TaskRequest>,
}

impl VersionMgmt {
    /// Constructs a fresh version management screen.
    pub fn new() -> Self {
        Self {
            tab: 0,
            core: core_tab::CoreTab::new(),
            ext: extensions_tab::ExtensionsTab::new(),
            install: install_tab::InstallTab::new(),
            pending: None,
            refresh: None,
            queued_refresh: None,
        }
    }

    /// Returns whether a mutation popup is currently shown.
    ///
    /// Background refresh state is not included because it does not
    /// block input.
    pub fn is_busy(&self) -> bool {
        self.pending.is_some()
    }

    /// Synchronously populate `self.ext.items` from a local-only scan so the
    /// Extensions list renders immediately on tab entry / manual refresh —
    /// independent of the background `git fetch` work that follows.
    fn populate_ext_local(&mut self, root: &std::path::Path, limit: usize) {
        let items = extensions_tab::scan_local(root, limit);
        self.ext.end_reached = items.len() < limit;
        self.ext.limit = limit;
        self.ext.items = items;
        self.ext.loaded_for = Some(root.to_path_buf());
        let n = self.ext.filtered_indices().len();
        if self.ext.selected >= n {
            self.ext.selected = n.saturating_sub(1);
        }
        self.ext.ensure_visible();
    }

    /// Same idea as `populate_ext_local` but for the Core tab. Reads local
    /// git log + HEAD info; no fetch.
    fn populate_core_local(&mut self, root: &std::path::Path, limit: usize) {
        let scan = core_tab::scan_local(root, limit);
        self.core.end_reached = scan.commits.len() < limit;
        self.core.limit = limit;
        self.core.commits = scan.commits;
        self.core.tags = scan.tags;
        self.core.current = scan.current;
        self.core.current_tag = scan.current_tag;
        self.core.branch = scan.branch;
        self.core.remote = scan.remote;
        self.core.loaded_for = Some(root.to_path_buf());
        if self.core.all_list_selected >= self.core.commits.len() {
            self.core.all_list_selected = self.core.commits.len().saturating_sub(1);
        }
        if self.core.stable_list_selected >= self.core.tags.len() {
            self.core.stable_list_selected = self.core.tags.len().saturating_sub(1);
        }
        self.core.restore_filter_state();
        self.core.ensure_visible();
    }

    /// Drains transient flash messages from the Extensions and Install
    /// sub-tabs and surfaces them to the application.
    pub fn take_flash(&mut self) -> Option<(crate::app::FlashKind, String)> {
        self.ext.take_flash().or_else(|| self.install.take_flash())
    }

    /// Returns the sticky banner text reflecting the current background
    /// refresh progress, or `None` when no refresh is in flight.
    pub fn permanent_flash(&self) -> Option<(crate::app::FlashKind, String)> {
        let p = self.refresh.as_ref()?;
        let text = match p.progress {
            Some((d, t)) => format!("{} ({d}/{t})", p.title),
            None => p.title.clone(),
        };
        Some((crate::app::FlashKind::Info, text))
    }

    /// Per-frame housekeeping.
    ///
    /// Drains each task channel, applies new data, chains the follow-up
    /// when a task finishes, and lazily kicks off the initial load when
    /// the user lands on a tab for the first time.
    pub fn tick(&mut self, cfg: &Config) {
        // Poll each sub-tab's persistent button widgets so the deferred
        // click-then-fire pipeline drains.
        let req = self
            .ext
            .poll_button_action(cfg)
            .or_else(|| self.install.poll_button_action(cfg));
        if let Some(req) = req {
            self.spawn(req);
        }
        // Drain both task slots; the two share the same logic and route
        // Progress updates to the slot they came from.
        self.drain_slot(true);
        self.drain_slot(false);
        // Promote any queued auto-refresh into the now-free slot.
        if self.refresh.is_none() {
            if let Some(req) = self.queued_refresh.take() {
                self.spawn_inner(req);
            }
        }
        // Lazily kick off the initial load for the active tab as a
        // refresh task; `spawn_auto` queues it when another refresh is
        // already running.
        let root = std::path::Path::new(&cfg.general.comfyui_dir).to_path_buf();
        // Re-evaluate every tick, but only when the queue slot is empty,
        // so a queued request is not overwritten on every frame.
        if !root.as_os_str().is_empty() && self.queued_refresh.is_none() {
            match self.tab {
                // 0 = Core (Stable), 1 = Core (All) — share the same scan task.
                0 | 1 if self.core.loaded_for.as_deref() != Some(&root) => {
                    let env_vars = crate::core::env::build(&cfg.network);
                    self.spawn_auto(core_tab::load_request_with_env(
                        root,
                        env_vars,
                        LIST_MAX_NUM,
                    ));
                }
                2 if self.ext.loaded_for.as_deref() != Some(&root) => {
                    self.populate_ext_local(&root, LIST_MAX_NUM);
                    let env_vars = crate::core::env::build(&cfg.network);
                    self.spawn_auto(extensions_tab::load_request_with_limit(
                        root,
                        LIST_MAX_NUM,
                        env_vars,
                    ));
                }
                3 if self.ext.loaded_for.as_deref() != Some(&root) => {
                    // Install New needs the local list for install-state
                    // comparison; populate synchronously so catalog rows
                    // render with correct Installed badges immediately.
                    self.populate_ext_local(&root, LIST_MAX_NUM);
                    let env_vars = crate::core::env::build(&cfg.network);
                    self.spawn_auto(extensions_tab::load_request_with_limit(
                        root,
                        LIST_MAX_NUM,
                        env_vars,
                    ));
                }
                3 if !self.install.catalog_loaded => {
                    let env_vars = crate::core::env::build(&cfg.network);
                    self.spawn_auto(install_tab::fetch_registry_request(env_vars));
                }
                _ => {}
            }
        }
    }

    /// Drains the active slot's channel, routes `Progress` into the
    /// slot's own field, hands data results to `apply_result`, and on
    /// disconnect clears the slot and queues the chained `then`.
    fn drain_slot(&mut self, for_refresh: bool) {
        let slot = if for_refresh {
            &mut self.refresh
        } else {
            &mut self.pending
        };
        if slot.is_none() {
            return;
        }
        let mut to_apply: Vec<TaskResult> = Vec::new();
        let mut finished_then: Option<TaskKind> = None;
        if let Some(p) = slot {
            loop {
                match p.rx.try_recv() {
                    Ok(TaskResult::Progress { done, total }) => {
                        p.progress = Some((done, total));
                    }
                    Ok(res) => to_apply.push(res),
                    Err(mpsc::TryRecvError::Empty) => break,
                    Err(mpsc::TryRecvError::Disconnected) => {
                        finished_then = Some(p.then.clone());
                        break;
                    }
                }
            }
        }
        for r in to_apply {
            self.apply_result(r);
        }
        if let Some(then) = finished_then {
            if for_refresh {
                self.refresh = None;
            } else {
                self.pending = None;
            }
            self.queue_kind(then);
        }
    }

    fn apply_result(&mut self, r: TaskResult) {
        match r {
            TaskResult::CoreData {
                commits,
                tags,
                current,
                current_tag,
                branch,
                remote,
                root,
                requested_limit,
            } => {
                self.core.end_reached = commits.len() < requested_limit;
                self.core.limit = requested_limit;
                self.core.commits = commits;
                self.core.tags = tags;
                self.core.current = current;
                self.core.current_tag = current_tag;
                self.core.branch = branch;
                self.core.remote = remote;
                self.core.loaded_for = Some(root);
                if self.core.all_list_selected >= self.core.commits.len() {
                    self.core.all_list_selected = self.core.commits.len().saturating_sub(1);
                }
                if self.core.stable_list_selected >= self.core.tags.len() {
                    self.core.stable_list_selected = self.core.tags.len().saturating_sub(1);
                }
                self.core.restore_filter_state();
                self.core.ensure_visible();
            }
            TaskResult::ExtData {
                items,
                root,
                requested_limit,
            } => {
                self.ext.end_reached = items.len() < requested_limit;
                self.ext.limit = requested_limit;
                self.ext.items = items;
                self.ext.loaded_for = Some(root);
                let n = self.ext.filtered_indices().len();
                if self.ext.selected >= n {
                    self.ext.selected = n.saturating_sub(1);
                }
                self.ext.ensure_visible();
            }
            TaskResult::RegistryData { entries } => {
                self.install.catalog = entries;
                self.install.catalog_loaded = true;
                if self.install.catalog_selected >= self.install.catalog.len() {
                    self.install.catalog_selected = self.install.catalog.len().saturating_sub(1);
                }
            }
            // Progress is consumed inside `drain_slot`; reaching
            // `apply_result` is a no-op fallback.
            TaskResult::Progress { .. } => {}
            TaskResult::ExtRowUpdate { old_path, ext } => {
                if let Some(i) = self.ext.items.iter().position(|e| e.path == old_path) {
                    self.ext.items[i] = ext;
                }
            }
            TaskResult::ExtRowRemove { path } => {
                if let Some(i) = self.ext.items.iter().position(|e| e.path == path) {
                    self.ext.items.remove(i);
                    let n = self.ext.filtered_indices().len();
                    if self.ext.selected >= n {
                        self.ext.selected = n.saturating_sub(1);
                    }
                    self.ext.ensure_visible();
                }
            }
            TaskResult::ExtRowAdd { ext } => {
                // Replace any existing row with the same path; otherwise append.
                if let Some(i) = self.ext.items.iter().position(|e| e.path == ext.path) {
                    self.ext.items[i] = ext;
                } else {
                    self.ext.items.push(ext);
                }
                self.ext.items.sort_by(|a, b| a.name.cmp(&b.name));
            }
            TaskResult::CoreHeadUpdate {
                current,
                current_tag,
            } => {
                self.core.current = current;
                self.core.current_tag = current_tag;
            }
            TaskResult::ExtCommits {
                ext_path,
                commits,
                current,
                requested_limit,
            } => {
                if let Some(vp) = &mut self.ext.version_picker {
                    if vp.ext_path == ext_path {
                        vp.end_reached = commits.len() < requested_limit;
                        vp.limit = requested_limit;
                        vp.commits = commits;
                        vp.current = current;
                        if vp.selected >= vp.commits.len() {
                            vp.selected = vp.commits.len().saturating_sub(1);
                        }
                        vp.ensure_visible();
                    }
                }
            }
        }
    }

    /// Closes any open popup inside the Version Management screen.
    ///
    /// Returns whether a text input widget currently has keyboard focus.
    pub fn text_input_focused(&self) -> bool {
        match self.tab {
            0 | 1 => self.core.text_input_focused(),
            2 => self.ext.text_input_focused(),
            3 => self.install.text_input_focused(),
            _ => false,
        }
    }

    /// Returns whether Esc was consumed so the application does not
    /// re-focus the menu.
    pub fn eat_esc(&mut self) -> bool {
        if self.ext.notice.is_some() {
            self.ext.notice = None;
            return true;
        }
        if self.ext.actions_menu.is_some() {
            self.ext.actions_menu = None;
            return true;
        }
        if self.ext.version_picker.is_some() {
            self.ext.version_picker = None;
            return true;
        }
        if self.ext.confirm.is_some() {
            self.ext.confirm = None;
            self.ext.pending_delete = None;
            return true;
        }
        if self.install.notice.is_some() {
            self.install.notice = None;
            return true;
        }
        if self.install.actions_menu.is_some() {
            self.install.actions_menu = None;
            return true;
        }
        if self.core.eat_search_esc() {
            return true;
        }
        false
    }

    fn queue_kind(&mut self, kind: TaskKind) {
        // Chained re-scan after a mutation. Repopulate the relevant table
        // synchronously first so the new HEAD or row state is visible
        // immediately, then kick off the background refresh to update
        // `behind` and unmerged-upstream rows.
        match kind {
            TaskKind::None => {}
            TaskKind::CoreLoad(root) => {
                let limit = self.core.limit;
                self.populate_core_local(&root, limit);
                self.spawn_auto(core_tab::load_request_with_env(
                    root,
                    std::collections::HashMap::new(),
                    limit,
                ));
            }
            TaskKind::ExtLoad(root) => {
                let limit = self.ext.limit;
                self.populate_ext_local(&root, limit);
                self.spawn_auto(extensions_tab::load_request_with_limit(
                    root,
                    limit,
                    std::collections::HashMap::new(),
                ));
            }
        }
    }

    /// Manual dispatch from a user key or click.
    ///
    /// Mutations go to the blocking `pending` slot. Refresh requests are
    /// dropped silently when a refresh is already running.
    fn spawn(&mut self, req: TaskRequest) {
        if req.is_refresh {
            if self.refresh.is_some() {
                return;
            }
            self.spawn_inner(req);
        } else {
            self.spawn_inner(req);
        }
    }

    /// Auto dispatch from `tick` or a chained `then`.
    ///
    /// Refresh tasks landing on a busy slot are queued one deep.
    /// Mutations never use this path.
    fn spawn_auto(&mut self, req: TaskRequest) {
        if req.is_refresh {
            if self.refresh.is_some() {
                self.queued_refresh = Some(req);
                return;
            }
            self.spawn_inner(req);
        } else {
            self.spawn_inner(req);
        }
    }

    fn spawn_inner(&mut self, req: TaskRequest) {
        let (tx, rx) = mpsc::channel();
        let title = req.title.clone();
        let work = req.work;
        log_bus::push("task", format!("start: {title}"));
        thread::spawn(move || {
            work(tx);
        });
        let task = PendingTask {
            title,
            rx,
            then: req.then,
            progress: None,
        };
        if req.is_refresh {
            self.refresh = Some(task);
        } else {
            self.pending = Some(task);
        }
    }

    /// Renders the screen into `area`.
    pub fn render(&self, f: &mut Frame, area: Rect, cfg: &Config, body_active: bool) {
        let v = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(2), Constraint::Min(0)])
            .split(area);
        let names = vec![
            i18n::t("tab_core_stable"),
            i18n::t("tab_core_all"),
            i18n::t("tab_extensions"),
            i18n::t("tab_install"),
        ];
        Tabs {
            items: &names,
            selected: self.tab,
            highlighted: None,
        }
        .render(f, v[0]);
        // Keep the sub-tab rendered as active even while a mutation popup
        // is up so the button the user clicked stays highlighted
        // underneath the popup.
        let sub_active = body_active;
        match self.tab {
            0 | 1 => self.core.render(f, v[1], cfg, sub_active),
            2 => self.ext.render(f, v[1], cfg, sub_active),
            3 => self
                .install
                .render(f, v[1], cfg, &self.ext.items, sub_active),
            _ => {}
        }
        if let Some(p) = &self.pending {
            self.render_pending(f, area, &p.title, p.progress);
        }
    }

    fn render_pending(
        &self,
        f: &mut Frame,
        area: Rect,
        title: &str,
        progress: Option<(usize, usize)>,
    ) {
        let r = popup::center(area, area.width.saturating_sub(8).min(90), 14);
        popup::clear_widechar_safe(f, r);
        let suffix = match progress {
            Some((done, total)) => format!(" ({done}/{total})"),
            None => String::new(),
        };
        let block = Block::default()
            .borders(Borders::ALL)
            .border_type(theme::border_type(true))
            .border_style(theme::accent())
            .title(format!(
                " {}: {}{} ",
                i18n::t("popup_working"),
                title,
                suffix
            ));
        let inner = Rect {
            x: r.x + 1,
            y: r.y + 1,
            width: r.width - 2,
            height: r.height - 2,
        };
        f.render_widget(block, r);

        // Tail of the log bus for live progress.
        let snap = log_bus::snapshot();
        let tail_n = inner.height.saturating_sub(2) as usize;
        let start = snap.len().saturating_sub(tail_n);
        let mut lines: Vec<Line> = vec![
            Line::from(Span::styled(i18n::t("popup_please_wait"), theme::base())),
            Line::from(""),
        ];
        for l in &snap[start..] {
            lines.push(Line::from(vec![
                Span::styled(format!("{} ", l.ts), theme::base()),
                Span::styled(format!("[{}] ", l.source), theme::accent()),
                Span::raw(l.text.clone()),
            ]));
        }
        f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
    }

    fn split(area: Rect) -> (Rect, Rect) {
        let v = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(2), Constraint::Min(0)])
            .split(area);
        (v[0], v[1])
    }

    /// Handles a mouse event.
    pub fn on_mouse(&mut self, m: crossterm::event::MouseEvent, area: Rect, cfg: &Config) {
        if self.is_busy() {
            return;
        }
        let (tabs_area, body) = Self::split(area);
        if m.row >= tabs_area.y && m.row < tabs_area.y + tabs_area.height {
            let names = vec![
                i18n::t("tab_core_stable"),
                i18n::t("tab_core_all"),
                i18n::t("tab_extensions"),
                i18n::t("tab_install"),
            ];
            if let Some(h) = (crate::widgets::tabs::Tabs {
                items: &names,
                selected: self.tab,
                highlighted: None,
            })
            .hit(tabs_area, m.column)
            {
                use crate::widgets::tabs::HitResult;
                match h {
                    HitResult::Tab(t) => {
                        if t != self.tab {
                            self.tab = t;
                        }
                    }
                    HitResult::PrevChevron => {
                        self.tab = (self.tab + 3) % 4;
                    }
                    HitResult::NextChevron => {
                        self.tab = (self.tab + 1) % 4;
                    }
                }
                self.sync_core_filter();
            }
            return;
        }
        self.sync_core_filter();
        let req = match self.tab {
            0 | 1 => self.core.on_mouse(m, body, cfg),
            2 => self.ext.on_mouse(m, body, cfg),
            3 => {
                let items = self.ext.items.clone();
                self.install.on_mouse(m, body, cfg, &items)
            }
            _ => None,
        };
        // List-row clicks spawn immediately because the select-then-
        // activate two-click flow has already shown the row selected.
        // Action buttons defer through their Button widget.
        if let Some(req) = req {
            self.spawn(req);
        }
    }

    /// Pushes the screen-level `tab` index into the Core sub-tab's
    /// `filter` field. Called whenever the user might have switched tabs.
    fn sync_core_filter(&mut self) {
        self.core.filter = match self.tab {
            0 => core_tab::CoreFilter::Stable,
            1 => core_tab::CoreFilter::All,
            _ => self.core.filter,
        };
    }

    /// Handles a wheel-scroll event. Routes to the version picker first,
    /// then to the active sub-tab. Ignored while a mutation is in flight.
    pub fn scroll(&mut self, delta: i32, _cfg: &Config) {
        if self.is_busy() {
            return;
        }
        if let Some(vp) = &mut self.ext.version_picker {
            let n = vp.commits.len();
            if n == 0 {
                return;
            }
            if delta < 0 {
                if vp.selected == 0 {
                    return;
                }
                vp.selected -= 1;
            } else {
                if vp.selected + 1 >= n {
                    return;
                }
                vp.selected += 1;
            }
            vp.ensure_visible();
            return;
        }
        self.sync_core_filter();
        match self.tab {
            0 | 1 => self.core.scroll(delta),
            2 => self.ext.scroll(delta),
            3 => self.install.scroll(delta),
            _ => {}
        }
    }

    /// Handles a key event.
    pub fn on_key(&mut self, code: KeyCode, cfg: &Config) {
        if self.is_busy() {
            return;
        }
        match code {
            KeyCode::Left => {
                let consumed = match self.tab {
                    0 | 1 => self.core.on_left(),
                    2 => self.ext.on_left(),
                    3 => self.install.on_left(),
                    _ => false,
                };
                if !consumed {
                    self.tab = (self.tab + 3) % 4;
                    self.sync_core_filter();
                }
            }
            KeyCode::Right => {
                let consumed = match self.tab {
                    0 | 1 => self.core.on_right(),
                    2 => self.ext.on_right(),
                    3 => self.install.on_right(),
                    _ => false,
                };
                if !consumed {
                    self.tab = (self.tab + 1) % 4;
                    self.sync_core_filter();
                }
            }
            _ => {
                self.sync_core_filter();
                let req = match self.tab {
                    0 | 1 => self.core.on_key(code, cfg),
                    2 => self.ext.on_key(code, cfg),
                    3 => {
                        let items = self.ext.items.clone();
                        self.install.on_key(code, cfg, &items)
                    }
                    _ => None,
                };
                if let Some(req) = req {
                    self.spawn(req);
                }
            }
        }
    }
}
