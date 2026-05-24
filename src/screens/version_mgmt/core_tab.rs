//! Core ComfyUI repository tab.
//!
//! Lists release tags or commits and provides actions to change version,
//! pull, and reinstall requirements.

use super::{TaskKind, TaskRequest, TaskResult, LIST_MAX_NUM};
use crate::core::config::Config;
use crate::core::{env, git, i18n, pip, theme};
use crate::widgets::focus_grid::{FocusGrid, RowKind};
use crate::widgets::input::Input;
use crate::widgets::table::{Column, Table};
use crossterm::event::KeyCode;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;
use std::cell::Cell;
use std::path::PathBuf;

/// Which slice of the Core repository history to display.
#[derive(Copy, Clone, PartialEq, Eq)]
pub enum CoreFilter {
    /// `v<major>.<minor>.<patch>` release tags only.
    Stable,
    /// Full commit log.
    All,
}

/// State for the Core repository tab.
pub struct CoreTab {
    /// Commit list, newest first.
    pub commits: Vec<git::Commit>,
    /// Release tags, descending by version.
    pub tags: Vec<git::TagCommit>,
    /// Active view filter.
    pub filter: CoreFilter,
    /// Centralized focus grid: Row 0 = search input, Row 1 = commit/tag list.
    pub grid: FocusGrid,
    /// Saved list_selected for the Stable view.
    pub stable_list_selected: usize,
    /// Saved list_scroll for the Stable view.
    pub stable_list_scroll: usize,
    /// Saved list_selected for the All view.
    pub all_list_selected: usize,
    /// Saved list_scroll for the All view.
    pub all_list_scroll: usize,
    /// Short SHA of the current `HEAD`.
    pub current: Option<String>,
    /// `v<major>.<minor>.<patch>` tag at `HEAD`, when one applies.
    pub current_tag: Option<String>,
    /// Remote origin URL.
    pub remote: Option<String>,
    /// Current branch.
    pub branch: Option<String>,
    /// Path the current data was loaded for.
    pub loaded_for: Option<PathBuf>,
    /// Row count requested from git for the current `commits` snapshot.
    pub limit: usize,
    /// Whether the last load returned fewer rows than requested.
    pub end_reached: bool,
    /// Number of data rows the table actually displays per frame.
    pub visible_rows: Cell<usize>,
    /// Search input filtering the table.
    pub search: Input,
}

impl CoreTab {
    /// Constructs a fresh Core tab.
    pub fn new() -> Self {
        let mut grid = FocusGrid::new(vec![RowKind::Fixed(1), RowKind::List]);
        grid.set_focus(1, 0);
        Self {
            commits: vec![],
            tags: vec![],
            filter: CoreFilter::Stable,
            grid,
            stable_list_selected: 0,
            stable_list_scroll: 0,
            all_list_selected: 0,
            all_list_scroll: 0,
            current: None,
            current_tag: None,
            remote: None,
            branch: None,
            loaded_for: None,
            limit: LIST_MAX_NUM,
            end_reached: false,
            visible_rows: Cell::new(0),
            search: Input::default().placeholder("placeholder_search"),
        }
    }

    /// Whether the search input currently has keyboard focus.
    pub fn search_focused(&self) -> bool {
        self.grid.row() == 0
    }

    /// Whether any text input widget currently has keyboard focus.
    pub fn text_input_focused(&self) -> bool {
        self.search_focused()
    }

    /// Saves grid list state to the current filter's storage.
    pub fn save_filter_state(&mut self) {
        match self.filter {
            CoreFilter::Stable => {
                self.stable_list_selected = self.grid.list_selected();
                self.stable_list_scroll = self.grid.list_scroll();
            }
            CoreFilter::All => {
                self.all_list_selected = self.grid.list_selected();
                self.all_list_scroll = self.grid.list_scroll();
            }
        }
    }

    /// Restores grid list state from the given filter's storage.
    pub fn restore_filter_state(&mut self) {
        match self.filter {
            CoreFilter::Stable => {
                self.grid.set_list_selected(self.stable_list_selected);
                self.grid.set_list_scroll(self.stable_list_scroll);
            }
            CoreFilter::All => {
                self.grid.set_list_selected(self.all_list_selected);
                self.grid.set_list_scroll(self.all_list_scroll);
            }
        }
    }

    /// Clamps scroll so the selected row of the active view is on-screen.
    pub fn ensure_visible(&mut self) {
        let v = self.visible_rows.get().max(1);
        self.grid.set_visible_rows(v);
        self.grid.ensure_visible();
        // Sync saved state.
        self.save_filter_state();
    }

