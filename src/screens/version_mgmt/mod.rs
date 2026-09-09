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
use crate::widgets::log_display::{
    LogBusSource, LogDisplay, LogDisplayOptions, LogRange, LogViewportMode,
};
use crate::widgets::popup;
use crate::widgets::popup::notice::{Notice, NoticeOutcome};
use crate::widgets::tabs::{Tabs, TabsState};
use crossterm::event::KeyCode;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders};
use ratatui::Frame;
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
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
    pub work: Box<dyn FnOnce(mpsc::Sender<TaskResult>) -> TaskOutcome + Send + 'static>,
    /// Follow-up task queued after this one completes.
    pub then: TaskKind,
    /// Whether this task uses the non-modal refresh presentation.
    ///
    /// Refresh tasks run in the background with progress in the top-right
    /// banner instead of the blocking working popup. A refresh may still run
    /// `git fetch`; `repository_access` independently controls exclusion.
    pub is_refresh: bool,
    /// Whether successful completion may have changed a repository or
    /// extension on disk. Some read-only tasks are modal but must not
    /// invalidate repository snapshots or display a mutation-success banner.
    pub changes_repository: bool,
    /// Whether this task performs Git or repository-tree writes that must not
    /// overlap another top-level repository task.
    pub repository_access: RepositoryAccess,
}

/// Scheduling class for operations that touch repositories on disk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RepositoryAccess {
    /// Pure local reads or unrelated network/cache work.
    None,
    /// Git writes or repository-tree mutations. Only one such top-level task
    /// runs at once; a batch may still use its configured internal workers.
    Exclusive,
}

/// Identifier for a follow-up task to queue after the current one finishes.
#[derive(Clone)]
pub enum TaskKind {
    /// No follow-up.
    None,
    /// Reload the Core commit list from the given repository path.
    CoreLoad {
        root: PathBuf,
        env: std::collections::HashMap<String, String>,
        limit: usize,
    },
    /// Reload the extensions list from the given ComfyUI root.
    ExtLoad {
        root: PathBuf,
        env: std::collections::HashMap<String, String>,
        limit: usize,
        git_concurrency: usize,
    },
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
    /// Non-fatal warning emitted while a task continues running.
    Warning { message: String },
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
    /// Terminal status sent by the task wrapper after all data/progress
    /// events. Unlike channel disconnect, this distinguishes success,
    /// partial success, and a real failure.
    Finished(TaskOutcome),
}

/// Emits one filesystem-agnostic, non-blocking storage warning before a
/// write-heavy operation. Callers continue regardless of the probe result.
pub fn warn_if_storage_low(tx: &mpsc::Sender<TaskResult>, path: &Path) {
    let snapshot = match crate::core::storage::inspect(path) {
        Ok(snapshot) => snapshot,
        Err(error) => {
            log_bus::push(
                "storage",
                format!("could not inspect storage near {}: {error}", path.display()),
            );
            return;
        }
    };
    let pressure = snapshot.pressure();
    if !pressure.any() {
        return;
    }

    let mut details = Vec::new();
    if pressure.bytes {
        let available = crate::core::storage::format_bytes(snapshot.available_bytes);
        let percent = format!("{:.2}", snapshot.available_bytes_percent());
        details.push(i18n::t_args(
            "storage_warning_space",
            &[("available", &available), ("percent", &percent)],
        ));
    }
    if pressure.file_slots {
        if let (Some(available), Some(percent)) = (
            snapshot.available_file_slots,
            snapshot.available_file_slots_percent(),
        ) {
            let available = available.to_string();
            let percent = format!("{percent:.2}");
            details.push(i18n::t_args(
                "storage_warning_files",
                &[("available", &available), ("percent", &percent)],
            ));
        }
    }
    let path = snapshot.path.display().to_string();
    let details = details.join("; ");
    let message = i18n::t_args("storage_warning", &[("path", &path), ("details", &details)]);
    log_bus::push("storage", &message);
    let _ = tx.send(TaskResult::Warning { message });
}

