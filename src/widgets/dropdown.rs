//! Helpers for rendering collapsed dropdown widgets.

use crate::core::theme;
use ratatui::style::Style;
use ratatui::text::Span;

/// Returns the styled current-value label of a collapsed dropdown.
///
/// `modified` is `true` when the value differs from the field default; the
/// label is then drawn in the accent colour. `focused` overrides both
/// styles for the active row.
pub fn summary(label: &str, modified: bool, focused: bool) -> Span<'static> {
    let style: Style = if focused {
        theme::focused()
    } else if modified {
        theme::accent()
    } else {
        theme::base()
    };
    Span::styled(label.to_string(), style)
}
