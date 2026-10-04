//! Data-source-independent, width-aware log display.

use crate::core::{log_bus, text, theme};
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph};
use ratatui::Frame;
use std::cell::{Cell, RefCell};
use std::collections::HashMap;

/// Portion of a log source requested by a display.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LogRange {
    /// Every retained entry.
    All,
    /// At most the newest `count` entries.
    Tail(usize),
}

/// Read-only input consumed by [`LogDisplay`].
///
/// `visit` returns the revision that was current for the visited sequence,
/// allowing sources with internal locking to provide a consistent snapshot.
pub trait LogSource {
    /// Stable identity of this source instance.
    fn identity(&self) -> u64;
    /// Cheap revision hint used to skip unchanged visits.
    fn revision(&self) -> u64;
    /// Visits the requested entries in display order.
    fn visit(&self, range: LogRange, visitor: &mut dyn FnMut(&log_bus::LogLine)) -> u64;
}

/// Adapter exposing the process-wide log bus through [`LogSource`].
#[derive(Debug, Default, Clone, Copy)]
pub struct LogBusSource;

impl LogSource for LogBusSource {
    fn identity(&self) -> u64 {
        1
    }

    fn revision(&self) -> u64 {
        log_bus::revision()
    }

    fn visit(&self, range: LogRange, visitor: &mut dyn FnMut(&log_bus::LogLine)) -> u64 {
        log_bus::with_lines(|lines, revision| {
            let skip = match range {
                LogRange::All => 0,
                LogRange::Tail(count) => lines.len().saturating_sub(count),
            };
            for line in lines.iter().skip(skip) {
                visitor(line);
            }
            revision
        })
    }
}

/// How a display chooses its visible window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogViewportMode {
    /// Honour the display's navigation state.
    Scrollable,
    /// Always show the newest visual rows.
    Tail,
}

/// Prefix fields and continuation layout for one log entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LogPrefix {
    pub timestamp: bool,
    pub source: bool,
    pub continuation_indent: bool,
}

impl Default for LogPrefix {
    fn default() -> Self {
        Self {
            timestamp: true,
            source: true,
            continuation_indent: true,
        }
    }
}

/// Styles applied by the shared formatter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LogStyles {
    pub timestamp: Style,
    pub source: Style,
    pub body: Style,
}

impl Default for LogStyles {
    fn default() -> Self {
        Self {
            timestamp: theme::base(),
            source: theme::accent(),
            body: theme::base(),
        }
    }
}

/// Per-render behaviour and chrome for [`LogDisplay`].
pub struct LogDisplayOptions<'a> {
    pub range: LogRange,
    pub viewport: LogViewportMode,
    pub block: Option<Block<'a>>,
    pub prefix: LogPrefix,
    pub leading_lines: &'a [Line<'a>],
    pub trailing_lines: &'a [Line<'a>],
    /// Auxiliary rows are shown only when at least this many log rows remain.
    pub min_log_rows: u16,
    pub empty_line: Option<Line<'a>>,
    pub styles: LogStyles,
}

impl Default for LogDisplayOptions<'_> {
    fn default() -> Self {
        Self {
            range: LogRange::All,
            viewport: LogViewportMode::Scrollable,
            block: None,
            prefix: LogPrefix::default(),
            leading_lines: &[],
            trailing_lines: &[],
            min_log_rows: 1,
            empty_line: None,
            styles: LogStyles::default(),
        }
    }
}

/// Navigation commands shared by keyboard and mouse callers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogNavigation {
    Lines(i32),
    Pages(i32),
    Home,
    End,
}

#[derive(Debug)]
struct CachedEntry {
    raw: log_bus::LogLine,
    rows: Vec<Line<'static>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Anchor {
    id: u64,
    subrow: usize,
}

#[derive(Debug)]
struct ReflowCache {
    source_identity: Option<u64>,
    revision: u64,
    range: LogRange,
    width: usize,
    prefix: LogPrefix,
    styles: LogStyles,
    entries: Vec<CachedEntry>,
    total_rows: usize,
}

