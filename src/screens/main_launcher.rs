//! Main launcher screen with the info list and the Launch / Activate buttons.

use crate::app::FlashKind;
use crate::core::config::Config;
use crate::core::paths::ComfyDirs;
use crate::core::schema::{self, Schema};
use crate::core::{clipboard, comfy_info, git, i18n, python, theme};
use crate::widgets::button::{Button, ButtonKind};
use crate::widgets::focus_grid::{FocusGrid, RowKind};
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;
use std::cell::Cell;
use std::path::{Path, PathBuf};
use unicode_width::UnicodeWidthStr;

/// One visible row in the scrollable info list.
pub enum Row {
    /// Non-interactive section header.
    Header(String),
    /// Label / value data row that can be focused and copied.
    Item {
        /// Row label.
        label: String,
        /// Row value.
        value: String,
    },
}

/// Action the application should perform after a Main Launcher interaction.
#[derive(Clone)]
pub enum MainAction {
    /// Nothing to do.
    None,
    /// Launch ComfyUI.
    Launch,
    /// Activate the given virtualenv after the run loop exits.
    ActivateVenv(PathBuf),
    /// Quit because the requested virtualenv is already activated.
    QuitAlreadyActivated,
    /// Show a transient banner.
    Flash(FlashKind, String),
}

/// Which control on the Main Launcher currently holds focus.
#[derive(Copy, Clone, PartialEq, Eq)]
pub enum MainFocus {
    /// The scrollable info list.
    List,
    /// The Activate Environment button.
    Activate,
    /// The Launch ComfyUI button.
    Launch,
}

/// Main launcher screen state.
pub struct MainLauncher {
    /// Centralized focus grid: Row 0 = list, Row 1 = [Activate, Launch].
    pub grid: FocusGrid,
    /// First absolute row to render.
    ///
    /// Re-clamped on every render so the selected row stays visible
    /// after a terminal resize or row list change.
    pub scroll: Cell<usize>,
    visible: Cell<usize>,
    /// Activate Environment button.
    pub btn_activate: Button,
    /// Launch ComfyUI button.
    pub btn_launch: Button,
}

impl MainLauncher {
    /// Constructs a fresh main launcher screen.
    pub fn new() -> Self {
        let mut grid = FocusGrid::new(vec![RowKind::List, RowKind::Fixed(2)]);
        // Default focus on Launch button (row 1, col 1).
        grid.set_focus(1, 1);
        Self {
            grid,
            scroll: Cell::new(0),
            visible: Cell::new(0),
            btn_activate: Button::new(ButtonKind::Primary),
            btn_launch: Button::new(ButtonKind::Primary),
        }
    }

    /// Returns the current focus as a `MainFocus` enum for external use.
    pub fn focus(&self) -> MainFocus {
        if self.grid.row() == 0 {
            MainFocus::List
        } else if self.grid.col() == 0 {
            MainFocus::Activate
        } else {
            MainFocus::Launch
        }
    }

    /// Polls the deferred-fire pipelines on the Launch and Activate
    /// buttons once per frame.
    pub fn poll_button_action(&mut self, cfg: &Config) -> MainAction {
        if self.btn_launch.poll_fire() {
            return MainAction::Launch;
        }
        if self.btn_activate.poll_fire() {
            return self.activate_venv(cfg);
        }
        MainAction::None
    }

    /// Esc handler. Returns whether the key was consumed.
    pub fn eat_esc(&mut self) -> bool {
        false
    }

    /// Reconstructs the exact command that F5 or Launch ComfyUI would run.
    ///
    /// Returns a shell-style string of the form
    /// `<python> <comfy_dir>/main.py <cli args>`, suitable for the user to
    /// copy and paste.
    fn launch_command(cfg: &Config, schema: &Schema) -> String {
        let python = if cfg.general.python.is_empty() {
            "python".to_string()
        } else {
            cfg.general.python.clone()
        };
        let main_py = PathBuf::from(&cfg.general.comfyui_dir).join("main.py");
        let args = schema::build_cli_args(schema, &cfg.comfy_settings);
        let mut parts: Vec<String> = Vec::with_capacity(2 + args.len());
        parts.push(shell_quote(&python));
        parts.push(shell_quote(&main_py.display().to_string()));
        for a in &args {
            parts.push(shell_quote(a));
        }
        parts.join(" ")
    }

