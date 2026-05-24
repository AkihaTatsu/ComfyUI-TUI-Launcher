//! Single-line text input widget.

use crate::core::{i18n, theme};
use crossterm::event::KeyCode;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;

/// Editable single-line text input.
#[derive(Default)]
pub struct Input {
    /// Current text value.
    pub value: String,
    /// Cursor position as a character index into `value`.
    pub cursor: usize,
    /// Optional i18n key for the placeholder shown when `value` is empty.
    ///
    /// Resolved every render via `i18n::t` so language switches apply
    /// immediately without rebuilding the widget.
    pub placeholder_key: Option<&'static str>,
}

impl Input {
    /// Constructs an input pre-populated with `value`, with the cursor at
    /// the end.
    pub fn with(value: impl Into<String>) -> Self {
        let v = value.into();
        let c = v.chars().count();
        Self {
            value: v,
            cursor: c,
            placeholder_key: None,
        }
    }

    /// Attaches a placeholder hint by i18n key in builder style.
    pub fn placeholder(mut self, key: &'static str) -> Self {
        self.placeholder_key = Some(key);
        self
    }

    /// Returns the placeholder text resolved against the current language.
    fn placeholder_text(&self) -> String {
        self.placeholder_key.map(i18n::t).unwrap_or_default()
    }
    /// Returns `true` when the cursor is at position 0 (left edge).
    pub fn at_start(&self) -> bool {
        self.cursor == 0
    }

    /// Returns `true` when the cursor is at or past the last character (right edge).
    pub fn at_end(&self) -> bool {
        self.cursor >= self.value.chars().count()
    }

    /// Processes one key event.
    pub fn on_key(&mut self, code: KeyCode) {
        match code {
            KeyCode::Char(c) => {
                let bidx = self.byte_idx(self.cursor);
                self.value.insert(bidx, c);
                self.cursor += 1;
            }
            KeyCode::Backspace if self.cursor > 0 => {
                let start = self.byte_idx(self.cursor - 1);
                let end = self.byte_idx(self.cursor);
                self.value.replace_range(start..end, "");
                self.cursor -= 1;
            }
            KeyCode::Backspace => {}
            KeyCode::Delete => {
                let len = self.value.chars().count();
                if self.cursor < len {
                    let start = self.byte_idx(self.cursor);
                    let end = self.byte_idx(self.cursor + 1);
                    self.value.replace_range(start..end, "");
                }
            }
            KeyCode::Left if self.cursor > 0 => {
                self.cursor -= 1;
            }
            KeyCode::Left => {}
            KeyCode::Right if self.cursor < self.value.chars().count() => {
                self.cursor += 1;
            }
            KeyCode::Right => {}
            KeyCode::Home => self.cursor = 0,
            KeyCode::End => self.cursor = self.value.chars().count(),
            _ => {}
        }
    }
    fn byte_idx(&self, ch: usize) -> usize {
        self.value
            .char_indices()
            .nth(ch)
            .map(|(i, _)| i)
            .unwrap_or(self.value.len())
    }

    /// Builds the visible line with a styled cursor at `self.cursor`.
    ///
    /// When focused the cursor cell is reverse-video; when unfocused it is
    /// underlined so the field still hints at being editable. When `value`
    /// is empty and a placeholder is set, the placeholder is rendered in
    /// the placeholder colour with the cursor on its first character.
    fn line(&self, focused: bool) -> Line<'static> {
        let cursor_style: Style = if focused {
            Style::default().add_modifier(Modifier::REVERSED)
        } else {
            Style::default().add_modifier(Modifier::UNDERLINED)
        };

        let placeholder = self.placeholder_text();
        if self.value.is_empty() && !placeholder.is_empty() {
            let mut spans: Vec<Span<'static>> = Vec::new();
            let mut chars = placeholder.chars();
            if let Some(first) = chars.next() {
                // The cursor cell uses the reversed style; the rest of
                // the placeholder uses the placeholder colour.
                spans.push(Span::styled(first.to_string(), cursor_style));
                let rest: String = chars.collect();
                if !rest.is_empty() {
                    spans.push(Span::styled(rest, theme::placeholder()));
                }
            } else {
                spans.push(Span::styled(" ".to_string(), cursor_style));
            }
            return Line::from(spans);
        }

        let chars: Vec<char> = self.value.chars().collect();
        let mut spans: Vec<Span<'static>> = Vec::new();
        let before: String = chars.iter().take(self.cursor).collect();
        let at = chars.get(self.cursor).copied();
        let after: String = chars.iter().skip(self.cursor + 1).collect();
        if !before.is_empty() {
            spans.push(Span::raw(before));
        }
        match at {
            Some(c) => spans.push(Span::styled(c.to_string(), cursor_style)),
            None => spans.push(Span::styled(" ".to_string(), cursor_style)),
        }
        if !after.is_empty() {
            spans.push(Span::raw(after));
        }
        Line::from(spans)
    }

    /// Renders the input into `area`.
    pub fn render(&self, f: &mut Frame, area: Rect, focused: bool) {
        let block = Block::default()
            .borders(Borders::ALL)
            .border_type(theme::border_type(focused))
            .border_style(if focused {
                theme::accent()
            } else {
                theme::border()
            });
        let para = Paragraph::new(self.line(focused))
            .block(block)
            .style(theme::base());
        f.render_widget(para, area);
    }
}
