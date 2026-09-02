//! Horizontal tab strip with permanently visible navigation chevrons and a
//! stable, width-aware viewport over the tab labels.

use crate::core::theme;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;
use std::cell::Cell;

/// Persistent viewport state shared by rendering and mouse hit-testing.
///
/// Keeping the first visible tab outside the ephemeral [`Tabs`] value prevents
/// the label window from being recomputed around every selection change.
#[derive(Default)]
pub struct TabsState {
    first_visible: Cell<usize>,
}

/// Horizontal tab strip.
pub struct Tabs<'a> {
    /// Tab labels in display order.
    pub items: &'a [String],
    /// Index of the selected tab.
    pub selected: usize,
    /// Persistent viewport state used by every render and hit-test for this
    /// tab strip.
    pub state: &'a TabsState,
    /// When set, tabs with `true` entries render in accent style and the
    /// selected marker is suppressed. Used during cross-tab search to show
    /// which tabs contain matches.
    pub highlighted: Option<&'a [bool]>,
}

/// Result of a click hit-test on the tab strip.
#[derive(Debug, PartialEq, Eq)]
pub enum HitResult {
    /// A tab at the given index.
    Tab(usize),
    /// The left scroll chevron.
    PrevChevron,
    /// The right scroll chevron.
    NextChevron,
}

#[derive(Debug, PartialEq, Eq)]
struct VisibleTab {
    index: usize,
    start: usize,
    end: usize,
}

#[derive(Debug, PartialEq, Eq)]
struct TabLayout {
    prev_cell: Option<usize>,
    next_cell: Option<usize>,
    tabs: Vec<VisibleTab>,
}

/// Cell width of one tab label as drawn (`label` width plus two padding cells).
fn tab_cells(label: &str) -> usize {
    crate::core::text::width(label) + 2
}

/// Gap between adjacent tabs.
const GAP: usize = 2;

fn total_tab_cells(items: &[String]) -> usize {
    items.iter().enumerate().fold(0usize, |total, (i, item)| {
        total
            .saturating_add(if i == 0 { 0 } else { GAP })
            .saturating_add(tab_cells(item))
    })
}

fn place_tabs(
    items: &[String],
    first_visible: usize,
    content_start: usize,
    content_end: usize,
) -> Vec<VisibleTab> {
    let mut tabs = Vec::new();
    let mut cursor = content_start;
    for (index, item) in items.iter().enumerate().skip(first_visible) {
        let gap = if tabs.is_empty() { 0 } else { GAP };
        let start = cursor.saturating_add(gap);
        let end = start.saturating_add(tab_cells(item));
        if end > content_end {
            break;
        }
        tabs.push(VisibleTab { index, start, end });
        cursor = end;
    }
    tabs
}

impl TabsState {
    fn layout(&self, items: &[String], selected: usize, avail: usize) -> TabLayout {
        // Both chevrons are permanent whenever enough cells exist to draw
        // them. Width-one areas safely retain only the leading chevron.
        let prev_cell = (avail > 0).then_some(0);
        let next_cell = (avail > 1).then(|| avail - 1);
        let content_start = usize::from(prev_cell.is_some());
        let content_end = next_cell.unwrap_or(avail);

        if items.is_empty() {
            self.first_visible.set(0);
            return TabLayout {
                prev_cell,
                next_cell,
                tabs: Vec::new(),
            };
        }

        let selected = selected.min(items.len() - 1);
        let content_width = content_end.saturating_sub(content_start);
        let mut first = self.first_visible.get().min(items.len() - 1);

        // Once every label fits, return to the canonical unscrolled window.
        if total_tab_cells(items) <= content_width {
            first = 0;
        } else if selected < first {
            // Moving left across the visible boundary reveals the new
            // selection as the first tab without otherwise reflowing.
            first = selected;
        }

        let mut tabs = place_tabs(items, first, content_start, content_end);
        // Moving right keeps the current window until the new selection is no
        // longer visible, then advances only as far as needed to reveal it.
        while selected > first && !tabs.iter().any(|tab| tab.index == selected) {
            first += 1;
            tabs = place_tabs(items, first, content_start, content_end);
        }

        self.first_visible.set(first);
        TabLayout {
            prev_cell,
            next_cell,
            tabs,
        }
    }
}