impl Default for ReflowCache {
    fn default() -> Self {
        Self {
            source_identity: None,
            revision: u64::MAX,
            range: LogRange::All,
            width: 0,
            prefix: LogPrefix::default(),
            styles: LogStyles::default(),
            entries: Vec::new(),
            total_rows: 0,
        }
    }
}

impl ReflowCache {
    /// Synchronises ordering/content first, then performs expensive wrapping
    /// after the source has released any internal lock.
    fn sync(
        &mut self,
        source: &dyn LogSource,
        range: LogRange,
        width: usize,
        prefix: LogPrefix,
        styles: LogStyles,
    ) -> bool {
        let identity = source.identity();
        let source_changed = self.source_identity != Some(identity);
        let range_changed = self.range != range;
        let hinted_revision = source.revision();
        let data_changed = source_changed || range_changed || self.revision != hinted_revision;
        let layout_changed = self.width != width || self.prefix != prefix || self.styles != styles;

        if !data_changed && !layout_changed {
            return false;
        }

        if data_changed {
            let mut old = if source_changed {
                HashMap::new()
            } else {
                std::mem::take(&mut self.entries)
                    .into_iter()
                    .map(|entry| (entry.raw.id(), entry))
                    .collect::<HashMap<_, _>>()
            };
            let mut next = Vec::new();
            let revision = source.visit(range, &mut |line| {
                let mut entry = old.remove(&line.id()).unwrap_or_else(|| CachedEntry {
                    raw: line.clone(),
                    rows: Vec::new(),
                });
                if entry.raw.version() != line.version() {
                    entry.raw = line.clone();
                    entry.rows.clear();
                }
                next.push(entry);
            });
            self.entries = next;
            self.source_identity = Some(identity);
            self.revision = revision;
            self.range = range;
        }

        if layout_changed {
            for entry in &mut self.entries {
                entry.rows.clear();
            }
            self.width = width;
            self.prefix = prefix;
            self.styles = styles;
        }

        for entry in &mut self.entries {
            if entry.rows.is_empty() && width > 0 {
                entry.rows = wrap_log_line(&entry.raw, width, prefix, styles);
            }
        }
        self.total_rows = self.entries.iter().map(|entry| entry.rows.len()).sum();
        true
    }

    fn anchor_at(&self, offset: usize) -> Option<Anchor> {
        let mut remaining = offset;
        for entry in &self.entries {
            if remaining < entry.rows.len() {
                return Some(Anchor {
                    id: entry.raw.id(),
                    subrow: remaining,
                });
            }
            remaining = remaining.saturating_sub(entry.rows.len());
        }
        None
    }

    fn resolve(&self, anchor: Anchor) -> Option<usize> {
        let mut offset = 0usize;
        for entry in &self.entries {
            if entry.raw.id() == anchor.id {
                return Some(offset + anchor.subrow.min(entry.rows.len().saturating_sub(1)));
            }
            offset = offset.saturating_add(entry.rows.len());
        }
        None
    }

    fn visible_rows(&self, offset: usize, height: usize) -> Vec<Line<'static>> {
        if height == 0 {
            return Vec::new();
        }
        let mut skip = offset;
        let mut out = Vec::with_capacity(height);
        for entry in &self.entries {
            if skip >= entry.rows.len() {
                skip -= entry.rows.len();
                continue;
            }
            for row in entry.rows.iter().skip(skip) {
                out.push(row.clone());
                if out.len() == height {
                    return out;
                }
            }
            skip = 0;
        }
        out
    }
}

/// Reusable log renderer with cached reflow and visual-row navigation.
pub struct LogDisplay {
    cache: RefCell<ReflowCache>,
    scroll: Cell<usize>,
    follow_tail: Cell<bool>,
    visible_rows: Cell<usize>,
    total_rows: Cell<usize>,
}

impl LogDisplay {
    pub fn new() -> Self {
        Self {
            cache: RefCell::new(ReflowCache::default()),
            scroll: Cell::new(0),
            follow_tail: Cell::new(true),
            visible_rows: Cell::new(0),
            total_rows: Cell::new(0),
        }
    }

    /// Moves the viewport to the newest visual rows and resumes following
    /// future source revisions.
    pub fn reset_to_tail(&self) {
        self.follow_tail.set(true);
        self.scroll.set(
            self.total_rows
                .get()
                .saturating_sub(self.visible_rows.get()),
        );
    }

