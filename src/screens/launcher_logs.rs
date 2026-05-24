//! Launcher log viewer screen with an Export Logs button.

use crate::core::{clipboard, i18n, log_bus};
use crate::widgets::button::{Button, ButtonKind};
use crate::widgets::log_view::LogView;
use crate::widgets::popup::notice::{Notice, NoticeCopy, NoticeOutcome};
use crossterm::event::{KeyCode, MouseEvent};
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::Frame;

/// Launcher log viewer screen.
pub struct LauncherLogs {
    /// Underlying scrollable log viewer.
    pub log: LogView,
    /// Optional notice popup currently displayed.
    pub message: Option<Notice>,
    /// Whether the Export Logs button is keyboard-focused.
    pub export_focused: bool,
    /// Total log line count from the last frame, used for scroll math.
    last_total: u16,
    /// Whether the viewer is pinned to the tail.
    ///
    /// Cleared when the user scrolls up and re-armed by `End` or by
    /// scrolling back to the bottom.
    sticky_tail: bool,
    /// Flash message awaiting promotion to the application banner.
    pub pending_flash: Option<(crate::app::FlashKind, String)>,
    /// Persistent Export Logs button.
    pub btn_export: Button,
}

impl LauncherLogs {
    /// Constructs a fresh launcher logs screen.
    pub fn new() -> Self {
        Self {
            log: LogView::new(),
            message: None,
            export_focused: true,
            last_total: 0,
            sticky_tail: true,
            pending_flash: None,
            btn_export: Button::new(ButtonKind::Default),
        }
    }

    /// Drains and returns the pending flash message, if any.
    pub fn take_flash(&mut self) -> Option<(crate::app::FlashKind, String)> {
        self.pending_flash.take()
    }