fn pad_to(spans: &mut Vec<Span<'static>>, cursor: &mut usize, target: usize) {
    if target > *cursor {
        spans.push(Span::raw(" ".repeat(target - *cursor)));
        *cursor = target;
    }
}

impl<'a> Tabs<'a> {
    /// Renders the tab strip into `area`.
    pub fn render(&self, f: &mut Frame, area: Rect) {
        let layout = self
            .state
            .layout(self.items, self.selected, area.width as usize);
        let mut spans: Vec<Span<'static>> = Vec::new();
        let mut cursor = 0usize;

        if let Some(cell) = layout.prev_cell {
            pad_to(&mut spans, &mut cursor, cell);
            spans.push(Span::styled("‹", theme::base()));
            cursor = cell + 1;
        }

        for tab in layout.tabs {
            pad_to(&mut spans, &mut cursor, tab.start);
            let style = if let Some(highlighted) = self.highlighted {
                if highlighted.get(tab.index).copied().unwrap_or(false) {
                    theme::focused()
                } else {
                    theme::base()
                }
            } else if tab.index == self.selected {
                theme::focused()
            } else {
                theme::base()
            };
            spans.push(Span::styled(format!(" {} ", self.items[tab.index]), style));
            cursor = tab.end;
        }

        if let Some(cell) = layout.next_cell {
            pad_to(&mut spans, &mut cursor, cell);
            spans.push(Span::styled("›", theme::base()));
        }
        f.render_widget(Paragraph::new(Line::from(spans)), area);
    }

    /// Returns the hit-test result for a click at column `x` on the strip.
    pub fn hit(&self, area: Rect, x: u16) -> Option<HitResult> {
        let rel = x.checked_sub(area.x)? as usize;
        if rel >= area.width as usize {
            return None;
        }
        let layout = self
            .state
            .layout(self.items, self.selected, area.width as usize);

        if layout.prev_cell == Some(rel) {
            return Some(HitResult::PrevChevron);
        }
        if layout.next_cell == Some(rel) {
            return Some(HitResult::NextChevron);
        }
        layout
            .tabs
            .into_iter()
            .find(|tab| rel >= tab.start && rel < tab.end)
            .map(|tab| HitResult::Tab(tab.index))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    fn labels(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_string()).collect()
    }

    fn visible_indices(layout: &TabLayout) -> Vec<usize> {
        layout.tabs.iter().map(|tab| tab.index).collect()
    }

    #[test]
    fn chevrons_are_permanent_and_fixed_to_both_edges() {
        let items = labels(&["One", "Two"]);
        let state = TabsState::default();
        let layout = state.layout(&items, 0, 20);

        assert_eq!(layout.prev_cell, Some(0));
        assert_eq!(layout.next_cell, Some(19));
        assert_eq!(visible_indices(&layout), vec![0, 1]);

        let tabs = Tabs {
            items: &items,
            selected: 0,
            state: &state,
            highlighted: None,
        };
        let area = Rect::new(7, 0, 20, 1);
        assert_eq!(tabs.hit(area, 7), Some(HitResult::PrevChevron));
        assert_eq!(tabs.hit(area, 26), Some(HitResult::NextChevron));
        assert_eq!(tabs.hit(area, 6), None);
        assert_eq!(tabs.hit(area, 27), None);
    }

