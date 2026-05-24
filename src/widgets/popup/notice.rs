//! Informational popup with an OK button and an optional Copy button.

use super::center;
use crate::core::{i18n, theme};
use crate::widgets::button::{Button, ButtonKind};
use crossterm::event::{KeyCode, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};
use ratatui::Frame;

/// Optional secondary Copy button rendered next to the OK button.
///
/// When present, Tab, Left, and Right toggle focus, and Enter on the Copy
/// button emits `NoticeOutcome::Copy(payload)`.
pub struct NoticeCopy {
    /// i18n key for the button label.
    pub label_key: &'static str,
    /// Text that should be copied to the clipboard.
    pub payload: String,
    /// Whether the Copy button currently holds focus.
    pub focused: bool,
}

/// Informational popup with a centered OK button and an optional Copy
/// button.
///
/// Both internal buttons use the standard `Button` deferred-fire pipeline,
/// so callers poll `tick()` once per frame to receive the outcome.
pub struct Notice {
    /// Popup title shown in the border.
    pub title: String,
    /// Body text shown above the buttons.
    pub body: String,
    /// Optional Copy button configuration.
    pub copy: Option<NoticeCopy>,
    btn_ok: Button,
    btn_copy: Button,
}

/// Outcome of a `Notice` interaction.
#[derive(Debug)]
pub enum NoticeOutcome {
    /// The user dismissed the popup.
    Close,
    /// The user pressed the Copy button; the caller should write
    /// `payload` to the clipboard and then close the popup.
    Copy(String),
}

impl Notice {
    /// Constructs a notice popup.
    pub fn new(title: String, body: String, copy: Option<NoticeCopy>) -> Self {
        Self {
            title,
            body,
            copy,
            btn_ok: Button::new(ButtonKind::Primary),
            btn_copy: Button::new(ButtonKind::Default),
        }
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

        let (ok_rect, copy_rect) = self.button_rects(v[1]);
        let ok_focused = self.copy.as_ref().map(|c| !c.focused).unwrap_or(true);
        self.btn_ok
            .render(f, ok_rect, &i18n::t("btn_ok"), ok_focused);
        if let (Some(c), Some(cr)) = (&self.copy, copy_rect) {
            self.btn_copy
                .render(f, cr, &i18n::t(c.label_key), c.focused);
        }
    }

    fn popup_rect(&self, area: Rect) -> Rect {
        center(area, 60, 9)
    }

    /// Returns `(ok_rect, copy_rect)`. The copy rect is `None` when no
    /// Copy button is configured.
    fn button_rects(&self, row: Rect) -> (Rect, Option<Rect>) {
        const W: u16 = 14;
        const GAP: u16 = 2;
        if self.copy.is_none() {
            let x = row.x + (row.width.saturating_sub(W)) / 2;
            return (
                Rect {
                    x,
                    y: row.y,
                    width: W,
                    height: 3,
                },
                None,
            );
        }
        let total = W * 2 + GAP;
        let left = row.x + (row.width.saturating_sub(total)) / 2;
        let ok = Rect {
            x: left,
            y: row.y,
            width: W,
            height: 3,
        };
        let copy = Rect {
            x: left + W + GAP,
            y: row.y,
            width: W,
            height: 3,
        };
        (ok, Some(copy))
    }

    /// Handles a key event.
    ///
    /// Esc closes immediately; Tab, Left, and Right toggle which button is
    /// focused; Enter arms the focused button's deferred-fire pipeline so
    /// the outcome arrives via `tick()` one frame later.
    pub fn on_key(&mut self, code: KeyCode) -> Option<NoticeOutcome> {
        match code {
            KeyCode::Esc => Some(NoticeOutcome::Close),
            KeyCode::Tab | KeyCode::Left | KeyCode::Right => {
                if let Some(c) = &mut self.copy {
                    c.focused = !c.focused;
                }
                None
            }
            KeyCode::Enter => {
                let copy_focused = self.copy.as_ref().map(|c| c.focused).unwrap_or(false);
                if copy_focused {
                    self.btn_copy.click();
                } else {
                    self.btn_ok.click();
                }
                None
            }
            _ => None,
        }
    }

    /// Handles a mouse event.
    ///
    /// A click outside the popup closes it immediately; a click on OK or
    /// Copy arms the corresponding button so the outcome arrives via
    /// `tick()` one frame later.
    pub fn on_mouse(&mut self, m: MouseEvent, area: Rect) -> Option<NoticeOutcome> {
        if !matches!(m.kind, MouseEventKind::Down(MouseButton::Left)) {
            return None;
        }
        let r = self.popup_rect(area);
        let inside =
            m.column >= r.x && m.column < r.x + r.width && m.row >= r.y && m.row < r.y + r.height;
        if !inside {
            return Some(NoticeOutcome::Close);
        }

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
        let (ok, copy) = self.button_rects(v[1]);
        if Button::hit(ok, m) {
            // Sync the focus indicator with the click before arming.
            if let Some(c) = &mut self.copy {
                c.focused = false;
            }
            self.btn_ok.click();
            return None;
        }
        if let (Some(c), Some(cr)) = (&mut self.copy, copy) {
            if Button::hit(cr, m) {
                c.focused = true;
                self.btn_copy.click();
                return None;
            }
        }
        None
    }

    /// Polls once per frame. Returns the deferred outcome after the
    /// highlight frame has rendered.
    pub fn tick(&mut self) -> Option<NoticeOutcome> {
        if self.btn_ok.poll_fire() {
            return Some(NoticeOutcome::Close);
        }
        if self.btn_copy.poll_fire() {
            let payload = self
                .copy
                .as_ref()
                .map(|c| c.payload.clone())
                .unwrap_or_default();
            return Some(NoticeOutcome::Copy(payload));
        }
        None
    }
}
