//! Yes / No confirmation popup.

use super::center;
use crate::core::{i18n, theme};
use crate::widgets::button::{Button, ButtonKind};
use crossterm::event::{KeyCode, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};
use ratatui::Frame;

/// Yes / No popup with an OK and a Cancel button.
///
/// Mouse clicks flow through the `Button` widget's deferred-fire pipeline,
/// so the focus highlight is drawn for one frame before the outcome is
/// surfaced via `tick()`. Callers performing destructive actions should
/// pass `focus_ok = false` so the safer Cancel button starts focused.
pub struct Confirm {
    /// Popup title shown in the border.
    pub title: String,
    /// Body text shown above the buttons.
    pub body: String,
    /// When `true`, the OK button is focused; when `false`, Cancel is.
    pub focus_ok: bool,
    btn_ok: Button,
    btn_cancel: Button,
}

impl Confirm {
    /// Constructs a confirmation popup.
    pub fn new(title: String, body: String, focus_ok: bool) -> Self {
        Self {
            title,
            body,
            focus_ok,
            btn_ok: Button::new(ButtonKind::Primary),
            btn_cancel: Button::new(ButtonKind::Default),
        }
    }

    fn popup_rect(&self, area: Rect) -> Rect {
        center(area, 60, 9)
    }

    fn button_rects(&self, area: Rect) -> (Rect, Rect) {
        let r = self.popup_rect(area);
        let inner = Rect {
            x: r.x + 1,
            y: r.y + 1,
            width: r.width.saturating_sub(2),
            height: r.height.saturating_sub(2),
        };
        let v = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(0), Constraint::Length(3)])
            .split(inner);
        let bw: u16 = 12;
        let gap: u16 = 2;
        let total = bw * 2 + gap;
        let x = v[1].x + (v[1].width.saturating_sub(total)) / 2;
        let ok_r = Rect {
            x,
            y: v[1].y,
            width: bw,
            height: 3,
        };
        let cancel_r = Rect {
            x: x + bw + gap,
            y: v[1].y,
            width: bw,
            height: 3,
        };
        (ok_r, cancel_r)
    }

    /// Renders the popup centered inside `area`.
    pub fn render(&self, f: &mut Frame, area: Rect) {
        let r = self.popup_rect(area);
        super::clear_widechar_safe(f, r);
        let block = Block::default()
            .borders(Borders::ALL)
            .border_type(theme::border_type(true))
            .border_style(theme::accent())
            .title(format!(" {} ", self.title));
        f.render_widget(block, r);

        let inner = Rect {
            x: r.x + 1,
            y: r.y + 1,
            width: r.width.saturating_sub(2),
            height: r.height.saturating_sub(2),
        };
        let v = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(0), Constraint::Length(3)])
            .split(inner);
        f.render_widget(
            Paragraph::new(self.body.as_str())
                .wrap(Wrap { trim: false })
                .style(theme::base()),
            v[0],
        );
        let (ok_r, cancel_r) = self.button_rects(area);
        self.btn_ok
            .render(f, ok_r, &i18n::t("btn_ok"), self.focus_ok);
        self.btn_cancel
            .render(f, cancel_r, &i18n::t("btn_cancel"), !self.focus_ok);
    }

    /// Handles a key event.
    ///
    /// Esc returns `Some(false)` immediately; Tab, Left, and Right toggle
    /// focus; Enter arms the focused button so the outcome arrives via
    /// `tick()` one frame later.
    pub fn on_key(&mut self, code: KeyCode) -> Option<bool> {
        match code {
            KeyCode::Esc => Some(false),
            KeyCode::Tab | KeyCode::Left | KeyCode::Right => {
                self.focus_ok = !self.focus_ok;
                None
            }
            KeyCode::Enter => {
                if self.focus_ok {
                    self.btn_ok.click();
                } else {
                    self.btn_cancel.click();
                }
                None
            }
            _ => None,
        }
    }

    /// Handles a mouse event.
    ///
    /// A click outside the popup returns `Some(false)` immediately. A click
    /// on OK or Cancel arms the corresponding button so the outcome
    /// arrives via `tick()` one frame later.
    pub fn on_mouse(&mut self, m: MouseEvent, area: Rect) -> Option<bool> {
        if !matches!(m.kind, MouseEventKind::Down(MouseButton::Left)) {
            return None;
        }
        let r = self.popup_rect(area);
        let inside =
            m.column >= r.x && m.column < r.x + r.width && m.row >= r.y && m.row < r.y + r.height;
        if !inside {
            return Some(false);
        }

        let (ok_r, cancel_r) = self.button_rects(area);
        if Button::hit(ok_r, m) {
            self.focus_ok = true;
            self.btn_ok.click();
            return None;
        }
        if Button::hit(cancel_r, m) {
            self.focus_ok = false;
            self.btn_cancel.click();
            return None;
        }
        None
    }

    /// Polls once per frame. Returns `Some(true)` when OK fires,
    /// `Some(false)` when Cancel fires, and `None` otherwise.
    pub fn tick(&mut self) -> Option<bool> {
        if self.btn_ok.poll_fire() {
            return Some(true);
        }
        if self.btn_cancel.poll_fire() {
            return Some(false);
        }
        None
    }

    /// Immediate hit-test used by tests and callers that want a synchronous
    /// result without going through `tick()`.
    #[allow(dead_code)]
    pub fn hit(&self, area: Rect, col: u16, row: u16) -> Option<bool> {
        let (ok_r, cancel_r) = self.button_rects(area);
        let inside =
            |r: Rect| col >= r.x && col < r.x + r.width && row >= r.y && row < r.y + r.height;
        if inside(ok_r) {
            return Some(true);
        }
        if inside(cancel_r) {
            return Some(false);
        }
        None
    }
}