    /// Returns indices of commits matching the search input.
    pub fn filtered_commits(&self) -> Vec<usize> {
        let needle = self.search.value.to_lowercase();
        if needle.is_empty() {
            return (0..self.commits.len()).collect();
        }
        self.commits
            .iter()
            .enumerate()
            .filter(|(_, c)| {
                c.short.to_lowercase().contains(&needle)
                    || c.subject.to_lowercase().contains(&needle)
            })
            .map(|(i, _)| i)
            .collect()
    }

    /// Returns indices of tags matching the search input.
    pub fn filtered_tags(&self) -> Vec<usize> {
        let needle = self.search.value.to_lowercase();
        if needle.is_empty() {
            return (0..self.tags.len()).collect();
        }
        self.tags
            .iter()
            .enumerate()
            .filter(|(_, t)| {
                t.tag.to_lowercase().contains(&needle)
                    || t.commit_short.to_lowercase().contains(&needle)
                    || t.commit_subject.to_lowercase().contains(&needle)
            })
            .map(|(i, _)| i)
            .collect()
    }

    /// Returns the filtered list length for the active view.
    fn filtered_len(&self) -> usize {
        match self.filter {
            CoreFilter::All => self.filtered_commits().len(),
            CoreFilter::Stable => self.filtered_tags().len(),
        }
    }