    // ── data model ──────────────────────────────────────────────────────
    fn build_rows(cfg: &Config, schema: &Schema) -> Vec<Row> {
        let mut rows: Vec<Row> = Vec::new();
        let root = PathBuf::from(&cfg.general.comfyui_dir);

        rows.push(Row::Header(i18n::t("section_comfy_info")));
        rows.push(Row::Item {
            label: i18n::t("info_run_command"),
            value: Self::launch_command(cfg, schema),
        });
        rows.push(Row::Item {
            label: i18n::t("info_comfy_version"),
            value: comfy_info::comfyui_version(&root).unwrap_or_else(|| "?".into()),
        });
        if let Some(sha) = git::current_commit(&root) {
            rows.push(Row::Item {
                label: i18n::t("info_comfy_head"),
                value: sha,
            });
        }
        let py_value = if cfg.general.python.is_empty() {
            i18n::t("info_not_set")
        } else {
            python::validate(std::path::Path::new(&cfg.general.python)).unwrap_or_default()
        };
        rows.push(Row::Item {
            label: i18n::t("info_py_version"),
            value: py_value,
        });
        rows.push(Row::Item {
            label: i18n::t("info_launcher_version"),
            value: env!("CARGO_PKG_VERSION").to_string(),
        });
        let (enabled, disabled) = comfy_info::count_custom_nodes(&root);
        let cn_value = if disabled == 0 {
            enabled.to_string()
        } else {
            format!(
                "{enabled} ({})",
                i18n::t_args("popup_n_disabled", &[("n", &disabled.to_string())])
            )
        };
        rows.push(Row::Item {
            label: i18n::t("info_custom_nodes"),
            value: cn_value,
        });

        rows.push(Row::Header(i18n::t("section_directories")));
        let dirs = ComfyDirs::new(&root);
        rows.push(Row::Item {
            label: i18n::t("setting_comfy_dir"),
            value: pretty_path(&dirs.root),
        });
        rows.push(Row::Item {
            label: i18n::t("btn_custom_nodes"),
            value: pretty_path(&dirs.custom_nodes()),
        });
        rows.push(Row::Item {
            label: i18n::t("btn_input_dir"),
            value: pretty_path(&dirs.input()),
        });
        rows.push(Row::Item {
            label: i18n::t("btn_output_dir"),
            value: pretty_path(&dirs.output()),
        });
        rows.push(Row::Item {
            label: i18n::t("dir_models_base"),
            value: pretty_path(&root.join("models")),
        });
        for (group, cat, abs) in comfy_info::extra_model_paths(&root) {
            rows.push(Row::Item {
                label: i18n::t_args("dir_models_extra", &[("cat", &cat), ("group", &group)]),
                value: pretty_path(&abs),
            });
        }
        rows
    }

    /// Indices in `rows` that are `Row::Item` (the ones the cursor can land on).
    fn item_indices(rows: &[Row]) -> Vec<usize> {
        rows.iter()
            .enumerate()
            .filter_map(|(i, r)| matches!(r, Row::Item { .. }).then_some(i))
            .collect()
    }

