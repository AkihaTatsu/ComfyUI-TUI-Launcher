//! Boolean toggle widget rendered as `[ ON ]` or `[ OFF ]`.

use crate::core::{i18n, theme};
use ratatui::style::Style;
use ratatui::text::Span;

/// Returns the styled `[ ON ]` or `[ OFF ]` span.
///
/// The colour reflects whether the value differs from the field default:
/// `modified = true` uses the accent colour, `modified = false` uses the
/// base colour. `focused` overrides both for the active row.
pub fn span(on: bool, modified: bool, focused: bool) -> Span<'static> {
    let label = if on {
        i18n::t("label_on")
    } else {
        i18n::t("label_off")
    };
    let mut style: Style = if modified {
        theme::accent()
    } else {
        theme::base()
    };
    if focused {
        style = theme::focused();
    }
    Span::styled(format!("[ {label} ]"), style)
}