    /// Renders the tab into `area`.
    pub fn render(&self, f: &mut Frame, area: Rect, cfg: &Config, body_active: bool) {
        let v = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(4),
                Constraint::Length(3),
                Constraint::Min(0),
            ])
            .split(area);
        let head_lines = vec![
            Line::from(vec![
                Span::styled(format!("{}: ", i18n::t("label_remote")), theme::base()),
                Span::raw(self.remote.clone().unwrap_or_default()),
            ]),
            Line::from(vec![
                Span::styled(format!("{}: ", i18n::t("label_branch")), theme::base()),
                Span::raw(self.branch.clone().unwrap_or_default()),
                Span::raw("    "),
                Span::styled(i18n::t("label_head"), theme::base()),
                Span::raw(self.current.clone().unwrap_or_default()),
            ]),
        ];
        f.render_widget(
            Paragraph::new(head_lines).block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(theme::border())
                    .title(format!(
                        " {} — {} ",
                        cfg.general.comfyui_dir,
                        i18n::t("btn_update_one")
                    )),
            ),
            v[0],
        );

        let search_focused = body_active && self.search_focused();
        self.search.render(f, v[1], search_focused);

        let table_active = body_active && !self.search_focused();
        let visible = (v[2].height as usize).saturating_sub(3);
        self.visible_rows.set(visible);
        let (sel, scr) = (self.grid.list_selected(), self.grid.list_scroll());
        match self.filter {
            CoreFilter::All => {
                let filtered = self.filtered_commits();
                render_commit_table_filtered(
                    f,
                    v[2],
                    &self.commits,
                    &filtered,
                    self.current.as_ref(),
                    sel,
                    scr,
                    table_active,
                );
            }
            CoreFilter::Stable => {
                let filtered = self.filtered_tags();
                render_tag_table_filtered(
                    f,
                    v[2],
                    &self.tags,
                    &filtered,
                    self.current_tag.as_ref(),
                    sel,
                    scr,
                    table_active,
                );
            }
        }
    }

    /// Handles a mouse event.
    pub fn on_mouse(
        &mut self,
        m: crossterm::event::MouseEvent,
        area: Rect,
        cfg: &Config,
    ) -> Option<TaskRequest> {
        // Layout: header (4) + search (3) + table (rest).
        let search_top = area.y + 4;
        let search_bottom = search_top + 3;
        if m.row >= search_top && m.row < search_bottom {
            self.grid.set_focus(0, 0);
            return None;
        }
        self.grid.set_focus(1, 0);
        // Table content starts after table border (1) + header row (1).
        let table_top = search_bottom + 1 + 1;
        if m.row < table_top {
            return None;
        }
        let rel = (m.row - table_top) as usize;
        let filtered = match self.filter {
            CoreFilter::All => self.filtered_commits(),
            CoreFilter::Stable => self.filtered_tags(),
        };
        let vi = self.grid.list_scroll() + rel;
        if vi >= filtered.len() {
            return None;
        }
        if vi != self.grid.list_selected() {
            self.grid.set_list_selected(vi);
            return None;
        }
        let fi = filtered[vi];
        let root = PathBuf::from(&cfg.general.comfyui_dir);
        match self.filter {
            CoreFilter::All => {
                let c = self.commits[fi].clone();
                Some(checkout_request(
                    root,
                    c.short,
                    env::build(&cfg.network),
                    cfg.general.python.clone(),
                ))
            }
            CoreFilter::Stable => {
                let t = self.tags[fi].clone();
                Some(checkout_request(
                    root,
                    t.tag,
                    env::build(&cfg.network),
                    cfg.general.python.clone(),
                ))
            }
        }
    }

    /// Handles a wheel-scroll event.
    pub fn scroll(&mut self, delta: i32) {
        let n = self.filtered_len();
        if n == 0 {
            return;
        }
        self.grid.set_list_len(n);
        self.grid.scroll(delta);
        self.save_filter_state();
    }

    /// Closes the search focus or clears text. Returns whether Esc was consumed.
    pub fn eat_search_esc(&mut self) -> bool {
        if self.search_focused() {
            self.grid.set_focus(1, 0);
            return true;
        }
        if !self.search.value.is_empty() {
            self.search.value.clear();
            self.search.cursor = 0;
            self.grid.set_list_selected(0);
            self.grid.set_list_scroll(0);
            self.save_filter_state();
            return true;
        }
        false
    }

    /// Attempts to handle a Left arrow within this tab.
    pub fn on_left(&mut self) -> bool {
        if self.search_focused() && !self.search.at_start() {
            self.search.on_key(KeyCode::Left);
            return true;
        }
        self.grid.move_left()
    }

    /// Attempts to handle a Right arrow within this tab.
    pub fn on_right(&mut self) -> bool {
        if self.search_focused() && !self.search.at_end() {
            self.search.on_key(KeyCode::Right);
            return true;
        }
        self.grid.move_right()
    }

    /// Handles a key event.
    pub fn on_key(&mut self, code: KeyCode, cfg: &Config) -> Option<TaskRequest> {
        let root = PathBuf::from(&cfg.general.comfyui_dir);
        let n = self.filtered_len();
        self.grid.set_list_len(n);
        self.grid.set_visible_rows(self.visible_rows.get().max(1));

        // Search input has focus.
        if self.search_focused() {
            match code {
                KeyCode::Esc => {
                    self.grid.set_focus(1, 0);
                }
                KeyCode::Enter | KeyCode::Down => {
                    self.grid.move_down();
                }
                KeyCode::Up => {
                    self.grid.move_up();
                }
                KeyCode::Tab | KeyCode::BackTab => {
                    self.grid.set_focus(1, 0);
                }
                k if !matches!(k, KeyCode::Left | KeyCode::Right) => {
                    self.search.on_key(k);
                    self.grid.set_list_selected(0);
                    self.grid.set_list_scroll(0);
                }
                _ => {}
            }
            self.save_filter_state();
            return None;
        }

        // Table has focus.
        let sel = self.grid.list_selected();
        match code {
            KeyCode::Tab | KeyCode::BackTab => {
                self.grid.set_focus(0, 0);
                None
            }
            KeyCode::Up | KeyCode::Down => {
                if code == KeyCode::Down
                    && sel + 1 >= n
                    && self.filter == CoreFilter::All
                    && !self.end_reached
                    && self.search.value.is_empty()
                {
                    let new_limit = self.limit.saturating_add(LIST_MAX_NUM);
                    return Some(load_request_with_env(
                        root,
                        env::build(&cfg.network),
                        new_limit,
                    ));
                }
                if code == KeyCode::Up {
                    self.grid.move_up();
                } else {
                    self.grid.move_down();
                }
                self.save_filter_state();
                None
            }
            KeyCode::PageUp => {
                self.grid.page_up();
                self.save_filter_state();
                None
            }
            KeyCode::PageDown => {
                self.grid.page_down();
                self.save_filter_state();
                None
            }
            KeyCode::Char('r') | KeyCode::Char('R') => Some(load_request_with_env(
                root,
                env::build(&cfg.network),
                self.limit,
            )),
            KeyCode::Char('u') | KeyCode::Char('U') => Some(pull_request(
                root.clone(),
                env::build(&cfg.network),
                cfg.general.python.clone(),
            )),
            KeyCode::Enter => {
                let filtered = match self.filter {
                    CoreFilter::All => self.filtered_commits(),
                    CoreFilter::Stable => self.filtered_tags(),
                };
                if let Some(&fi) = filtered.get(sel) {
                    match self.filter {
                        CoreFilter::All => {
                            let c = &self.commits[fi];
                            return Some(checkout_request(
                                root,
                                c.short.clone(),
                                env::build(&cfg.network),
                                cfg.general.python.clone(),
                            ));
                        }
                        CoreFilter::Stable => {
                            let t = &self.tags[fi];
                            return Some(checkout_request(
                                root,
                                t.tag.clone(),
                                env::build(&cfg.network),
                                cfg.general.python.clone(),
                            ));
                        }
                    }
                }
                None
            }
            _ => None,
        }
    }
}