/// One failed item/stage inside a task.
#[derive(Debug, Clone)]
pub struct ItemFailure {
    pub item: String,
    pub stage: String,
    pub message: String,
}

impl ItemFailure {
    pub fn new(
        item: impl Into<String>,
        stage: impl Into<String>,
        message: impl Into<String>,
    ) -> Self {
        Self {
            item: item.into(),
            stage: stage.into(),
            message: message.into(),
        }
    }
}

/// Final mutation result. `PartialFailure` means some durable state changed
/// (for example Git succeeded but pip failed, or only part of Update All
/// completed), so callers must refresh from disk rather than roll back the UI.
#[derive(Debug, Clone)]
pub enum TaskOutcome {
    Success,
    PartialFailure(Vec<ItemFailure>),
    Failure(Vec<ItemFailure>),
}

fn outcome_allows_follow_up(outcome: &TaskOutcome) -> bool {
    !matches!(outcome, TaskOutcome::Failure(_))
}

impl TaskOutcome {
    pub fn failure(
        item: impl Into<String>,
        stage: impl Into<String>,
        message: impl Into<String>,
    ) -> Self {
        Self::Failure(vec![ItemFailure::new(item, stage, message)])
    }
}

struct PendingTask {
    title: String,
    rx: mpsc::Receiver<TaskResult>,
    then: TaskKind,
    progress: Option<(usize, usize)>,
    changes_repository: bool,
    repository_access: RepositoryAccess,
    warnings: Vec<String>,
}

/// Version management screen state.
pub struct VersionMgmt {
    /// Active tab index.
    pub tab: usize,
    /// Persistent horizontal viewport for the version-management tab strip.
    tabs_state: TabsState,
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
    /// FIFO of automatic refreshes, promoted into `refresh` as each prior
    /// request finishes. Manual refreshes bypass this queue.
    queued_refresh: VecDeque<TaskRequest>,
    /// A user mutation waiting for a background Git-writing refresh to finish.
    /// Input is blocked while present, so this queue is intentionally one deep.
    queued_mutation: Option<TaskRequest>,
    /// Persistent result popup for failed/partially failed mutations.
    result_notice: Option<Notice>,
    /// Shared tail-following log display used by the blocking task popup.
    pending_logs: LogDisplay,
    /// One-shot task status promoted to the application banner.
    task_flash: Option<(crate::app::FlashKind, String)>,
    /// Set after a mutation changed repository state so the main-page info
    /// snapshot can be invalidated without polling Git from the UI thread.
    repo_changed: bool,
    core_requested_for: Option<PathBuf>,
    ext_requested_for: Option<PathBuf>,
    registry_requested: bool,
}

impl VersionMgmt {
    /// Constructs a fresh version management screen.
    pub fn new() -> Self {
        Self {
            tab: 0,
            tabs_state: TabsState::default(),
            core: core_tab::CoreTab::new(),
            ext: extensions_tab::ExtensionsTab::new(),
            install: install_tab::InstallTab::new(),
            pending: None,
            refresh: None,
            queued_refresh: VecDeque::new(),
            queued_mutation: None,
            result_notice: None,
            pending_logs: LogDisplay::new(),
            task_flash: None,
            repo_changed: false,
            core_requested_for: None,
            ext_requested_for: None,
            registry_requested: false,
        }
    }

    /// Returns whether a mutation popup is currently shown.
    ///
    /// Background refresh state is not included because it does not
    /// block input.
    pub fn is_busy(&self) -> bool {
        self.pending.is_some() || self.queued_mutation.is_some()
    }

    /// Drains transient flash messages from the Extensions and Install
    /// sub-tabs and surfaces them to the application.
    pub fn take_flash(&mut self) -> Option<(crate::app::FlashKind, String)> {
        self.task_flash
            .take()
            .or_else(|| self.ext.take_flash())
            .or_else(|| self.install.take_flash())
    }

