//! First-launch wizard that collects the ComfyUI directory and Python
//! interpreter before letting the user into the main UI.

use crate::core::config::Config;
use crate::core::paths::ComfyDirs;
use crate::core::{i18n, python, theme};
use crate::widgets::button::{Button, ButtonKind};
use crate::widgets::input::Input;
use crate::widgets::popup;
use crate::widgets::table::{Column, Table};
use crossterm::event::{KeyCode, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};
use ratatui::Frame;
use std::sync::mpsc;
use std::thread;

/// First-launch wizard state.
///
/// Uses the same widgets as the main UI (`Input`, `Table`, `Button`) so
/// mouse clicks, wheel scrolling, and the standard input handling apply
/// without any wizard-specific paths.
pub struct Tutorial {
    /// Current step: 0 is welcome and git check, 1 is the ComfyUI
    /// directory, 2 is the Python interpreter, and `0xFF` is done.
    pub step: u8,
    /// Input for the ComfyUI directory step.
    pub dir_input: Input,
    /// Detected Python candidates.
    pub py_candidates: Vec<python::PythonCandidate>,
    /// Selected row inside `py_candidates`.
    pub py_selected: usize,
    /// Scroll offset for the Python candidate list.
    pub py_scroll: usize,
    /// Input for the "type a custom Python path" mode, when active.
    pub py_custom: Option<Input>,
    /// Channel for the background `python::detect` thread, when running.
    pub detect_rx: Option<mpsc::Receiver<Vec<python::PythonCandidate>>>,
    /// Validation error to surface in the body.
    pub error: Option<String>,
    /// Persistent Continue button.
    pub btn_continue: Button,
}

impl Tutorial {
    /// Constructs the wizard, choosing the first incomplete step based on
    /// the existing configuration.
    pub fn new(existing: &Config) -> Self {
        let step = if existing.general.comfyui_dir.is_empty()
            || !std::path::Path::new(&existing.general.comfyui_dir).is_dir()
        {
            1
        } else if existing.general.python.is_empty()
            || python::validate(std::path::Path::new(&existing.general.python)).is_none()
        {
            2
        } else {
            0
        };
        let mut t = Self {
            step,
            dir_input: Input::with(existing.general.comfyui_dir.clone()),
            py_candidates: Vec::new(),
            py_selected: 0,
            py_scroll: 0,
            py_custom: None,
            detect_rx: None,
            error: None,
            btn_continue: Button::new(ButtonKind::Primary),
        };
        if step == 2 {
            t.spawn_detect(&existing.general.comfyui_dir);
        }
        t
    }

    /// Returns whether the wizard has finished.
    pub fn is_done(&self) -> bool {
        self.step == 0xFF
    }

    /// Per-frame housekeeping called by `App::tick`.
    ///
    /// Polls the background Python detection thread and drains the
    /// Continue button's deferred-fire pipeline.
    pub fn tick(&mut self, cfg: &mut Config) {
        let done = if let Some(rx) = &self.detect_rx {
            rx.try_recv().ok()
        } else {
            None
        };
        if let Some(cands) = done {
            self.py_candidates = cands;
            self.py_selected = 0;
            self.py_scroll = 0;
            self.detect_rx = None;
        }
        // Deferred Continue dispatch — step decides what to actually do.
        if self.btn_continue.poll_fire() {
            match self.step {
                1 => self.confirm_dir(cfg),
                2 if self.py_custom.is_some() => self.on_key_python(KeyCode::Enter, cfg),
                2 => self.activate_python_row(cfg),
                _ => {}
            }
        }
    }

    fn spawn_detect(&mut self, dir: &str) {
        let (tx, rx) = mpsc::channel();
        let dir = dir.to_string();
        self.detect_rx = Some(rx);
        thread::spawn(move || {
            let cands = python::detect(Some(std::path::Path::new(&dir)));
            let _ = tx.send(cands);
        });
    }