// ── Filtered table renderers ──────────────────────────────────────────────

/// Renders commit table with only the filtered subset visible.
#[allow(clippy::too_many_arguments)]
fn render_commit_table_filtered(
    f: &mut Frame,
    area: Rect,
    commits: &[git::Commit],
    filtered: &[usize],
    current: Option<&String>,
    selected: usize,
    scroll: usize,
    active: bool,
) {
    let side: u16 = 10 + 20 + 8 + 4 + 2;
    let body_w = area.width.saturating_sub(side).max(1);
    let cols = vec![
        Column {
            title: i18n::t("label_version_id"),
            width: 10,
        },
        Column {
            title: i18n::t("label_update_content"),
            width: body_w,
        },
        Column {
            title: i18n::t("label_date"),
            width: 20,
        },
        Column {
            title: i18n::t("label_current"),
            width: 8,
        },
    ];
    Table {
        columns: &cols,
        row_count: filtered.len(),
        selected,
        scroll,
    }
    .render(
        f,
        area,
        |i| {
            let c = &commits[filtered[i]];
            let cur = if Some(&c.short) == current {
                "[x]".into()
            } else {
                "[ ]".into()
            };
            vec![c.short.clone(), c.subject.clone(), c.date.clone(), cur]
        },
        active,
    );
}

/// Renders tag table with only the filtered subset visible.
#[allow(clippy::too_many_arguments)]
fn render_tag_table_filtered(
    f: &mut Frame,
    area: Rect,
    tags: &[git::TagCommit],
    filtered: &[usize],
    current_tag: Option<&String>,
    selected: usize,
    scroll: usize,
    active: bool,
) {
    let side: u16 = 12 + 10 + 12 + 8 + 4 + 2;
    let body_w = area.width.saturating_sub(side).max(1);
    let cols = vec![
        Column {
            title: i18n::t("label_tag"),
            width: 12,
        },
        Column {
            title: i18n::t("label_version_id"),
            width: 10,
        },
        Column {
            title: i18n::t("label_date"),
            width: 12,
        },
        Column {
            title: i18n::t("label_update_content"),
            width: body_w,
        },
        Column {
            title: i18n::t("label_current"),
            width: 8,
        },
    ];
    Table {
        columns: &cols,
        row_count: filtered.len(),
        selected,
        scroll,
    }
    .render(
        f,
        area,
        |i| {
            let t = &tags[filtered[i]];
            let cur = if Some(&t.tag) == current_tag {
                "[x]".into()
            } else {
                "[ ]".into()
            };
            vec![
                t.tag.clone(),
                t.commit_short.clone(),
                t.commit_date.clone(),
                t.commit_subject.clone(),
                cur,
            ]
        },
        active,
    );
}

/// Renders a shared commit table used by the Extensions version picker.
pub fn render_commit_table(
    f: &mut Frame,
    area: Rect,
    commits: &[git::Commit],
    current: Option<&String>,
    selected: usize,
    scroll: usize,
    active: bool,
) {
    let side: u16 = 10 + 20 + 8 + 4 + 2;
    let body_w = area.width.saturating_sub(side).max(1);
    let cols = vec![
        Column {
            title: i18n::t("label_version_id"),
            width: 10,
        },
        Column {
            title: i18n::t("label_update_content"),
            width: body_w,
        },
        Column {
            title: i18n::t("label_date"),
            width: 20,
        },
        Column {
            title: i18n::t("label_current"),
            width: 8,
        },
    ];
    Table {
        columns: &cols,
        row_count: commits.len(),
        selected,
        scroll,
    }
    .render(
        f,
        area,
        |i| {
            let c = &commits[i];
            let cur = if Some(&c.short) == current {
                "[x]".into()
            } else {
                "[ ]".into()
            };
            vec![c.short.clone(), c.subject.clone(), c.date.clone(), cur]
        },
        active,
    );
}