    /// Returns and clears the repository-change notification consumed by the
    /// main screen's asynchronous information cache.
    pub fn take_repo_changed(&mut self) -> bool {
        std::mem::take(&mut self.repo_changed)
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
    pub fn tick(&mut self, cfg: &Config) -> bool {
        let mut changed = false;
        if let Some(notice) = &mut self.result_notice {
            if matches!(notice.tick(), Some(NoticeOutcome::Close)) {
                self.result_notice = None;
                changed = true;
            }
        }
        // Poll each sub-tab's persistent button widgets so the deferred
        // click-then-fire pipeline drains.
        let req = self
            .ext
            .poll_button_action(cfg)
            .or_else(|| self.install.poll_button_action(cfg));
        if let Some(req) = req {
            self.spawn(req);
            changed = true;
        }
        // Drain both task slots; the two share the same logic and route
        // Progress updates to the slot they came from.
        changed |= self.drain_slot(true);
        changed |= self.drain_slot(false);
        // A user mutation takes priority once the active repository-writing
        // refresh releases its exclusive slot.
        if self.pending.is_none()
            && self
                .queued_mutation
                .as_ref()
                .is_some_and(|req| !self.repository_conflict(req.repository_access))
        {
            if let Some(req) = self.queued_mutation.take() {
                self.spawn_inner(req);
                changed = true;
            }
        }
        // Promote an automatic refresh only when it does not conflict with a
        // running task or a user mutation waiting ahead of it.
        if self.refresh.is_none()
            && self.queued_mutation.is_none()
            && self
                .queued_refresh
                .front()
                .is_some_and(|req| !self.repository_conflict(req.repository_access))
        {
            if let Some(req) = self.queued_refresh.pop_front() {
                self.spawn_inner(req);
                changed = true;
            }
        }
        // Lazily kick off the initial load for the active tab as a
        // refresh task; `spawn_auto` queues it when another refresh is
        // already running.
        let root = std::path::Path::new(&cfg.general.comfyui_dir).to_path_buf();
        // Re-evaluate every tick, but only when the queue slot is empty,
        // so a queued request is not overwritten on every frame.
        if !root.as_os_str().is_empty() && self.queued_refresh.is_empty() {
            match self.tab {
                // 0 = Core (Stable), 1 = Core (All) — share the same scan task.
                0 | 1 if self.core.loaded_for.as_deref() != Some(&root) => {
                    if self.core_requested_for.as_deref() != Some(&root) {
                        self.core_requested_for = Some(root.clone());
                        let env_vars = crate::core::env::build(&cfg.network);
                        self.spawn_auto(core_tab::local_load_request(root, env_vars, LIST_MAX_NUM));
                        changed = true;
                    }
                }
                2 if self.ext.loaded_for.as_deref() != Some(&root) => {
                    if self.ext_requested_for.as_deref() != Some(&root) {
                        self.ext_requested_for = Some(root.clone());
                        let env_vars = crate::core::env::build(&cfg.network);
                        self.spawn_auto(extensions_tab::local_load_request(
                            root,
                            LIST_MAX_NUM,
                            env_vars,
                            cfg.general.git_concurrency,
                        ));
                        changed = true;
                    }
                }
                3 if self.ext.loaded_for.as_deref() != Some(&root) => {
                    if self.ext_requested_for.as_deref() != Some(&root) {
                        self.ext_requested_for = Some(root.clone());
                        let env_vars = crate::core::env::build(&cfg.network);
                        self.spawn_auto(extensions_tab::local_load_request(
                            root,
                            LIST_MAX_NUM,
                            env_vars,
                            cfg.general.git_concurrency,
                        ));
                        changed = true;
                    }
                }
                3 if !self.install.catalog_loaded && !self.registry_requested => {
                    self.registry_requested = true;
                    let env_vars = crate::core::env::build(&cfg.network);
                    self.spawn_auto(install_tab::fetch_registry_request(env_vars));
                    changed = true;
                }
                _ => {}
            }
        }
        changed
    }

    /// Drains the active slot's channel, routes `Progress` into the
    /// slot's own field, hands data results to `apply_result`, and on
    /// disconnect clears the slot and queues the chained `then`.
    fn drain_slot(&mut self, for_refresh: bool) -> bool {
        let slot = if for_refresh {
            &mut self.refresh
        } else {
            &mut self.pending
        };
        if slot.is_none() {
            return false;
        }
        let mut changed = false;
        let mut to_apply: Vec<TaskResult> = Vec::new();
        let mut finished: Option<(TaskKind, TaskOutcome, bool, Vec<String>)> = None;
        let mut new_warnings = Vec::new();
        if let Some(p) = slot {
            loop {
                match p.rx.try_recv() {
                    Ok(TaskResult::Progress { done, total }) => {
                        p.progress = Some((done, total));
                        changed = true;
                    }
                    Ok(TaskResult::Finished(outcome)) => {
                        finished = Some((
                            p.then.clone(),
                            outcome,
                            p.changes_repository,
                            p.warnings.clone(),
                        ));
                        break;
                    }
                    Ok(TaskResult::Warning { message }) => {
                        p.warnings.push(message.clone());
                        new_warnings.push(message);
                        changed = true;
                    }
                    Ok(res) => to_apply.push(res),
                    Err(mpsc::TryRecvError::Empty) => break,
                    Err(mpsc::TryRecvError::Disconnected) => {
                        finished = Some((
                            p.then.clone(),
                            TaskOutcome::failure(
                                &p.title,
                                "worker",
                                "background worker ended without a result",
                            ),
                            p.changes_repository,
                            p.warnings.clone(),
                        ));
                        break;
                    }
                }
            }
        }
        for r in to_apply {
            self.apply_result(r);
            changed = true;
        }
        if let Some(message) = new_warnings.last() {
            self.task_flash = Some((crate::app::FlashKind::Warning, message.clone()));
        }
        if let Some((then, outcome, changes_repository, warnings)) = finished {
            if for_refresh {
                self.refresh = None;
            } else {
                self.pending = None;
            }
            let run_follow_up = outcome_allows_follow_up(&outcome);
            self.handle_outcome(for_refresh, changes_repository, outcome, warnings);
            if run_follow_up {
                self.queue_kind(then);
            }
            changed = true;
        }
        changed
    }

    fn handle_outcome(
        &mut self,
        for_refresh: bool,
        changed: bool,
        outcome: TaskOutcome,
        warnings: Vec<String>,
    ) {
        match outcome {
            TaskOutcome::Success => {
                if !for_refresh && changed {
                    self.repo_changed = true;
                }
                if let Some(message) = warnings.last() {
                    self.task_flash = Some((crate::app::FlashKind::Warning, message.clone()));
                } else if !for_refresh && changed {
                    self.task_flash =
                        Some((crate::app::FlashKind::Info, i18n::t("task_result_success")));
                }
            }
            TaskOutcome::PartialFailure(failures) => {
                if changed {
                    self.repo_changed = true;
                }
                self.show_task_failures(true, failures, &warnings);
            }
            TaskOutcome::Failure(failures) => {
                if for_refresh {
                    let message = failures
                        .first()
                        .map(|f| format!("{}: {}", f.stage, f.message))
                        .unwrap_or_else(|| i18n::t("task_result_failed"));
                    log_bus::push("task", format!("refresh failed: {message}"));
                    self.task_flash = Some((crate::app::FlashKind::Error, message));
                } else {
                    self.show_task_failures(false, failures, &warnings);
                }
            }
        }
    }

    fn show_task_failures(
        &mut self,
        partial: bool,
        failures: Vec<ItemFailure>,
        warnings: &[String],
    ) {
        let title = if partial {
            i18n::t("task_result_partial")
        } else {
            i18n::t("task_result_failed")
        };
        let mut body = String::new();
        for failure in failures.iter().take(12) {
            if !body.is_empty() {
                body.push('\n');
            }
            body.push_str(&format!(
                "{} — {}: {}",
                failure.item, failure.stage, failure.message
            ));
        }
        if failures.len() > 12 {
            body.push_str(&format!("\n… +{}", failures.len() - 12));
        }
        for warning in warnings {
            body.push_str(&format!("\n\n⚠ {warning}"));
        }
        if let Some(path) = log_bus::file_path() {
            body.push_str(&format!("\n\n{}", path.display()));
        }
        self.result_notice = Some(Notice::new(title, body, None));
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
                self.core_requested_for = None;
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
                self.ext_requested_for = None;
                let n = self.ext.filtered_indices().len();
                self.ext.grid.set_list_len(n);
                self.ext.ensure_visible();
            }
            TaskResult::RegistryData { entries } => {
                self.install.catalog = entries;
                self.install.catalog_loaded = true;
                self.registry_requested = false;
                self.install.grid.set_list_len(self.install.catalog.len());
            }
            // Progress is consumed inside `drain_slot`; reaching
            // `apply_result` is a no-op fallback.
            TaskResult::Progress { .. } => {}
            // Warnings are consumed inside `drain_slot` so they remain tied
            // to the task's final outcome.
            TaskResult::Warning { .. } => {}
            TaskResult::ExtRowUpdate { old_path, ext } => {
                if let Some(i) = self.ext.items.iter().position(|e| e.path == old_path) {
                    self.ext.items[i] = ext;
                }
            }
            TaskResult::ExtRowRemove { path } => {
                if let Some(i) = self.ext.items.iter().position(|e| e.path == path) {
                    self.ext.items.remove(i);
                    let n = self.ext.filtered_indices().len();
                    self.ext.grid.set_list_len(n);
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
            TaskResult::Finished(_) => {}
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
        if self.result_notice.is_some() {
            self.result_notice = None;
            return true;
        }
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
            TaskKind::CoreLoad { root, env, limit } => {
                self.spawn_auto(core_tab::load_request_with_env(root, env, limit));
            }
            TaskKind::ExtLoad {
                root,
                env,
                limit,
                git_concurrency,
            } => {
                self.spawn_auto(extensions_tab::load_request_with_limit(
                    root,
                    limit,
                    env,
                    git_concurrency,
                ));
            }
        }
    }

    fn repository_task_active(&self) -> bool {
        self.pending
            .as_ref()
            .is_some_and(|task| task.repository_access == RepositoryAccess::Exclusive)
            || self
                .refresh
                .as_ref()
                .is_some_and(|task| task.repository_access == RepositoryAccess::Exclusive)
    }

    fn repository_conflict(&self, access: RepositoryAccess) -> bool {
        access == RepositoryAccess::Exclusive && self.repository_task_active()
    }

    /// Manual dispatch from a user key or click.
    ///
    /// Mutations go to the blocking `pending` slot. Refresh requests are
    /// dropped silently when a refresh is already running.
    fn spawn(&mut self, req: TaskRequest) {
        if req.is_refresh {
            if self.refresh.is_some() || self.repository_conflict(req.repository_access) {
                return;
            }
            self.spawn_inner(req);
        } else if self.pending.is_none() && !self.repository_conflict(req.repository_access) {
            self.spawn_inner(req);
        } else if self.queued_mutation.is_none() {
            log_bus::push("task", format!("queued: {}", req.title));
            self.queued_mutation = Some(req);
        }
    }

    /// Auto dispatch from `tick` or a chained `then`.
    ///
    /// Refresh tasks landing on a busy slot are queued.
    /// Mutations never use this path.
    fn spawn_auto(&mut self, req: TaskRequest) {
        if req.is_refresh {
            if self.refresh.is_some()
                || self.repository_conflict(req.repository_access)
                || (self.queued_mutation.is_some()
                    && req.repository_access == RepositoryAccess::Exclusive)
            {
                self.queued_refresh.push_back(req);
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
            let outcome = work(tx.clone());
            let _ = tx.send(TaskResult::Finished(outcome));
        });
        let task = PendingTask {
            title,
            rx,
            then: req.then,
            progress: None,
            changes_repository: req.changes_repository,
            repository_access: req.repository_access,
            warnings: Vec::new(),
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
            state: &self.tabs_state,
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
        } else if let Some(req) = &self.queued_mutation {
            let title = i18n::t_args("task_waiting_repository", &[("title", &req.title)]);
            self.render_pending(f, area, &title, None);
        }
        if let Some(notice) = &self.result_notice {
            notice.render(f, area);
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
            width: r.width.saturating_sub(2),
            height: r.height.saturating_sub(2),
        };
        f.render_widget(block, r);

        let leading = [
            Line::from(Span::styled(i18n::t("popup_please_wait"), theme::base())),
            Line::from(""),
        ];
        let options = LogDisplayOptions {
            range: LogRange::Tail((inner.height as usize).saturating_mul(8).max(32)),
            viewport: LogViewportMode::Tail,
            leading_lines: &leading,
            min_log_rows: 1,
            ..LogDisplayOptions::default()
        };
        self.pending_logs.render(f, inner, &LogBusSource, options);
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
        if let Some(notice) = &mut self.result_notice {
            if matches!(notice.on_mouse(m, area), Some(NoticeOutcome::Close)) {
                self.result_notice = None;
            }
            return;
        }
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
                state: &self.tabs_state,
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
        if let Some(notice) = &mut self.result_notice {
            if matches!(notice.on_key(code), Some(NoticeOutcome::Close)) {
                self.result_notice = None;
            }
            return;
        }
        if self.is_busy() {
            return;
        }
        match code {
            KeyCode::Left => {
                if self.current_tab_popup_open() {
                    self.dispatch_current_tab_key(code, cfg);
                    return;
                }
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
                if self.current_tab_popup_open() {
                    self.dispatch_current_tab_key(code, cfg);
                    return;
                }
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
                self.dispatch_current_tab_key(code, cfg);
            }
        }
    }

    fn current_tab_popup_open(&self) -> bool {
        match self.tab {
            2 => {
                self.ext.notice.is_some()
                    || self.ext.actions_menu.is_some()
                    || self.ext.version_picker.is_some()
                    || self.ext.confirm.is_some()
            }
            3 => self.install.notice.is_some() || self.install.actions_menu.is_some(),
            _ => false,
        }
    }

    fn dispatch_current_tab_key(&mut self, code: KeyCode, cfg: &Config) {
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;

    #[test]
    fn failed_mutation_does_not_run_its_refresh_follow_up() {
        let failure = TaskOutcome::failure("repo", "git", "exit 128");
        assert!(!outcome_allows_follow_up(&failure));
        assert!(outcome_allows_follow_up(&TaskOutcome::Success));
        assert!(outcome_allows_follow_up(&TaskOutcome::PartialFailure(
            vec![ItemFailure::new("repo", "pip", "failed"),]
        )));
    }

    #[test]
    fn mutation_waits_for_repository_writing_refresh() {
        let mut screen = VersionMgmt::new();
        let (release_tx, release_rx) = mpsc::channel();
        screen.spawn_auto(TaskRequest {
            title: "repository refresh".into(),
            work: Box::new(move |_| {
                let _ = release_rx.recv();
                TaskOutcome::Success
            }),
            then: TaskKind::None,
            is_refresh: true,
            changes_repository: false,
            repository_access: RepositoryAccess::Exclusive,
        });

        let ran = Arc::new(AtomicBool::new(false));
        let ran_in_task = ran.clone();
        screen.spawn(TaskRequest {
            title: "manual update".into(),
            work: Box::new(move |_| {
                ran_in_task.store(true, Ordering::SeqCst);
                TaskOutcome::Success
            }),
            then: TaskKind::None,
            is_refresh: false,
            changes_repository: true,
            repository_access: RepositoryAccess::Exclusive,
        });

        assert!(screen.refresh.is_some());
        assert!(screen.pending.is_none());
        assert!(screen.queued_mutation.is_some());
        assert!(screen.is_busy());
        assert!(!ran.load(Ordering::SeqCst));

        release_tx.send(()).unwrap();
        let cfg = Config::default();
        for _ in 0..1000 {
            screen.tick(&cfg);
            if ran.load(Ordering::SeqCst) {
                break;
            }
            std::thread::yield_now();
        }
        assert!(ran.load(Ordering::SeqCst));
    }
}
