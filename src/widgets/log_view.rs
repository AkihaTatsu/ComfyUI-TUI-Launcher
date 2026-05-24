//! Scrollable read-only viewer over the global log bus.

use crate::core::{i18n, log_bus, theme};
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;
use std::cell::Cell;

/// Scrollable viewer over the global log bus.
pub struct LogView {
    /// Top-of-window line index. Clamped by `render`, so callers can
    /// request "scroll to tail" by passing a very large value.
    pub scroll: u16,
    /// When `true`, `render` keeps the last line visible as new lines
    /// arrive.
    pub sticky_tail: bool,
    /// Inner content height of the most recent render, used by callers to
    /// compute scroll deltas.
    pub visible: Cell<u16>,
}

impl LogView {
    /// Constructs a new viewer that follows the tail.
    pub fn new() -> Self {
        Self {
            scroll: 0,
            sticky_tail: true,
            visible: Cell::new(0),
        }
    }

    /// Renders the viewer into `area`.
    pub fn render(&self, f: &mut Frame, area: Rect) {
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(theme::border())
            .title(format!(" {} ", i18n::t("label_console")));
        let inner_h = area.height.saturating_sub(2);
        self.visible.set(inner_h);

        let snap = log_bus::snapshot();
        let total = snap.len() as u16;
        let max_off = total.saturating_sub(inner_h);
        let off = if self.sticky_tail {
            max_off
        } else {
            self.scroll.min(max_off)
        };

        // Materialise only the visible window so a 4000-line buffer
        // produces ~20 allocations per frame instead of ~4000.
        let start = off as usize;
        let end = (start + inner_h as usize).min(snap.len());
        let lines: Vec<Line> = snap[start..end]
            .iter()
            .map(|l| {
                Line::from(vec![
                    Span::styled(format!("{} ", l.ts), theme::base()),
                    Span::styled(format!("[{}] ", l.source), theme::accent()),
                    Span::raw(l.text.clone()),
                ])
            })
            .collect();
        f.render_widget(Paragraph::new(lines).block(block), area);
    }
}

impl Default for LogView {
    fn default() -> Self {
        Self::new()
    }
}
