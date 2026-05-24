//! Focusable button widget with a click-then-fire-on-next-frame pipeline.

use crate::core::theme;
use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::Line;
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;

/// Semantic flavour of a button.
///
/// Drives the text colour. Focus changes only the interior styling; the
/// border stays plain in every flavour.
#[derive(Copy, Clone, PartialEq, Eq, Default)]
pub enum ButtonKind {
    /// Plain button styled with the base foreground colour.
    #[default]
    Default,
    /// Primary action styled with the accent colour.
    Primary,
    /// Destructive action styled with the danger colour.
    #[allow(dead_code)]
    Danger,
}

/// Stateful button widget.
///
/// Each call site owns one `Button` value, renders it every frame,
/// hit-tests it on click, and polls `poll_fire` once per tick to know
/// when the action should run. The widget implements the
/// "click then highlight one frame then fire on the next frame" pipeline
/// internally, so screens never need their own deferred-click state.
///
/// Keyboard `Enter` should fire the action directly without going through
/// `click()` because the focus highlight has already been visible.
pub struct Button {
    /// Visual flavour.
    pub kind: ButtonKind,
    /// Deferred-fire state.
    ///
    /// `None` is idle; `Some(false)` means a click just landed; `Some(true)`
    /// means the highlight frame has been drawn and the next `poll_fire`
    /// call returns `true`.
    pending: Option<bool>,
}

impl Button {
    /// Constructs a new button of the given visual flavour.
    pub fn new(kind: ButtonKind) -> Self {
        Self {
            kind,
            pending: None,
        }
    }

    /// Returns whether `m` is a left-click that landed inside `area`.
    pub fn hit(area: Rect, m: MouseEvent) -> bool {
        matches!(m.kind, MouseEventKind::Down(MouseButton::Left))
            && m.column >= area.x
            && m.column < area.x + area.width
            && m.row >= area.y
            && m.row < area.y + area.height
    }

    /// Arms the deferred-fire pipeline after a mouse click.
    ///
    /// The caller is expected to also update its own focus enum so the
    /// next `render(focused = true)` highlights the button.
    pub fn click(&mut self) {
        self.pending = Some(false);
    }

    /// Polls the deferred-fire pipeline once per tick.
    ///
    /// Returns `true` exactly once, on the second tick after `click()`.
    pub fn poll_fire(&mut self) -> bool {
        match self.pending {
            None => false,
            Some(false) => {
                self.pending = Some(true);
                false
            }
            Some(true) => {
                self.pending = None;
                true
            }
        }
    }

    /// Renders the button with `label` into `area`.
    pub fn render(&self, f: &mut Frame, area: Rect, label: &str, focused: bool) {
        let text_style: Style = match (self.kind, focused) {
            (ButtonKind::Default, false) => theme::base(),
            (ButtonKind::Primary, false) => theme::accent(),
            (ButtonKind::Danger, false) => theme::danger(),
            (ButtonKind::Danger, true) => theme::focused_danger(),
            (_, true) => theme::focused(),
        };
        // The border stays plain regardless of focus; only the interior
        // label reflects focus through a reverse-video highlight.
        let block = Block::default()
            .borders(Borders::ALL)
            .border_type(theme::border_type(false))
            .border_style(theme::border());
        let para = Paragraph::new(Line::from(label.to_string()))
            .style(text_style)
            .alignment(ratatui::layout::Alignment::Center)
            .block(block);
        f.render_widget(para, area);
    }
}
