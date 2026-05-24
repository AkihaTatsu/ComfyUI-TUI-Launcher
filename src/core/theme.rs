//! Color palette and ratatui style helpers shared by every screen.

use ratatui::style::{Color, Modifier, Style};
use ratatui::widgets::BorderType;

/// Color palette used by the launcher.
pub struct Theme {
    /// Foreground color for ordinary content.
    pub fg: Color,
    /// Foreground color for placeholder text.
    pub placeholder: Color,
    /// Accent color used for focus, headers, and primary actions.
    pub accent: Color,
    /// Foreground color used on top of `accent`.
    pub accent_fg: Color,
    /// Color used for destructive actions and warnings.
    pub danger: Color,
    /// Border color.
    pub border: Color,
}

/// Default dark palette.
pub const DARK: Theme = Theme {
    fg: Color::White,
    // The terminal's "bright black" slot keeps placeholder text dim without
    // competing with real content.
    placeholder: Color::DarkGray,
    accent: Color::Cyan,
    accent_fg: Color::White,
    danger: Color::Red,
    border: Color::White,
};

/// Returns the base text style.
pub fn base() -> Style {
    Style::default().fg(DARK.fg)
}
/// Returns the placeholder text style.
///
/// Use only for placeholder text such as `<empty>` or input hints.
pub fn placeholder() -> Style {
    Style::default().fg(DARK.placeholder)
}
/// Returns the accent style used for headings and emphasised values.
pub fn accent() -> Style {
    Style::default()
        .fg(DARK.accent)
        .add_modifier(Modifier::BOLD)
}
/// Returns the style used for focused interactive elements.
pub fn focused() -> Style {
    Style::default()
        .fg(DARK.accent_fg)
        .bg(DARK.accent)
        .add_modifier(Modifier::BOLD)
}
/// Returns the focused style for danger-colored buttons.
pub fn focused_danger() -> Style {
    Style::default()
        .fg(DARK.accent_fg)
        .bg(DARK.danger)
        .add_modifier(Modifier::BOLD)
}
/// Returns the danger style used for destructive actions.
pub fn danger() -> Style {
    Style::default()
        .fg(DARK.danger)
        .add_modifier(Modifier::BOLD)
}
/// Returns the border style.
pub fn border() -> Style {
    Style::default().fg(DARK.border)
}

/// Returns the border line style: double-line when focused, plain otherwise.
pub fn border_type(focused: bool) -> BorderType {
    if focused {
        BorderType::Double
    } else {
        BorderType::Plain
    }
}
