//! Horizontal tab strip with scroll chevrons when the labels exceed the
//! available width.

use crate::core::theme;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

/// Horizontal tab strip.
pub struct Tabs<'a> {
    /// Tab labels in display order.
    pub items: &'a [String],
    /// Index of the selected tab.
    pub selected: usize,
    /// When set, tabs with `true` entries render in accent style and the
    /// selected marker is suppressed. Used during cross-tab search to show
    /// which tabs contain matches.
    pub highlighted: Option<&'a [bool]>,
}

/// Result of a click hit-test on the tab strip.
pub enum HitResult {
    /// A tab at the given index.
    Tab(usize),
    /// The left scroll chevron.
    PrevChevron,
    /// The right scroll chevron.
    NextChevron,
}

/// Cell width of one tab label as drawn (`label` width plus two padding cells).
fn tab_cells(label: &str) -> usize {
    crate::core::text::width(label) + 2
}

/// Gap between adjacent tabs.
const GAP: usize = 2;

/// Computes the index of the first tab to draw and whether the left and
/// right scroll chevrons should be drawn for the supplied items, selected
/// index, and available width.
fn layout(items: &[String], selected: usize, avail: usize) -> (usize, bool, bool) {
    if items.is_empty() {
        return (0, false, false);
    }
    // Total un-scrolled width.
    let total: usize = items
        .iter()
        .enumerate()
        .map(|(i, t)| {
            if i == 0 {
                tab_cells(t)
            } else {
                GAP + tab_cells(t)
            }
        })
        .sum();
    if total <= avail {
        return (0, false, false);
    }

    // Scrolling required. Reserve one cell per drawn chevron and find the
    // smallest `start` index where the selected tab still fits in the
    // window.
    let n = items.len();
    let sel = selected.min(n.saturating_sub(1));

    // Try every candidate start <= sel; pick the smallest where the
    // selected tab is fully visible.
    let mut start = 0usize;
    for s in 0..=sel {
        let left_chev = s > 0;
        // Compute width consumed by tabs [s..=sel].
        let mut used = if left_chev { 1 } else { 0 };
        for (i, item) in items.iter().enumerate().take(sel + 1).skip(s) {
            if i > s {
                used += GAP;
            }
            used += tab_cells(item);
        }
        // Reserve a right chevron cell when more tabs follow `sel`.
        if sel + 1 < n {
            used += 1;
        }
        if used <= avail {
            start = s;
            break;
        }
        start = s + 1; // selected does not fit yet; try a later start
    }
    if start > sel {
        start = sel;
    }

    let left = start > 0;
    // Compute how many tabs fit starting at `start` to decide `right`.
    let mut used = if left { 1 } else { 0 };
    let mut last_drawn = start;
    for (i, item) in items.iter().enumerate().take(n).skip(start) {
        let add = (if i > start { GAP } else { 0 }) + tab_cells(item);
        // Reserve the right chevron only when more tabs follow `i`.
        let reserve_right = i + 1 < n;
        let budget = if reserve_right {
            avail.saturating_sub(1)
        } else {
            avail
        };
        if used + add > budget {
            break;
        }
        used += add;
        last_drawn = i;
    }
    let right = last_drawn + 1 < n;
    (start, left, right)
}

impl<'a> Tabs<'a> {
    /// Renders the tab strip into `area`.
    pub fn render(&self, f: &mut Frame, area: Rect) {
        let avail = area.width as usize;
        let (start, left, right) = layout(self.items, self.selected, avail);

        let mut spans: Vec<Span> = Vec::new();
        if left {
            spans.push(Span::styled("‹", theme::base()));
        }
        // Available cell budget for tabs.
        let mut used: usize = if left { 1 } else { 0 };
        let right_reserve = if right { 1 } else { 0 };
        let mut first = true;
        for i in start..self.items.len() {
            let t = &self.items[i];
            let add = (if first { 0 } else { GAP }) + tab_cells(t);
            if used + add + right_reserve > avail {
                break;
            }
            if !first {
                spans.push(Span::raw("  "));
            }
            let s = if let Some(hl) = self.highlighted {
                if hl.get(i).copied().unwrap_or(false) {
                    theme::focused()
                } else {
                    theme::base()
                }
            } else if i == self.selected {
                theme::focused()
            } else {
                theme::base()
            };
            spans.push(Span::styled(format!(" {t} "), s));
            used += add;
            first = false;
        }
        if right {
            spans.push(Span::styled("›", theme::base()));
        }
        f.render_widget(Paragraph::new(Line::from(spans)), area);
    }

    /// Returns the hit-test result for a click at column `x` on the strip.
    pub fn hit(&self, area: Rect, x: u16) -> Option<HitResult> {
        if x < area.x {
            return None;
        }
        let rel = (x - area.x) as usize;
        let avail = area.width as usize;
        let (start, left, right) = layout(self.items, self.selected, avail);

        // Left chevron occupies cell 0.
        if left && rel == 0 {
            return Some(HitResult::PrevChevron);
        }

        let mut cur: usize = if left { 1 } else { 0 };
        let right_reserve = if right { 1 } else { 0 };
        let mut first = true;
        for i in start..self.items.len() {
            let w = tab_cells(&self.items[i]);
            let gap = if first { 0 } else { GAP };
            if cur + gap + w + right_reserve > avail {
                break;
            }
            cur += gap;
            if rel >= cur && rel < cur + w {
                return Some(HitResult::Tab(i));
            }
            cur += w;
            first = false;
        }
        if right && rel == avail.saturating_sub(1) {
            return Some(HitResult::NextChevron);
        }
        None
    }
}