    // ── rendering ────────────────────────────────────────────────────────
    fn body_rects(area: Rect) -> (Rect, Rect) {
        let v = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(3), Constraint::Min(0)])
            .split(area);
        (v[0], v[1])
    }

    /// Renders the wizard into `area`.
    pub fn render(&self, f: &mut Frame, area: Rect) {
        let (title_r, body_r) = Self::body_rects(area);
        f.render_widget(
            Paragraph::new(i18n::t("tutorial_welcome"))
                .style(theme::accent())
                .block(
                    Block::default()
                        .borders(Borders::ALL)
                        .border_style(theme::border()),
                ),
            title_r,
        );

        match self.step {
            0 => self.render_welcome(f, body_r),
            1 => self.render_dir(f, body_r),
            2 => {
                if self.py_custom.is_some() {
                    self.render_py_custom(f, body_r);
                } else {
                    self.render_py_list(f, body_r);
                }
            }
            _ => {}
        }

        // Detection popup overlays the body whenever a probe is in flight.
        if self.detect_rx.is_some() {
            self.render_detecting(f, area);
        }
    }

    fn render_welcome(&self, f: &mut Frame, area: Rect) {
        let outer = Block::default()
            .borders(Borders::ALL)
            .border_style(theme::border());
        f.render_widget(outer, area);
        let inner = Rect {
            x: area.x + 1,
            y: area.y + 1,
            width: area.width.saturating_sub(2),
            height: area.height.saturating_sub(2),
        };
        let lines = vec![
            Line::from(i18n::t("tutorial_welcome")),
            Line::from(""),
            Line::from(self.error.clone().unwrap_or_default()).style(theme::danger()),
        ];
        f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
    }

    fn render_dir(&self, f: &mut Frame, area: Rect) {
        let outer = Block::default()
            .borders(Borders::ALL)
            .border_style(theme::border());
        f.render_widget(outer, area);
        let inner = Rect {
            x: area.x + 1,
            y: area.y + 1,
            width: area.width.saturating_sub(2),
            height: area.height.saturating_sub(2),
        };
        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1),
                Constraint::Length(1),
                Constraint::Length(3),
                Constraint::Length(1),
                Constraint::Min(0),
                Constraint::Length(3),
            ])
            .split(inner);
        f.render_widget(Paragraph::new(i18n::t("tutorial_step_dir")), rows[0]);
        self.dir_input.render(f, rows[2], true);
        f.render_widget(
            Paragraph::new(self.error.clone().unwrap_or_default()).style(theme::danger()),
            rows[4],
        );
        self.btn_continue.render(
            f,
            Self::footer_button_rect(rows[5]),
            &i18n::t("tutorial_continue"),
            true,
        );
    }

    fn render_py_list(&self, f: &mut Frame, area: Rect) {
        let outer = Block::default()
            .borders(Borders::ALL)
            .border_style(theme::border());
        f.render_widget(outer, area);
        let inner = Rect {
            x: area.x + 1,
            y: area.y + 1,
            width: area.width.saturating_sub(2),
            height: area.height.saturating_sub(2),
        };
        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1),
                Constraint::Length(1),
                Constraint::Min(0),
                Constraint::Length(1),
                Constraint::Length(1),
                Constraint::Length(3),
            ])
            .split(inner);
        f.render_widget(Paragraph::new(i18n::t("tutorial_step_python")), rows[0]);

        // Table = scroll wheel + click-row out of the box.
        let n = self.py_candidates.len() + 1; // +1 for "(custom path…)"
        let cols = vec![Column {
            title: i18n::t("setting_python"),
            width: rows[2].width.saturating_sub(2),
        }];
        let custom_label = i18n::t("tutorial_custom_path");
        let candidates = &self.py_candidates;
        Table {
            columns: &cols,
            row_count: n,
            selected: self.py_selected,
            scroll: self.py_scroll,
        }
        .render(
            f,
            rows[2],
            |i| {
                if i < candidates.len() {
                    vec![candidates[i].label.clone()]
                } else {
                    vec![custom_label.clone()]
                }
            },
            true,
        );

        f.render_widget(
            Paragraph::new(self.error.clone().unwrap_or_default()).style(theme::danger()),
            rows[4],
        );
        self.btn_continue.render(
            f,
            Self::footer_button_rect(rows[5]),
            &i18n::t("tutorial_continue"),
            true,
        );
    }

    fn render_py_custom(&self, f: &mut Frame, area: Rect) {
        let outer = Block::default()
            .borders(Borders::ALL)
            .border_style(theme::border());
        f.render_widget(outer, area);
        let inner = Rect {
            x: area.x + 1,
            y: area.y + 1,
            width: area.width.saturating_sub(2),
            height: area.height.saturating_sub(2),
        };
        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1),
                Constraint::Length(1),
                Constraint::Length(3),
                Constraint::Length(1),
                Constraint::Min(0),
                Constraint::Length(3),
            ])
            .split(inner);
        f.render_widget(Paragraph::new(i18n::t("tutorial_type_py_path")), rows[0]);
        if let Some(inp) = &self.py_custom {
            inp.render(f, rows[2], true);
        }
        f.render_widget(
            Paragraph::new(self.error.clone().unwrap_or_default()).style(theme::danger()),
            rows[4],
        );
        self.btn_continue.render(
            f,
            Self::footer_button_rect(rows[5]),
            &i18n::t("tutorial_continue"),
            true,
        );
    }

    fn render_detecting(&self, f: &mut Frame, area: Rect) {
        let r = popup::center(area, 60, 5);
        popup::clear_widechar_safe(f, r);
        let block = Block::default()
            .borders(Borders::ALL)
            .border_type(theme::border_type(true))
            .border_style(theme::accent())
            .title(format!(" {} ", i18n::t("popup_working")));
        f.render_widget(block, r);
        let inner = Rect {
            x: r.x + 1,
            y: r.y + 1,
            width: r.width.saturating_sub(2),
            height: r.height.saturating_sub(2),
        };
        let lines = vec![Line::from(Span::styled(
            i18n::t("tutorial_detecting_python"),
            theme::base(),
        ))];
        f.render_widget(Paragraph::new(lines), inner);
    }

    /// Centred button rect inside a 3-row footer area.
    fn footer_button_rect(row: Rect) -> Rect {
        use unicode_width::UnicodeWidthStr;
        let label = i18n::t("tutorial_continue");
        let w = (label.width() as u16 + 4).max(12);
        let x = row.x + (row.width.saturating_sub(w)) / 2;
        Rect {
            x,
            y: row.y,
            width: w,
            height: 3,
        }
    }

    // ── keyboard ────────────────────────────────────────────────────────
    /// Handles a key event.
    pub fn on_key(&mut self, code: KeyCode, cfg: &mut Config) {
        self.error = None;
        if self.detect_rx.is_some() {
            return;
        } // input frozen during detection.
        match self.step {
            0 => self.on_key_welcome(code),
            1 => self.on_key_dir(code, cfg),
            2 => self.on_key_python(code, cfg),
            _ => {}
        }
    }

    fn on_key_welcome(&mut self, code: KeyCode) {
        if !python::git_available() {
            self.error = Some(i18n::t("tutorial_need_git"));
            return;
        }
        if matches!(code, KeyCode::Enter) {
            self.step = 1;
        }
    }

    fn on_key_dir(&mut self, code: KeyCode, cfg: &mut Config) {
        match code {
            KeyCode::Enter => self.confirm_dir(cfg),
            k => self.dir_input.on_key(k),
        }
    }

    fn confirm_dir(&mut self, cfg: &mut Config) {
        let dir = self.dir_input.value.trim().to_string();
        if !ComfyDirs::new(&dir).is_valid() {
            self.error = Some(i18n::t("tutorial_invalid_dir"));
            return;
        }
        cfg.general.comfyui_dir = dir.clone();
        let _ = cfg.save();
        self.py_custom = None;
        self.spawn_detect(&dir);
        self.step = 2;
    }

    fn on_key_python(&mut self, code: KeyCode, cfg: &mut Config) {
        if let Some(inp) = &mut self.py_custom {
            match code {
                KeyCode::Esc => self.py_custom = None,
                KeyCode::Enter => {
                    let raw = inp.value.trim().to_string();
                    let path = python::resolve(std::path::Path::new(&raw));
                    if python::validate(&path).is_none() {
                        self.error = Some(i18n::t("tutorial_invalid_py"));
                        return;
                    }
                    cfg.general.python = path.display().to_string();
                    let _ = cfg.save();
                    self.step = 0xFF;
                }
                k => inp.on_key(k),
            }
            return;
        }
        let n = self.py_candidates.len() + 1;
        match code {
            KeyCode::Up => {
                if n == 0 {
                    return;
                }
                self.py_selected = if self.py_selected == 0 {
                    n - 1
                } else {
                    self.py_selected - 1
                };
            }
            KeyCode::Down => {
                if n == 0 {
                    return;
                }
                self.py_selected = (self.py_selected + 1) % n;
            }
            KeyCode::Enter => self.activate_python_row(cfg),
            _ => {}
        }
    }

    fn activate_python_row(&mut self, cfg: &mut Config) {
        let idx = self.py_selected;
        if idx < self.py_candidates.len() {
            let path = self.py_candidates[idx].path.clone();
            if python::validate(&path).is_none() {
                self.error = Some(i18n::t("tutorial_invalid_py"));
                return;
            }
            cfg.general.python = path.display().to_string();
            let _ = cfg.save();
            self.step = 0xFF;
        } else {
            // "(custom path…)" row.
            self.py_custom = Some(Input::with(cfg.general.python.clone()));
        }
    }

    // ── mouse ──────────────────────────────────────────────────────────
    /// Handles a mouse event.
    pub fn on_mouse(&mut self, m: MouseEvent, area: Rect, cfg: &mut Config) {
        if self.detect_rx.is_some() {
            return;
        }
        // Wheel scroll for the candidate list (step 2, list mode).
        if self.step == 2
            && self.py_custom.is_none()
            && matches!(
                m.kind,
                MouseEventKind::ScrollUp | MouseEventKind::ScrollDown
            )
        {
            let n = self.py_candidates.len() + 1;
            if n == 0 {
                return;
            }
            if matches!(m.kind, MouseEventKind::ScrollUp) {
                if self.py_selected == 0 {
                    return;
                }
                self.py_selected -= 1;
            } else {
                if self.py_selected + 1 >= n {
                    return;
                }
                self.py_selected += 1;
            }
            self.adjust_py_scroll();
            return;
        }
        if !matches!(m.kind, MouseEventKind::Down(MouseButton::Left)) {
            return;
        }
        let (_title_r, body_r) = Self::body_rects(area);
        match self.step {
            0 => {
                // Click anywhere in body acts like Enter on the welcome screen.
                self.on_key_welcome(KeyCode::Enter);
            }
            1 => self.on_mouse_dir(m, body_r, cfg),
            2 => {
                if self.py_custom.is_some() {
                    self.on_mouse_py_custom(m, body_r, cfg);
                } else {
                    self.on_mouse_py_list(m, body_r, cfg);
                }
            }
            _ => {}
        }
    }

    fn on_mouse_dir(&mut self, m: MouseEvent, body: Rect, _cfg: &mut Config) {
        let inner = Rect {
            x: body.x + 1,
            y: body.y + 1,
            width: body.width.saturating_sub(2),
            height: body.height.saturating_sub(2),
        };
        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1),
                Constraint::Length(1),
                Constraint::Length(3),
                Constraint::Length(1),
                Constraint::Min(0),
                Constraint::Length(3),
            ])
            .split(inner);
        let btn = Self::footer_button_rect(rows[5]);
        if Self::inside(m, btn) {
            self.btn_continue.click();
        }
    }

    fn on_mouse_py_list(&mut self, m: MouseEvent, body: Rect, cfg: &mut Config) {
        let inner = Rect {
            x: body.x + 1,
            y: body.y + 1,
            width: body.width.saturating_sub(2),
            height: body.height.saturating_sub(2),
        };
        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1),
                Constraint::Length(1),
                Constraint::Min(0),
                Constraint::Length(1),
                Constraint::Length(1),
                Constraint::Length(3),
            ])
            .split(inner);
        let btn = Self::footer_button_rect(rows[5]);
        if Self::inside(m, btn) {
            // Defer dispatch by one frame via the Button's pipeline.
            let _ = cfg;
            self.btn_continue.click();
            return;
        }

        // Table rows are inside rows[2] with a border (top 1 row) + header (1 row).
        let table = rows[2];
        let row_top = table.y + 2;
        if m.row < row_top || m.row >= table.y + table.height {
            return;
        }
        let rel = (m.row - row_top) as usize;
        let idx = self.py_scroll + rel;
        let n = self.py_candidates.len() + 1;
        if idx >= n {
            return;
        }
        if idx != self.py_selected {
            self.py_selected = idx;
            self.adjust_py_scroll();
            return;
        }
        self.activate_python_row(cfg);
    }

    fn on_mouse_py_custom(&mut self, m: MouseEvent, body: Rect, _cfg: &mut Config) {
        let inner = Rect {
            x: body.x + 1,
            y: body.y + 1,
            width: body.width.saturating_sub(2),
            height: body.height.saturating_sub(2),
        };
        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1),
                Constraint::Length(1),
                Constraint::Length(3),
                Constraint::Length(1),
                Constraint::Min(0),
                Constraint::Length(3),
            ])
            .split(inner);
        let btn = Self::footer_button_rect(rows[5]);
        if Self::inside(m, btn) {
            self.btn_continue.click();
        }
    }

    fn inside(m: MouseEvent, r: Rect) -> bool {
        m.column >= r.x && m.column < r.x + r.width && m.row >= r.y && m.row < r.y + r.height
    }

    fn adjust_py_scroll(&mut self) {
        // Best-effort: keep selected in view. Real visible-row count isn't
        // known here, so a simple "centered" heuristic is fine.
        if self.py_selected < self.py_scroll {
            self.py_scroll = self.py_selected;
        }
        let visible: usize = 10; // a sane minimum; Table widget will clamp further.
        let max_off = self.py_selected.saturating_sub(visible.saturating_sub(1));
        if self.py_scroll < max_off {
            self.py_scroll = max_off;
        }
    }
}
