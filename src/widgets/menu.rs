//! Vertical menu widget with primary and secondary sections.

use crate::core::{i18n, theme};
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;

/// One row in a `Menu`.
pub struct MenuEntry {
    /// i18n key for the entry's label, resolved every render so language
    /// switches apply immediately.
    pub label_key: &'static str,
}

/// Vertical menu with a primary section, a divider, and a secondary section.
pub struct Menu {
    /// Primary entries, shown above the divider.
    pub primary: Vec<MenuEntry>,
    /// Secondary entries, shown below the divider.
    pub secondary: Vec<MenuEntry>,
    /// Index of the selected entry across both sections.
    pub selected: usize,
}

impl Menu {
    /// Returns the total number of selectable entries.
    pub fn total(&self) -> usize {
        self.primary.len() + self.secondary.len()
    }

    /// Moves the selection up by one, wrapping around.
    pub fn up(&mut self) {
        let n = self.total();
        if n == 0 {
            return;
        }
        self.selected = if self.selected == 0 {
            n - 1
        } else {
            self.selected - 1
        };
    }
    /// Moves the selection down by one, wrapping around.
    pub fn down(&mut self) {
        let n = self.total();
        if n == 0 {
            return;
        }
        self.selected = (self.selected + 1) % n;
    }

    /// Total rendered rows including the divider between sections.
    fn total_rows(&self) -> usize {
        self.primary.len() + 1 + self.secondary.len()
    }

    /// Zero-based row index of the selected entry inside the rendered list.
    fn selected_row(&self) -> usize {
        let p = self.primary.len();
        if self.selected < p {
            self.selected
        } else {
            self.selected + 1 /* skip divider */
        }
    }

    /// Computes the scroll offset so the selected row is inside the
    /// `[scroll, scroll + visible)` window.
    fn scroll_offset(&self, visible: usize) -> usize {
        let total = self.total_rows();
        if visible == 0 || total <= visible {
            return 0;
        }
        let sel = self.selected_row();
        let max_scroll = total - visible;
        if sel < visible {
            0
        } else {
            (sel + 1).saturating_sub(visible).min(max_scroll)
        }
    }

    /// Returns the entry index hit by a click at row `y` inside `area`.
    pub fn hit(&self, area: Rect, y: u16) -> Option<usize> {
        let visible = (area.height.saturating_sub(2)) as usize;
        let inside = y.checked_sub(area.y + 1)? as usize;
        if inside >= visible {
            return None;
        }
        let scroll = self.scroll_offset(visible);
        let row = inside + scroll;
        let p = self.primary.len();
        if row < p {
            return Some(row);
        }
        if row == p {
            return None;
        } // divider
        let idx = row - p - 1;
        if idx < self.secondary.len() {
            Some(p + idx)
        } else {
            None
        }
    }

    /// Renders the menu into `area`.
    pub fn render(&self, f: &mut Frame, area: Rect, focused: bool) {
        let block = Block::default()
            .borders(Borders::ALL)
            .border_type(theme::border_type(focused))
            .border_style(if focused {
                theme::accent()
            } else {
                theme::border()
            });
        f.render_widget(block, area);

        let inner_w = area.width.saturating_sub(2);
        let visible = area.height.saturating_sub(2) as usize;
        let scroll = self.scroll_offset(visible);
        let total = self.total_rows();

        // Build every row up front and slice the visible window from it.
        let mut all: Vec<Line> = Vec::with_capacity(total);
        for (i, e) in self.primary.iter().enumerate() {
            all.push(self.row(i, e));
        }
        all.push(Line::from("-".repeat(inner_w as usize)).style(theme::base()));
        for (i, e) in self.secondary.iter().enumerate() {
            all.push(self.row(self.primary.len() + i, e));
        }

        let end = (scroll + visible).min(total);
        let slice: Vec<Line> = all.into_iter().skip(scroll).take(end - scroll).collect();

        let inner = Rect {
            x: area.x + 1,
            y: area.y + 1,
            width: inner_w,
            height: visible as u16,
        };
        f.render_widget(Paragraph::new(slice).style(theme::base()), inner);

        // Up and down chevrons on the left border when content is clipped.
        if scroll > 0 {
            let r = Rect {
                x: area.x,
                y: area.y,
                width: 1,
                height: 1,
            };
            f.render_widget(
                Paragraph::new(Line::from(Span::styled("↑", theme::border()))),
                r,
            );
        }
        if end < total {
            let r = Rect {
                x: area.x,
                y: area.y + area.height.saturating_sub(1),
                width: 1,
                height: 1,
            };
            f.render_widget(
                Paragraph::new(Line::from(Span::styled("↓", theme::border()))),
                r,
            );
        }
    }

    fn row(&self, idx: usize, e: &MenuEntry) -> Line<'static> {
        let marker = if idx == self.selected { ">" } else { " " };
        let style: Style = if idx == self.selected {
            theme::accent()
        } else {
            theme::base()
        };
        Line::from(format!("{marker} {}", i18n::t(e.label_key))).style(style)
    }
}