    /// Splits `area` into the log body and the Export Logs button row.
    fn layout(area: Rect) -> (Rect, Rect) {
        let v = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(0), Constraint::Length(3)])
            .split(area);
        let w = export_button_width();
        let h = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Length(w), Constraint::Min(0)])
            .split(v[1]);
        (v[0], h[0])
    }

    /// Renders the screen into `area`.
    pub fn render(&self, f: &mut Frame, area: Rect, body_active: bool) {
        let (body, btn) = Self::layout(area);
        self.log.render(f, body);
        self.btn_export.render(
            f,
            btn,
            &i18n::t("btn_export_logs"),
            self.export_focused && body_active,
        );
        if let Some(c) = &self.message {
            c.render(f, area);
        }
    }

    /// Per-frame housekeeping called from `App::tick`.
    ///
    /// Propagates sticky-tail intent to the log widget, records the total
    /// line count for navigation math, and polls the Export button's
    /// deferred-fire pipeline.
    pub fn tick(&mut self) {
        self.last_total = log_bus::snapshot().len() as u16;
        self.log.sticky_tail = self.sticky_tail;
        if self.btn_export.poll_fire() {
            self.export();
        }
        // Drain the Notice popup's deferred-fire pipeline (OK / Copy Path).
        if let Some(n) = &mut self.message {
            match n.tick() {
                Some(NoticeOutcome::Close) => {
                    self.message = None;
                }
                Some(NoticeOutcome::Copy(s)) => {
                    self.message = None;
                    self.copy_to_clipboard(s);
                }
                None => {}
            }
        }
    }

    /// Closes the notice popup if one is open. Returns whether Esc was consumed.
    pub fn eat_esc(&mut self) -> bool {
        if self.message.is_some() {
            self.message = None;
            return true;
        }
        false
    }

    /// Handles a key event and returns whether the screen consumed it.
    pub fn on_key(&mut self, code: KeyCode) -> bool {
        if let Some(n) = &mut self.message {
            // Esc closes immediately; Enter arms the focused button so the
            // Copy outcome arrives via `tick()` next frame.
            if matches!(n.on_key(code), Some(NoticeOutcome::Close)) {
                self.message = None;
            }
            return true;
        }
        match code {
            KeyCode::Tab | KeyCode::BackTab => {
                self.export_focused = !self.export_focused;
                true
            }
            KeyCode::Up => {
                let visible = self.log.visible.get();
                let max_off = self.last_total.saturating_sub(visible);
                if self.sticky_tail {
                    self.log.scroll = max_off;
                    self.sticky_tail = false;
                }
                self.log.scroll = self.log.scroll.saturating_sub(1);
                true
            }
            KeyCode::Down => {
                let visible = self.log.visible.get();
                let max_off = self.last_total.saturating_sub(visible);
                self.log.scroll = self.log.scroll.saturating_add(1).min(max_off);
                if self.log.scroll >= max_off {
                    self.sticky_tail = true;
                }
                true
            }
            KeyCode::PageUp => {
                let visible = self.log.visible.get();
                let max_off = self.last_total.saturating_sub(visible);
                if self.sticky_tail {
                    self.log.scroll = max_off;
                    self.sticky_tail = false;
                }
                self.log.scroll = self.log.scroll.saturating_sub(visible.max(1));
                true
            }
            KeyCode::PageDown => {
                let visible = self.log.visible.get();
                let max_off = self.last_total.saturating_sub(visible);
                self.log.scroll = self.log.scroll.saturating_add(visible.max(1)).min(max_off);
                if self.log.scroll >= max_off {
                    self.sticky_tail = true;
                }
                true
            }
            KeyCode::Home => {
                self.sticky_tail = false;
                self.log.scroll = 0;
                true
            }
            KeyCode::End => {
                self.sticky_tail = true;
                true
            }
            KeyCode::Enter => {
                self.export();
                true
            }
            _ => true,
        }
    }

    fn export(&mut self) {
        // The snapshot is saved under the same per-app subfolder as the
        // session log so users have one place to look. In portable builds
        // that is `<exe_dir>/local_data/logs/`.
        let dir = crate::core::paths::logs_dir();
        let _ = std::fs::create_dir_all(&dir);
        let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S");
        let path = dir.join(format!("comfyui-tui-launcher-{stamp}.log"));
        let (body, copy) = match std::fs::write(&path, log_bus::dump_text()) {
            Ok(_) => {
                let p = path.to_string_lossy().to_string();
                let body = i18n::t_args("popup_logs_exported", &[("path", &p)]);
                let copy = Some(NoticeCopy {
                    label_key: "btn_copy_path",
                    payload: p,
                    focused: false,
                });
                (body, copy)
            }
            Err(e) => (
                i18n::t_args("popup_logs_export_failed", &[("err", &e.to_string())]),
                None,
            ),
        };
        self.message = Some(Notice::new(i18n::t("btn_export_logs"), body, copy));
    }

    fn copy_to_clipboard(&mut self, s: String) {
        self.pending_flash = Some(clipboard::copy_with_flash(&s));
    }

    /// Handles a mouse event.
    pub fn on_mouse(&mut self, m: MouseEvent, area: Rect) {
        if let Some(n) = &mut self.message {
            // Click outside fires Close immediately; OK and Copy go
            // through the Button deferred-fire pipeline.
            if matches!(n.on_mouse(m, area), Some(NoticeOutcome::Close)) {
                self.message = None;
            }
            return;
        }
        let (_body, btn) = Self::layout(area);
        if !Button::hit(btn, m) {
            // A non-button click in the body defocuses the Export button.
            if matches!(
                m.kind,
                crossterm::event::MouseEventKind::Down(crossterm::event::MouseButton::Left)
            ) {
                self.export_focused = false;
            }
            return;
        }
        // Arm the deferred-fire pipeline on the Button widget. It renders
        // highlighted on the next frame; `tick` polls `poll_fire()` and
        // runs `export()` on the frame after.
        self.export_focused = true;
        self.btn_export.click();
    }
}

fn export_button_width() -> u16 {
    use unicode_width::UnicodeWidthStr;
    i18n::t("btn_export_logs").width() as u16 + 4
}
