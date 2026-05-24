//! Generic action-menu popup.
//!
//! Owns only the visual state (title, item list, and selected index). The
//! per-row metadata that decides what each action does lives on the
//! calling screen.

use super::{center, clear_widechar_safe};
use crate::core::{i18n, theme};
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;

/// One row in a [`PopupMenu`].
#[derive(Debug, Clone)]
pub struct MenuItem {
    /// i18n key for the row label.
    pub label_key: &'static str,
    /// Disabled rows still render but Enter on them is a no-op.
    #[allow(dead_code)]
    pub enabled: bool,
}

/// Centered, bordered popup that lists a small set of actions.
///
/// The widget itself is stateless beyond `selected`; callers own the
/// per-action dispatch logic and just consult `selected` (and the
/// matching item) when the user activates the menu.
pub struct PopupMenu {
    /// Title shown in the popup's top border (already-localised text).
    pub title: String,
    /// The action rows, in display order.
    pub items: Vec<MenuItem>,
    /// Cursor position inside `items`.
    pub selected: usize,
}

impl PopupMenu {
    /// Constructs a menu, clamping `selected` to a valid row.
    pub fn new(title: String, items: Vec<MenuItem>, selected: usize) -> Self {
        let selected = selected.min(items.len().saturating_sub(1));
        Self {
            title,
            items,
            selected,
        }
    }

    /// Returns the centered popup rectangle inside `area`.
    pub fn popup_rect(&self, area: Rect) -> Rect {
        let h = (self.items.len() as u16).saturating_add(2).min(area.height);
        center(area, 50, h)
    }

    /// Renders the popup, clearing the underlying body in a wide-char-safe
    /// way so CJK glyphs underneath cannot leak into the popup interior.
    pub fn render(&self, f: &mut Frame, area: Rect) {
        let r = self.popup_rect(area);
        clear_widechar_safe(f, r);
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
        let mut lines: Vec<Line> = Vec::with_capacity(self.items.len());
        for (i, it) in self.items.iter().enumerate() {
            let label = i18n::t(it.label_key);
            let style: Style = if i == self.selected {
                theme::focused()
            } else {
                theme::base()
            };
            lines.push(Line::from(Span::styled(format!(" {label} "), style)));
        }
        f.render_widget(Paragraph::new(lines), inner);
    }
}
