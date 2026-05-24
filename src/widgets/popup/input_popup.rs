//! Single-line input popup.

use super::center;
use crate::core::theme;
use crate::widgets::input::Input;
use ratatui::layout::Rect;
use ratatui::widgets::{Block, Borders};
use ratatui::Frame;

/// Modal popup wrapping a single `Input` widget.
pub struct InputPopup {
    /// Popup title shown in the border.
    pub title: String,
    /// Inner editable input.
    pub input: Input,
}

impl InputPopup {
    /// Constructs an input popup with the supplied title and initial value.
    pub fn new(title: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            input: Input::with(value),
        }
    }
    /// Renders the popup centered inside `area`.
    pub fn render(&self, f: &mut Frame, area: Rect) {
        let r = center(area, 60, 5);
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
            width: r.width - 2,
            height: 3,
        };
        self.input.render(f, inner, true);
    }
}