    #[cfg(test)]
    pub(crate) fn is_following_tail(&self) -> bool {
        self.follow_tail.get()
    }

    /// Renders logs and optional chrome using the current source revision.
    pub fn render(
        &self,
        f: &mut Frame,
        area: Rect,
        source: &dyn LogSource,
        mut options: LogDisplayOptions<'_>,
    ) {
        let inner = options
            .block
            .as_ref()
            .map(|block| block.inner(area))
            .unwrap_or(area);
        if let Some(block) = options.block.take() {
            f.render_widget(block, area);
        }

        let (leading_len, trailing_len) = auxiliary_lengths(
            inner.height as usize,
            options.leading_lines.len(),
            options.trailing_lines.len(),
            options.min_log_rows as usize,
        );
        if leading_len > 0 {
            let leading_area = Rect::new(inner.x, inner.y, inner.width, leading_len as u16);
            f.render_widget(
                Paragraph::new(options.leading_lines[..leading_len].to_vec()),
                leading_area,
            );
        }
        if trailing_len > 0 {
            let trailing_area = Rect::new(
                inner.x,
                inner.bottom().saturating_sub(trailing_len as u16),
                inner.width,
                trailing_len as u16,
            );
            f.render_widget(
                Paragraph::new(options.trailing_lines[..trailing_len].to_vec()),
                trailing_area,
            );
        }
        let log_area = Rect::new(
            inner.x,
            inner.y.saturating_add(leading_len as u16),
            inner.width,
            inner
                .height
                .saturating_sub((leading_len + trailing_len) as u16),
        );

        let old_scroll = self.scroll.get();
        let scrollable = options.viewport == LogViewportMode::Scrollable;
        let anchored = scrollable && !self.follow_tail.get();
        let mut cache = self.cache.borrow_mut();
        let source_changed = cache.source_identity != Some(source.identity());
        let anchor = anchored.then(|| cache.anchor_at(old_scroll)).flatten();
        let changed = cache.sync(
            source,
            options.range,
            log_area.width as usize,
            options.prefix,
            options.styles,
        );

        let total = cache.total_rows;
        let visible = log_area.height as usize;
        let max_offset = total.saturating_sub(visible);
        let mut offset = old_scroll;
        if source_changed && anchored {
            offset = 0;
        } else if changed && anchored {
            if let Some(anchor) = anchor {
                offset = cache.resolve(anchor).unwrap_or(offset);
            }
        }
        if options.viewport == LogViewportMode::Tail || self.follow_tail.get() {
            offset = max_offset;
        } else {
            offset = offset.min(max_offset);
        }

        self.scroll.set(offset);
        self.visible_rows.set(visible);
        self.total_rows.set(total);

        let rows = cache.visible_rows(offset, visible);
        drop(cache);
        if rows.is_empty() {
            if let Some(empty) = options.empty_line {
                f.render_widget(Paragraph::new(empty), log_area);
            }
        } else {
            f.render_widget(Paragraph::new(rows), log_area);
        }
    }

    /// Applies visual-row navigation. Callers map keys and mouse gestures to
    /// this small command set instead of duplicating scroll arithmetic.
    pub fn navigate(&self, navigation: LogNavigation) {
        let visible = self.visible_rows.get().max(1);
        let max_offset = self
            .total_rows
            .get()
            .saturating_sub(self.visible_rows.get());
        let mut offset = self.scroll.get().min(max_offset);
        match navigation {
            LogNavigation::Lines(delta) => {
                if delta < 0 {
                    if self.follow_tail.replace(false) {
                        offset = max_offset;
                    }
                    offset = offset.saturating_sub(delta.unsigned_abs() as usize);
                } else {
                    offset = offset.saturating_add(delta as usize).min(max_offset);
                    if offset >= max_offset {
                        self.follow_tail.set(true);
                    }
                }
            }
            LogNavigation::Pages(delta) => {
                let amount = visible.saturating_mul(delta.unsigned_abs() as usize);
                if delta < 0 {
                    if self.follow_tail.replace(false) {
                        offset = max_offset;
                    }
                    offset = offset.saturating_sub(amount);
                } else {
                    offset = offset.saturating_add(amount).min(max_offset);
                    if offset >= max_offset {
                        self.follow_tail.set(true);
                    }
                }
            }
            LogNavigation::Home => {
                self.follow_tail.set(false);
                offset = 0;
            }
            LogNavigation::End => {
                self.reset_to_tail();
                return;
            }
        }
        self.scroll.set(offset);
    }
}