    /// Clamp `self.scroll` so the currently-selected data row stays in
    /// view. Resolves `grid.list_selected()` (a *data* index) to the
    /// absolute row index (which counts both Headers and Items) via
    /// `item_indices`, then scrolls the window so that row sits inside
    /// `[scroll, scroll + height)`. Takes `&self` because `scroll` is a
    /// `Cell` — this is also called from `render(&self)` so the selected
    /// row stays visible even after a terminal resize or row list change.
    fn ensure_visible(&self, cfg: &Config, schema: &Schema) {
        let rows = Self::build_rows(cfg, schema);
        let items = Self::item_indices(&rows);
        if items.is_empty() {
            self.scroll.set(0);
            return;
        }
        let v = self.visible.get().max(1);
        let sel = self.grid.list_selected().min(items.len() - 1);
        let row_idx = items[sel];
        let mut s = self.scroll.get();
        // Scrolled too far → bring row_idx to the top edge.
        if row_idx < s {
            s = row_idx;
        }
        // Scrolled too short → bring row_idx to the bottom edge.
        let max_off = row_idx + 1 - v.min(row_idx + 1);
        if s < max_off {
            s = max_off;
        }
        // Don't scroll past the end of the list.
        let max_scroll = rows.len().saturating_sub(v);
        if s > max_scroll {
            s = max_scroll;
        }
        self.scroll.set(s);
    }