    #[test]
    fn viewport_moves_only_after_selection_crosses_a_visible_edge() {
        let items = labels(&["A", "B", "C", "D", "E"]);
        let state = TabsState::default();

        assert_eq!(visible_indices(&state.layout(&items, 0, 13)), vec![0, 1]);
        assert_eq!(state.first_visible.get(), 0);
        assert_eq!(visible_indices(&state.layout(&items, 1, 13)), vec![0, 1]);
        assert_eq!(state.first_visible.get(), 0);

        assert_eq!(visible_indices(&state.layout(&items, 2, 13)), vec![1, 2]);
        assert_eq!(state.first_visible.get(), 1);
        // Moving left while the selected tab is still visible does not move
        // the label window.
        assert_eq!(visible_indices(&state.layout(&items, 1, 13)), vec![1, 2]);
        assert_eq!(state.first_visible.get(), 1);
        // Crossing the left edge reveals exactly the newly selected tab.
        assert_eq!(visible_indices(&state.layout(&items, 0, 13)), vec![0, 1]);
        assert_eq!(state.first_visible.get(), 0);
    }

    #[test]
    fn wrapping_and_resizing_keep_the_selection_visible() {
        let items = labels(&["A", "B", "C", "D", "E"]);
        let state = TabsState::default();

        assert_eq!(visible_indices(&state.layout(&items, 4, 13)), vec![3, 4]);
        assert_eq!(state.first_visible.get(), 3);
        assert_eq!(visible_indices(&state.layout(&items, 0, 13)), vec![0, 1]);
        assert_eq!(state.first_visible.get(), 0);

        state.layout(&items, 4, 13);
        assert_eq!(
            visible_indices(&state.layout(&items, 4, 30)),
            vec![0, 1, 2, 3, 4]
        );
        assert_eq!(state.first_visible.get(), 0);
    }

    #[test]
    fn cjk_widths_and_gaps_share_render_and_hit_geometry() {
        let items = labels(&["内核", "扩展", "安装"]);
        let state = TabsState::default();
        let tabs = Tabs {
            items: &items,
            selected: 0,
            state: &state,
            highlighted: None,
        };
        let area = Rect::new(10, 0, 17, 1);
        let layout = state.layout(&items, 0, area.width as usize);
        assert_eq!(visible_indices(&layout), vec![0, 1]);

        for visible in &layout.tabs {
            assert_eq!(
                tabs.hit(area, area.x + visible.start as u16),
                Some(HitResult::Tab(visible.index))
            );
            assert_eq!(
                tabs.hit(area, area.x + visible.end as u16 - 1),
                Some(HitResult::Tab(visible.index))
            );
        }
        assert_eq!(tabs.hit(area, area.x + layout.tabs[0].end as u16), None);
    }

    #[test]
    fn render_places_chevrons_at_the_same_cells_as_hit_testing() {
        let items = labels(&["A", "B", "C"]);
        let state = TabsState::default();
        let backend = TestBackend::new(13, 1);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| {
                Tabs {
                    items: &items,
                    selected: 0,
                    state: &state,
                    highlighted: None,
                }
                .render(frame, frame.area());
            })
            .unwrap();

        let buffer = terminal.backend().buffer();
        assert_eq!(buffer[(0, 0)].symbol(), "‹");
        assert_eq!(buffer[(12, 0)].symbol(), "›");
    }

    #[test]
    fn tiny_widths_and_oversized_labels_are_safe() {
        let items = labels(&["an extremely long tab", "B", "C"]);
        let state = TabsState::default();

        assert_eq!(
            state.layout(&items, 0, 0),
            TabLayout {
                prev_cell: None,
                next_cell: None,
                tabs: Vec::new(),
            }
        );
        let one = state.layout(&items, 0, 1);
        assert_eq!(one.prev_cell, Some(0));
        assert_eq!(one.next_cell, None);
        assert!(one.tabs.is_empty());

        let oversized = state.layout(&items, usize::MAX, 4);
        assert_eq!(state.first_visible.get(), 2);
        assert_eq!(visible_indices(&oversized), Vec::<usize>::new());
    }
}
