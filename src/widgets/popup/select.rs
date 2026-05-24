//! Modal selection popup that lists one choice per row.

use super::center;
use crate::core::theme;
use crossterm::event::KeyCode;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::Line;
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;

/// Single-choice selection popup.
pub struct Select {
    /// Popup title shown in the border.
    pub title: String,
    /// Selectable items in display order.
    pub items: Vec<String>,
    /// Index of the currently selected item.
    pub selected: usize,
}

impl Select {
    /// Moves the selection up one row, wrapping at the top.
    pub fn up(&mut self) {
        let n = self.items.len();
        if n == 0 {
            return;
        }
        self.selected = if self.selected == 0 {
            n - 1
        } else {
            self.selected - 1
        };
    }
    /// Moves the selection down one row, wrapping at the bottom.
    pub fn down(&mut self) {
        let n = self.items.len();
        if n == 0 {
            return;
        }
        self.selected = (self.selected + 1) % n;
    }
    /// Handles a key event. Returns `true` if the key was consumed.
    ///
    /// Up/Down navigate the list; PgUp jumps to first; PgDn jumps to last.
    /// Esc and Enter are NOT handled here — the caller decides what they do.
    pub fn on_key(&mut self, code: KeyCode) -> bool {
        match code {
            KeyCode::Up => {
                self.up();
                true
            }
            KeyCode::Down => {
                self.down();
                true
            }
            KeyCode::PageUp => {
                if !self.items.is_empty() {
                    self.selected = 0;
                }
                true
            }
            KeyCode::PageDown => {
                if !self.items.is_empty() {
                    self.selected = self.items.len() - 1;
                }
                true
            }
            _ => false,
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
        let lines: Vec<Line> = self
            .items
            .iter()
            .enumerate()
            .map(|(i, it)| {
                let s: Style = if i == self.selected {
                    theme::focused()
                } else {
                    theme::base()
                };
                Line::from(format!(" {it} ")).style(s)
            })
            .collect();
        f.render_widget(Paragraph::new(lines).block(block), r);
    }

    /// Returns the centered rectangle used by `render`, for callers that
    /// need to do their own hit-testing.
    pub fn popup_rect(&self, area: Rect) -> Rect {
        let h = (self.items.len() as u16 + 2)
            .min(area.height.saturating_sub(4))
            .max(4);
        center(area, 60, h)
    }

    /// Returns the item index under `(col, row)`, or `None` on miss.
    pub fn hit(&self, area: Rect, col: u16, row: u16) -> Option<usize> {
        let r = self.popup_rect(area);
        if col < r.x + 1 || col >= r.x + r.width.saturating_sub(1) {
            return None;
        }
        if row < r.y + 1 || row >= r.y + r.height.saturating_sub(1) {
            return None;
        }
        let idx = (row - (r.y + 1)) as usize;
        if idx < self.items.len() {
            Some(idx)
        } else {
            None
        }
    }
}