/// Full local snapshot returned by [`scan_local`].
pub struct CoreScan {
    /// Recent commits, newest first.
    pub commits: Vec<git::Commit>,
    /// Release tags, descending by version.
    pub tags: Vec<git::TagCommit>,
    /// Short SHA of the current `HEAD`.
    pub current: Option<String>,
    /// Release tag at `HEAD`, when one applies.
    pub current_tag: Option<String>,
    /// Current branch.
    pub branch: Option<String>,
    /// Remote origin URL.
    pub remote: Option<String>,
}

/// Reads a synchronous local snapshot of the Core repository without
/// fetching from the network.
pub fn scan_local(root: &std::path::Path, limit: usize) -> CoreScan {
    let current = git::current_commit(root);
    let branch = git::current_branch(root);
    let remote = git::remote_url(root);
    let commits = git::log_all(root, limit).unwrap_or_default();
    let tags = git::tags_pointing_at_releases(root);
    let current_tag = git::current_release_tag(root);
    CoreScan {
        commits,
        tags,
        current,
        current_tag,
        branch,
        remote,
    }
}

/// Builds a background task that fetches from origin and then reads the
/// last `limit` commits across every ref so unmerged upstream commits
/// remain visible.
pub fn load_request_with_env(
    root: PathBuf,
    env_vars: std::collections::HashMap<String, String>,
    limit: usize,
) -> TaskRequest {
    TaskRequest {
        title: i18n::t("task_core_load"),
        then: TaskKind::None,
        is_refresh: true,
        work: Box::new(move |tx| {
            // Fetch first so HEAD/remote refs are current, then deepen if the
            // repo is a shallow clone — otherwise `git log` would silently
            // stop at the shallow boundary and the user could never page past
            // it. Both calls are best-effort and ignore errors.
            let _ = git::fetch(&root, env_vars.clone());
            // Deepen until the repo is fully unshallowed (or we've truly tried).
            let _ = git::deepen_until_full(&root, env_vars);
            let current = git::current_commit(&root);
            let branch = git::current_branch(&root);
            let remote = git::remote_url(&root);
            let commits = git::log_all(&root, limit).unwrap_or_default();
            let tags = git::tags_pointing_at_releases(&root);
            let current_tag = git::current_release_tag(&root);
            let _ = tx.send(TaskResult::CoreData {
                commits,
                tags,
                current,
                current_tag,
                branch,
                remote,
                root,
                requested_limit: limit,
            });
        }),
    }
}

fn pull_request(
    root: PathBuf,
    env_vars: std::collections::HashMap<String, String>,
    python: String,
) -> TaskRequest {
    let then = TaskKind::CoreLoad(root.clone());
    TaskRequest {
        title: i18n::t("task_core_pull"),
        then,
        is_refresh: false,
        work: Box::new(move |_tx| {
            // Same fetch+reset strategy as extensions: works for detached
            // HEAD (from prior Change Version) and discards local edits to
            // tracked files (untracked files are kept).
            let _ = git::fetch(&root, env_vars.clone());
            // On a branch → @{u}; detached HEAD → origin/HEAD then FETCH_HEAD.
            let targets: &[&str] = if git::current_branch(&root).is_some() {
                &["@{u}"]
            } else {
                &["origin/HEAD", "FETCH_HEAD"]
            };
            for t in targets {
                if git::reset_hard(&root, t, env_vars.clone()).unwrap_or(false) {
                    break;
                }
            }
            if !python.is_empty() {
                let _ = pip::install_requirements(std::path::Path::new(&python), &root, env_vars);
            }
        }),
    }
}

fn checkout_request(
    root: PathBuf,
    rev: String,
    env_vars: std::collections::HashMap<String, String>,
    python: String,
) -> TaskRequest {
    let title = i18n::t_args("task_core_checkout", &[("rev", &rev)]);
    TaskRequest {
        title,
        // No full Core reload — Change Version is a single-entry mutation.
        // The commit list is unchanged; we only need to update which row is
        // marked as "Current". The user can hit R to fetch fresh upstream.
        then: TaskKind::None,
        is_refresh: false,
        work: Box::new(move |tx| {
            let _ = git::checkout(&root, &rev, env_vars.clone());
            if !python.is_empty() {
                let _ = pip::install_requirements(std::path::Path::new(&python), &root, env_vars);
            }
            let _ = tx.send(TaskResult::CoreHeadUpdate {
                current: git::current_commit(&root),
                current_tag: git::current_release_tag(&root),
            });
        }),
    }
}