    // ── render ─────────────────────────────────────────────────────────
    fn split(area: Rect) -> (Rect, Rect, Rect, Rect) {
        // body → list area, then 3-line button row split into [spacer, activate, gap, launch].
        let v = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(0), Constraint::Length(3)])
            .split(area);
        let launch_label = format!("{}  (F5)", i18n::t("btn_launch"));
        let launch_w = (launch_label.width() as u16).saturating_add(6).max(20);
        let act_w = (i18n::t("btn_activate_env").width() as u16)
            .saturating_add(6)
            .max(24);
        let btn_row = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Min(0),
                Constraint::Length(act_w),
                Constraint::Length(1),
                Constraint::Length(launch_w),
            ])
            .split(v[1]);
        (v[0], btn_row[1], btn_row[3], v[1])
    }

    /// Renders the screen into `area`.
    pub fn render(
        &self,
        f: &mut Frame,
        area: Rect,
        cfg: &Config,
        schema: &Schema,
        body_active: bool,
    ) {
        let (list_area, act_rect, launch_rect, _btn_row) = Self::split(area);

        // The list title in the top border doubles as a hint that
        // pressing Enter on a selected row copies its value, saving an
        // extra hint line.
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(theme::border())
            .title(format!(" {} ", i18n::t("main_copy_hint")));
        f.render_widget(block, list_area);
        let inner = Rect {
            x: list_area.x + 1,
            y: list_area.y + 1,
            width: list_area.width.saturating_sub(2),
            height: list_area.height.saturating_sub(2),
        };
        self.visible.set(inner.height as usize);

        let rows = Self::build_rows(cfg, schema);
        let items = Self::item_indices(&rows);
        let selected_data = self.grid.list_selected().min(items.len().saturating_sub(1));
        let selected_row_idx = items.get(selected_data).copied().unwrap_or(usize::MAX);
        let list_active = body_active && self.grid.row() == 0;
        // Re-clamp scroll so the selected row stays visible after a
        // terminal resize or row-list change.
        self.ensure_visible(cfg, schema);

        let start = self.scroll.get().min(rows.len().saturating_sub(1));
        let end = (start + inner.height as usize).min(rows.len());
        let mut lines: Vec<Line> = Vec::with_capacity(end - start);
        #[allow(clippy::needless_range_loop)]
        for i in start..end {
            match &rows[i] {
                Row::Header(txt) => {
                    lines.push(Line::from(Span::styled(txt.clone(), theme::accent())));
                }
                Row::Item { label, value } => {
                    let avail = inner.width as usize;
                    let pad = avail.saturating_sub(label.width() + value.width() + 4);
                    let style = if i == selected_row_idx && list_active {
                        theme::focused()
                    } else {
                        theme::base()
                    };
                    lines.push(Line::from(vec![
                        Span::styled(format!("  {label}: "), style),
                        Span::raw(" ".repeat(pad)),
                        Span::styled(value.clone(), style),
                    ]));
                }
            }
        }
        f.render_widget(Paragraph::new(lines), inner);

        // Buttons are always visible. Focus comes from the grid;
        // the `Button` widget owns the deferred-fire pipeline.
        self.btn_activate.render(
            f,
            act_rect,
            &i18n::t("btn_activate_env"),
            self.grid.row() == 1 && self.grid.col() == 0 && body_active,
        );
        self.btn_launch.render(
            f,
            launch_rect,
            &format!("{}  (F5)", i18n::t("btn_launch")),
            self.grid.row() == 1 && self.grid.col() == 1 && body_active,
        );
    }

    /// Moves focus left within the button row.
    pub fn left(&mut self) {
        self.grid.move_left();
    }
    /// Moves focus right within the button row.
    pub fn right(&mut self) {
        self.grid.move_right();
    }
    /// Moves the cursor up through the list, or back to the list from the
    /// button row.
    pub fn up(&mut self, cfg: &Config, schema: &Schema) {
        let items_len = Self::item_indices(&Self::build_rows(cfg, schema)).len();
        self.grid.set_list_len(items_len);
        self.grid.set_visible_rows(self.visible.get());
        self.grid.move_up();
        self.ensure_visible(cfg, schema);
    }
    /// Moves the cursor down through the list, or from the list to the
    /// button row.
    pub fn down(&mut self, cfg: &Config, schema: &Schema) {
        let items_len = Self::item_indices(&Self::build_rows(cfg, schema)).len();
        self.grid.set_list_len(items_len);
        self.grid.set_visible_rows(self.visible.get());
        self.grid.move_down();
        self.ensure_visible(cfg, schema);
    }

    /// Jumps to the first row (list).
    pub fn page_up(&mut self, cfg: &Config, schema: &Schema) {
        let items_len = Self::item_indices(&Self::build_rows(cfg, schema)).len();
        self.grid.set_list_len(items_len);
        self.grid.page_up();
        self.ensure_visible(cfg, schema);
    }

    /// Jumps to the last row (button row).
    pub fn page_down(&mut self, _cfg: &Config, _schema: &Schema) {
        self.grid.page_down();
    }

    /// Handles a wheel-scroll event, cycling through list rows and the
    /// button row.
    pub fn scroll(&mut self, delta: i32, cfg: &Config, schema: &Schema) {
        let items_len = Self::item_indices(&Self::build_rows(cfg, schema)).len();
        self.grid.set_list_len(items_len);
        self.grid.set_visible_rows(self.visible.get());
        self.grid.scroll(delta);
        self.ensure_visible(cfg, schema);
    }

    /// Handles a mouse event.
    pub fn on_mouse(
        &mut self,
        m: crossterm::event::MouseEvent,
        area: Rect,
        cfg: &Config,
        schema: &Schema,
    ) -> MainAction {
        let (list_area, act_rect, launch_rect, _) = Self::split(area);
        let inside = |r: Rect| {
            m.column >= r.x && m.column < r.x + r.width && m.row >= r.y && m.row < r.y + r.height
        };

        if inside(act_rect) {
            // Arm the Button's deferred-fire pipeline; `App` polls
            // `poll_button_action` next tick to resolve the venv path
            // and emit the actual `MainAction`.
            self.grid.set_focus(1, 0);
            self.btn_activate.click();
            return MainAction::None;
        }
        if inside(launch_rect) {
            self.grid.set_focus(1, 1);
            self.btn_launch.click();
            return MainAction::None;
        }
        if !inside(list_area) {
            return MainAction::None;
        }

        // Translate click into a data-row index via the visible window.
        let rows = Self::build_rows(cfg, schema);
        let items = Self::item_indices(&rows);
        let inner_top = list_area.y + 1;
        if m.row < inner_top {
            return MainAction::None;
        }
        let rel = (m.row - inner_top) as usize;
        let abs_row = self.scroll.get() + rel;
        if abs_row >= rows.len() {
            return MainAction::None;
        }
        // Only data rows are clickable.
        let Some(data_idx) = items.iter().position(|&r| r == abs_row) else {
            return MainAction::None;
        };
        let prev_selected = self.grid.list_selected();
        self.grid.set_focus(0, 0);
        self.grid.set_list_selected(data_idx);
        if data_idx != prev_selected {
            self.ensure_visible(cfg, schema);
            return MainAction::None;
        }
        // A second click on the same row copies its value.
        self.activate(cfg, schema)
    }

    /// Activates the focused control.
    ///
    /// List rows copy their value; the buttons return the matching
    /// `MainAction`.
    pub fn activate(&mut self, cfg: &Config, schema: &Schema) -> MainAction {
        match self.focus() {
            MainFocus::List => self.copy_selected(cfg, schema),
            MainFocus::Activate => self.activate_venv(cfg),
            MainFocus::Launch => MainAction::Launch,
        }
    }

    fn copy_selected(&mut self, cfg: &Config, schema: &Schema) -> MainAction {
        let rows = Self::build_rows(cfg, schema);
        let items = Self::item_indices(&rows);
        let Some(&abs) = items.get(self.grid.list_selected()) else {
            return MainAction::None;
        };
        if let Row::Item { value, .. } = &rows[abs] {
            match clipboard::copy(value) {
                Ok(()) => MainAction::Flash(FlashKind::Info, i18n::t("popup_copied")),
                Err(e) => MainAction::Flash(
                    FlashKind::Error,
                    format!("{} {e}", i18n::t("popup_copy_failed")),
                ),
            }
        } else {
            MainAction::None
        }
    }

    fn activate_venv(&mut self, cfg: &Config) -> MainAction {
        let py = cfg.general.python.trim();
        if py.is_empty() {
            return MainAction::Flash(FlashKind::Error, i18n::t("popup_activate_no_python"));
        }
        let py_path = std::path::Path::new(py);
        let root = match python::venv_root(py_path) {
            Some(r) => r,
            None => {
                return MainAction::Flash(FlashKind::Error, i18n::t("popup_activate_not_venv"));
            }
        };
        if python::currently_activated(&root) {
            MainAction::QuitAlreadyActivated
        } else {
            MainAction::ActivateVenv(root)
        }
    }
}

