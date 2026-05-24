//! Generic table widget with lazy row materialisation.

use crate::core::theme;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;

/// Description of one table column.
pub struct Column {
    /// Header text.
    pub title: String,
    /// Fixed column width in cells.
    pub width: u16,
}

/// Table renderer.
pub struct Table<'a> {
    /// Columns in display order.
    pub columns: &'a [Column],
    /// Total number of rows.
    pub row_count: usize,
    /// Index of the selected row.
    pub selected: usize,
    /// Top-of-window row offset.
    pub scroll: usize,
}

impl<'a> Table<'a> {
    /// Renders the table using a lazy row provider.
    ///
    /// `get_row(i)` is invoked only for rows inside the visible window.
    /// `active` controls whether the selected row gets the focus highlight;
    /// pass `false` when the table is not the active focus.
    pub fn render(
        &self,
        f: &mut Frame,
        area: Rect,
        get_row: impl FnMut(usize) -> Vec<String>,
        active: bool,
    ) {
        self.render_styled(f, area, get_row, |_| None, active);
    }

    /// Like `render` but applies a per-row style override.
    pub fn render_styled(
        &self,
        f: &mut Frame,
        area: Rect,
        mut get_row: impl FnMut(usize) -> Vec<String>,
        mut get_row_style: impl FnMut(usize) -> Option<Style>,
        active: bool,
    ) {
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(theme::border());
        let inner_h = area.height.saturating_sub(2) as usize;
        f.render_widget(block, area);
        let inner = Rect {
            x: area.x + 1,
            y: area.y + 1,
            width: area.width.saturating_sub(2),
            height: area.height.saturating_sub(2),
        };
        let visible_rows = inner_h.saturating_sub(1); // exclude header row
        let start = self
            .scroll
            .min(self.row_count.saturating_sub(visible_rows.max(1)));
        let start = start.min(self.row_count);
        let end = (start + visible_rows).min(self.row_count);

        let mut lines: Vec<Line> = Vec::with_capacity(1 + visible_rows);
        let mut hdr: Vec<Span> = Vec::with_capacity(self.columns.len() * 2);
        for c in self.columns {
            hdr.push(Span::styled(pad(&c.title, c.width), theme::accent()));
            hdr.push(Span::raw(" "));
        }
        lines.push(Line::from(hdr));

        for i in start..end {
            let row = get_row(i);
            let mut spans: Vec<Span> = Vec::with_capacity(self.columns.len() * 2);
            for (ci, c) in self.columns.iter().enumerate() {
                let cell = row.get(ci).cloned().unwrap_or_default();
                spans.push(Span::raw(pad(&cell, c.width)));
                spans.push(Span::raw(" "));
            }
            let style: Style = if i == self.selected {
                if active {
                    theme::focused()
                } else {
                    get_row_style(i).unwrap_or_else(theme::accent)
                }
            } else {
                get_row_style(i).unwrap_or_else(theme::base)
            };
            lines.push(Line::from(spans).style(style));
        }
        f.render_widget(Paragraph::new(lines), inner);
    }
}

/// Fits `s` into exactly `width` cells using width-aware padding from
/// `core::text`.
fn pad(s: &str, width: u16) -> String {
    crate::core::text::pad_to_width(s, width as usize)
}