impl Default for LogDisplay {
    fn default() -> Self {
        Self::new()
    }
}

fn auxiliary_lengths(
    height: usize,
    leading: usize,
    trailing: usize,
    min_log_rows: usize,
) -> (usize, usize) {
    if height
        >= leading
            .saturating_add(trailing)
            .saturating_add(min_log_rows)
    {
        (leading, trailing)
    } else {
        (0, 0)
    }
}

fn wrap_log_line(
    line: &log_bus::LogLine,
    width: usize,
    prefix_options: LogPrefix,
    styles: LogStyles,
) -> Vec<Line<'static>> {
    if width == 0 {
        return Vec::new();
    }

    let source = text::sanitize_for_tui_log(&line.source);
    let body = text::sanitize_for_tui_log(&line.text);
    let timestamp = if prefix_options.timestamp {
        format!("{} ", line.ts)
    } else {
        String::new()
    };
    let source_prefix = if prefix_options.source {
        format!("[{source}] ")
    } else {
        String::new()
    };
    let prefix = format!("{timestamp}{source_prefix}");
    let prefix_width = text::width(&prefix);

    if prefix_width >= width {
        return text::wrap_to_width(&format!("{prefix}{body}"), width)
            .into_iter()
            .map(|part| Line::from(Span::styled(part, styles.body)))
            .collect();
    }

    let body_width = width - prefix_width;
    let wrapped = text::wrap_to_width(&body, body_width);
    let indent = " ".repeat(prefix_width);

    wrapped
        .into_iter()
        .enumerate()
        .map(|(idx, part)| {
            if idx == 0 {
                let mut spans = Vec::with_capacity(3);
                if !timestamp.is_empty() {
                    spans.push(Span::styled(timestamp.clone(), styles.timestamp));
                }
                if !source_prefix.is_empty() {
                    spans.push(Span::styled(source_prefix.clone(), styles.source));
                }
                spans.push(Span::styled(part, styles.body));
                Line::from(spans)
            } else if prefix_options.continuation_indent {
                Line::from(vec![
                    Span::styled(indent.clone(), styles.body),
                    Span::styled(part, styles.body),
                ])
            } else {
                Line::from(Span::styled(part, styles.body))
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    struct TestSource {
        identity: u64,
        revision: u64,
        lines: Vec<log_bus::LogLine>,
    }

    impl LogSource for TestSource {
        fn identity(&self) -> u64 {
            self.identity
        }

        fn revision(&self) -> u64 {
            self.revision
        }

        fn visit(&self, range: LogRange, visitor: &mut dyn FnMut(&log_bus::LogLine)) -> u64 {
            let skip = match range {
                LogRange::All => 0,
                LogRange::Tail(count) => self.lines.len().saturating_sub(count),
            };
            for line in self.lines.iter().skip(skip) {
                visitor(line);
            }
            self.revision
        }
    }

    fn source(lines: &[&str]) -> TestSource {
        TestSource {
            identity: 10,
            revision: 1,
            lines: lines
                .iter()
                .enumerate()
                .map(|(idx, text)| log_bus::LogLine::test((idx + 1) as u64, "test", text))
                .collect(),
        }
    }

    fn plain(line: &Line<'_>) -> String {
        line.spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect()
    }

    #[test]
    fn wraps_unicode_and_sanitizes_terminal_controls() {
        let line = log_bus::LogLine::test(
            1,
            "测试",
            "\x1b[2K\r正在下载 50%|████ alpha-beta-without-spaces",
        );
        let rows = wrap_log_line(&line, 24, LogPrefix::default(), LogStyles::default());
        let joined = rows.iter().map(plain).collect::<Vec<_>>().join("\n");
        let compact = joined.split_whitespace().collect::<String>();

        assert!(rows.len() > 1);
        assert!(rows.iter().all(|row| row.width() <= 24));
        assert!(!joined.contains('\r'));
        assert!(!joined.contains('\x1b'));
        assert!(joined.contains("正在下载"));
        assert!(compact.contains("alpha-beta-without-spaces"));
    }

    #[test]
    fn continuation_rows_share_the_prefix_indent() {
        let line = log_bus::LogLine::test(1, "test", "alpha beta gamma delta epsilon");
        let prefix_width = text::width("00:00:00 [test] ");
        let rows = wrap_log_line(&line, 28, LogPrefix::default(), LogStyles::default());

        assert!(rows.len() > 1);
        assert!(plain(&rows[1]).starts_with(&" ".repeat(prefix_width)));
    }

    #[test]
    fn range_and_source_identity_invalidate_the_cache() {
        let first = source(&["one", "two", "three"]);
        let mut cache = ReflowCache::default();
        cache.sync(
            &first,
            LogRange::Tail(2),
            80,
            LogPrefix::default(),
            LogStyles::default(),
        );
        assert_eq!(cache.entries.len(), 2);
        assert_eq!(cache.entries[0].raw.text, "two");

        let second = TestSource {
            identity: 11,
            revision: 1,
            lines: vec![log_bus::LogLine::test(20, "other", "replacement")],
        };
        cache.sync(
            &second,
            LogRange::All,
            80,
            LogPrefix::default(),
            LogStyles::default(),
        );
        assert_eq!(cache.entries.len(), 1);
        assert_eq!(cache.entries[0].raw.text, "replacement");
    }

    #[test]
    fn content_versions_refresh_only_the_changed_entry() {
        let mut data = source(&["old text", "unchanged"]);
        let mut cache = ReflowCache::default();
        cache.sync(
            &data,
            LogRange::All,
            80,
            LogPrefix::default(),
            LogStyles::default(),
        );
        data.revision = 2;
        data.lines[0].version += 1;
        data.lines[0].text = "new text".to_string();
        cache.sync(
            &data,
            LogRange::All,
            80,
            LogPrefix::default(),
            LogStyles::default(),
        );

        assert!(plain(&cache.entries[0].rows[0]).contains("new text"));
        assert!(plain(&cache.entries[1].rows[0]).contains("unchanged"));
    }

    #[test]
    fn resize_resolves_the_same_log_anchor() {
        let data = source(&[
            "alpha beta gamma delta epsilon zeta eta theta",
            "second logical entry remains the anchor while resizing",
            "tail",
        ]);
        let mut cache = ReflowCache::default();
        cache.sync(
            &data,
            LogRange::All,
            24,
            LogPrefix::default(),
            LogStyles::default(),
        );
        let second_offset = cache.entries[0].rows.len();
        let anchor = cache.anchor_at(second_offset).expect("second entry anchor");

        cache.sync(
            &data,
            LogRange::All,
            48,
            LogPrefix::default(),
            LogStyles::default(),
        );
        let resized_offset = cache.resolve(anchor).expect("anchor survives resize");
        assert_eq!(
            cache.anchor_at(resized_offset).map(|a| a.id),
            Some(anchor.id)
        );
    }

    #[test]
    fn render_reflows_an_unchanged_source_at_the_new_width() {
        let data = source(&[
            "alpha beta gamma delta epsilon zeta eta theta iota kappa lambda mu",
            "the newest logical log entry must remain visible at the tail",
        ]);
        let display = LogDisplay::new();
        let mut narrow = Terminal::new(TestBackend::new(28, 4)).expect("narrow terminal");
        narrow
            .draw(|frame| {
                display.render(
                    frame,
                    frame.area(),
                    &data,
                    LogDisplayOptions {
                        viewport: LogViewportMode::Tail,
                        ..LogDisplayOptions::default()
                    },
                );
            })
            .expect("narrow render");
        let narrow_rows = display.total_rows.get();

        let mut wide = Terminal::new(TestBackend::new(52, 4)).expect("wide terminal");
        wide.draw(|frame| {
            display.render(
                frame,
                frame.area(),
                &data,
                LogDisplayOptions {
                    viewport: LogViewportMode::Tail,
                    ..LogDisplayOptions::default()
                },
            );
        })
        .expect("wide render");

        assert!(display.total_rows.get() < narrow_rows);
        assert_eq!(
            display.scroll.get(),
            display
                .total_rows
                .get()
                .saturating_sub(display.visible_rows.get())
        );
    }

    #[test]
    fn switching_sources_resets_a_non_tail_view_even_when_ids_overlap() {
        let first = source(&[
            "first source row one",
            "first source row two",
            "first source row three",
            "first source row four",
        ]);
        let second = TestSource {
            identity: 99,
            revision: 1,
            lines: vec![
                log_bus::LogLine::test(1, "other", "second source row one"),
                log_bus::LogLine::test(2, "other", "second source row two"),
                log_bus::LogLine::test(3, "other", "second source row three"),
                log_bus::LogLine::test(4, "other", "second source row four"),
            ],
        };
        let display = LogDisplay::new();
        let mut terminal = Terminal::new(TestBackend::new(40, 2)).expect("terminal");
        terminal
            .draw(|frame| {
                display.render(frame, frame.area(), &first, LogDisplayOptions::default());
            })
            .expect("first source render");
        display.navigate(LogNavigation::Home);
        display.navigate(LogNavigation::Lines(1));
        assert_eq!(display.scroll.get(), 1);

        terminal
            .draw(|frame| {
                display.render(frame, frame.area(), &second, LogDisplayOptions::default());
            })
            .expect("second source render");
        assert_eq!(display.scroll.get(), 0);
    }

    #[test]
    fn navigation_uses_visual_rows_and_rearms_tail_following() {
        let display = LogDisplay::new();
        display.total_rows.set(20);
        display.visible_rows.set(5);
        display.scroll.set(15);

        display.navigate(LogNavigation::Lines(-1));
        assert_eq!(display.scroll.get(), 14);
        assert!(!display.follow_tail.get());
        display.navigate(LogNavigation::Pages(1));
        assert_eq!(display.scroll.get(), 15);
        assert!(display.follow_tail.get());
        display.navigate(LogNavigation::Home);
        assert_eq!(display.scroll.get(), 0);
        display.navigate(LogNavigation::End);
        assert_eq!(display.scroll.get(), 15);
    }

    #[test]
    fn new_logs_follow_only_while_viewport_is_at_tail() {
        let mut data = source(&[
            "one", "two", "three", "four", "five", "six", "seven", "eight",
        ]);
        let display = LogDisplay::new();
        let mut terminal = Terminal::new(TestBackend::new(40, 3)).expect("terminal");
        terminal
            .draw(|frame| {
                display.render(frame, frame.area(), &data, LogDisplayOptions::default());
            })
            .expect("initial render");
        assert_eq!(display.scroll.get(), 5);
        assert!(display.follow_tail.get());

        display.navigate(LogNavigation::Lines(-1));
        assert_eq!(display.scroll.get(), 4);
        assert!(!display.follow_tail.get());

        data.revision = 2;
        data.lines.push(log_bus::LogLine::test(9, "test", "nine"));
        terminal
            .draw(|frame| {
                display.render(frame, frame.area(), &data, LogDisplayOptions::default());
            })
            .expect("paused render");
        assert_eq!(display.scroll.get(), 4);
        assert!(!display.follow_tail.get());

        display.navigate(LogNavigation::Lines(1));
        assert!(!display.follow_tail.get());
        display.navigate(LogNavigation::Lines(1));
        assert_eq!(display.scroll.get(), 6);
        assert!(display.follow_tail.get());

        data.revision = 3;
        data.lines.push(log_bus::LogLine::test(10, "test", "ten"));
        terminal
            .draw(|frame| {
                display.render(frame, frame.area(), &data, LogDisplayOptions::default());
            })
            .expect("following render");
        assert_eq!(display.scroll.get(), 7);
    }

    #[test]
    fn cramped_layout_drops_auxiliary_rows_before_logs() {
        assert_eq!(auxiliary_lengths(5, 2, 1, 1), (2, 1));
        assert_eq!(auxiliary_lengths(3, 2, 1, 1), (0, 0));
        assert_eq!(auxiliary_lengths(2, 2, 0, 1), (0, 0));
    }
}