/// Renders `p` as the simplest absolute path string for display.
///
/// Uses `fs::canonicalize` when the path exists; otherwise falls back to a
/// lexical normalisation. On Windows, strips the `\\?\` UNC prefix that
/// `canonicalize` adds.
fn pretty_path(p: &Path) -> String {
    let resolved = std::fs::canonicalize(p).unwrap_or_else(|_| lexical_absolute(p));
    let s = resolved.display().to_string();
    if cfg!(windows) {
        s.strip_prefix(r"\\?\").map(|t| t.to_string()).unwrap_or(s)
    } else {
        s
    }
}

/// Make `p` absolute against the current working directory (if it isn't
/// already) and collapse `.` / `..` components without touching the filesystem.
fn lexical_absolute(p: &Path) -> PathBuf {
    use std::path::Component;
    let abs = if p.is_absolute() {
        p.to_path_buf()
    } else {
        std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("."))
            .join(p)
    };
    let mut out = PathBuf::new();
    for c in abs.components() {
        match c {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// POSIX-ish shell quoting for display / copy. Adds single quotes when the
/// arg contains whitespace or shell metacharacters; escapes embedded
/// single quotes via `'\''`. Leaves plain tokens untouched so common cases
/// stay readable.
fn shell_quote(s: &str) -> String {
    let needs = s.is_empty()
        || s.chars().any(|c| {
            matches!(
                c,
                ' ' | '\t'
                    | '\n'
                    | '"'
                    | '\''
                    | '\\'
                    | '$'
                    | '`'
                    | '&'
                    | '|'
                    | ';'
                    | '<'
                    | '>'
                    | '('
                    | ')'
                    | '*'
                    | '?'
                    | '['
                    | ']'
                    | '{'
                    | '}'
                    | '#'
                    | '!'
            )
        });
    if !needs {
        return s.to_string();
    }
    let escaped = s.replace('\'', "'\\''");
    format!("'{escaped}'")
}
