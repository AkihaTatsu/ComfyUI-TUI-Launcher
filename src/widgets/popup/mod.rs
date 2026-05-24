//! Modal popup widgets and shared geometry helpers.

/// Confirm / cancel popup.
pub mod confirm;
/// Single-line input popup.
pub mod input_popup;
/// Vertical menu popup.
pub mod menu;
/// Multi-line notice popup.
pub mod notice;
/// Single-choice selection popup.
pub mod select;

use ratatui::layout::Rect;
use ratatui::widgets::Clear;
use ratatui::Frame;

/// Returns a centered rectangle of `w` x `h` cells inside `area`.
pub fn center(area: Rect, w: u16, h: u16) -> Rect {
    let w = w.min(area.width.saturating_sub(2));
    let h = h.min(area.height.saturating_sub(2));
    let x = area.x + (area.width.saturating_sub(w)) / 2;
    let y = area.y + (area.height.saturating_sub(h)) / 2;
    Rect {
        x,
        y,
        width: w,
        height: h,
    }
}

/// Clears `area` plus one extra column on each side so a wide (CJK or
/// fullwidth) character on the underlying body cannot leak its second half
/// into the popup. The default `Clear` only zeroes the cells inside the
/// rect, which leaves a stray half-glyph when the popup boundary cuts a
/// wide cell.
pub fn clear_widechar_safe(f: &mut Frame, area: Rect) {
    let term = f.area();
    let x0 = area.x.saturating_sub(1);
    let x1 = area
        .x
        .saturating_add(area.width)
        .saturating_add(1)
        .min(term.x.saturating_add(term.width));
    let y0 = area.y.max(term.y);
    let y1 = area
        .y
        .saturating_add(area.height)
        .min(term.y.saturating_add(term.height));
    let expanded = Rect {
        x: x0,
        y: y0,
        width: x1.saturating_sub(x0),
        height: y1.saturating_sub(y0),
    };
    f.render_widget(Clear, expanded);
}
