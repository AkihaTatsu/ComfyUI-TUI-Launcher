//! Shared schema-driven settings widget.
//!
//! Both the ComfyUI Settings screen and the Launcher Settings screen render
//! the same layout — a (possibly multi-tab) bordered body of rows, where each
//! row is one of three field kinds: `Toggle`, `Choice`, or `Custom`. They use
//! the same popups for editing (`Select` for Choice, `InputPopup` for Custom),
//! the same key bindings, the same mouse rules, and emit any validation
//! errors through a shared `Notice` popup.
//!
//! Callers own:
//!   1. A `Schema` describing the visible fields (built from disk for ComfyUI
//!      Settings; built in-code for Launcher Settings).
//!   2. A `Config` (read for rendering, read+write for input handling).
//!   3. A `get` closure mapping `(cfg, key) -> Value` and a `set` closure
//!      mapping `(cfg, key, value) -> Result<(), String>`. Both are stateless;
//!      this lets the view sequence read and write borrows without colliding.

use crate::core::config::Config;
use crate::core::schema::{FieldType, Schema};
use crate::core::{gpu, i18n, theme};
use crate::widgets::focus_grid::{FocusGrid, RowKind};
use crate::widgets::input::Input;
use crate::widgets::popup::input_popup::InputPopup;
use crate::widgets::popup::select::Select;
use crate::widgets::{dropdown, tabs::Tabs, toggle};
use crossterm::event::{KeyCode, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;
use std::cell::Cell;
use toml::Value;
use unicode_width::UnicodeWidthStr;

/// A field reference that records both which tab and which field-within-tab
/// a match refers to. Used by cross-tab search.
struct FilteredField {
    tab_idx: usize,
    field_idx: usize,
}

/// Currently displayed popup, if any.
pub enum Popup {
    /// No popup is open.
    None,
    /// Select popup for `Choice` fields, paired with the field key.
    Select(Select, String),
    /// Input popup for `Custom` fields, paired with the field key.
    Input(InputPopup, String),
}

/// Schema-driven settings editor shared by ComfyUI Settings and Launcher
/// Settings.
pub struct SettingsView {
    /// Active tab index.
    pub tab: usize,
    /// Saved tab index to restore when cross-tab search is cleared.
    saved_tab: Option<usize>,
    /// Centralized focus grid: Row 0 = filter input, Row 1 = field list.
    pub grid: FocusGrid,
    /// Active popup, when one is open.
    pub popup: Popup,
    /// Pending flash message awaiting promotion to the application banner.
    ///
    /// Set when a validating setter rejects user input.
    pub pending_flash: Option<(crate::app::FlashKind, String)>,
    /// Scroll offset measured in fields (within the filtered view).
    /// Kept as Cell for render-time clamping (render takes &self).
    pub scroll: Cell<usize>,
    /// Approximate number of fields that fit in the inner body area on the
    /// last frame, used by `on_mouse` for click-to-field hit-tests.
    visible: Cell<usize>,
    /// Filter text input for searching settings by name or description.
    pub filter: Input,
}

impl SettingsView {
    /// Constructs a fresh editor.
    pub fn new() -> Self {
        let mut grid = FocusGrid::new(vec![RowKind::Fixed(1), RowKind::List]);
        // Default focus on the field list (row 1).
        grid.set_focus(1, 0);
        Self {
            tab: 0,
            saved_tab: None,
            grid,
            popup: Popup::None,
            pending_flash: None,
            scroll: Cell::new(0),
            visible: Cell::new(0),
            filter: Input::default().placeholder("placeholder_search"),
        }
    }

    /// Whether the filter input currently has keyboard focus.
    pub fn filter_focused(&self) -> bool {
        self.grid.row() == 0
    }

    /// Drains and returns any pending flash message produced since the last
    /// dispatch.
    pub fn take_flash(&mut self) -> Option<(crate::app::FlashKind, String)> {
        self.pending_flash.take()
    }

    /// Wrap a description into rendered lines. Each `\n`-separated chunk
    /// is word-wrapped to the available width (accounting for the 4-cell
    /// indent prefix). Returns at least one element so per-field math
    /// stays consistent for empty / very-narrow areas.
    fn wrap_desc(desc: &str, body_width: usize) -> Vec<String> {
        let indent = 4usize;
        let avail = body_width.saturating_sub(indent).max(1);
        let mut out: Vec<String> = Vec::new();
        for chunk in desc.split('\n') {
            for line in crate::core::text::wrap_to_width(chunk, avail) {
                out.push(line);
            }
        }
        if out.is_empty() {
            out.push(String::new());
        }
        out
    }

    /// Per-field visible height in lines: 1 (title) + total wrapped desc
    /// lines for the current body width.
    fn field_height(desc: &str, body_width: usize) -> usize {
        1 + Self::wrap_desc(desc, body_width).len()
    }

    /// Returns filtered fields. When the filter is empty, returns fields from
    /// the active tab only. When non-empty, searches across ALL tabs.
    fn filtered_fields(&self, schema: &Schema) -> Vec<FilteredField> {
        let needle = self.filter.value.to_lowercase();
        if needle.is_empty() {
            let tab = match schema.tabs.get(self.tab) {
                Some(t) => t,
                None => return Vec::new(),
            };
            return (0..tab.fields.len())
                .map(|i| FilteredField {
                    tab_idx: self.tab,
                    field_idx: i,
                })
                .collect();
        }
        let mut out = Vec::new();
        for (tab_idx, tab) in schema.tabs.iter().enumerate() {
            for (field_idx, f) in tab.fields.iter().enumerate() {
                if i18n::t(&f.name).to_lowercase().contains(&needle)
                    || i18n::t(&f.desc).to_lowercase().contains(&needle)
                {
                    out.push(FilteredField { tab_idx, field_idx });
                }
            }
        }
        out
    }

    /// Closes any open popup or clears the filter. Returns whether Esc was
    /// consumed.
    pub fn eat_esc(&mut self) -> bool {
        if !matches!(self.popup, Popup::None) {
            self.popup = Popup::None;
            return true;
        }
        if self.filter_focused() {
            self.grid.set_focus(1, 0);
            return true;
        }
        if !self.filter.value.is_empty() {
            self.filter.value.clear();
            self.filter.cursor = 0;
            self.grid.set_list_selected(0);
            self.scroll.set(0);
            // Restore saved tab.
            if let Some(t) = self.saved_tab.take() {
                self.tab = t;
            }
            return true;
        }
        false
    }

    fn body_area(area: Rect, show_tabs: bool) -> Rect {
        if show_tabs {
            Layout::default()
                .direction(Direction::Vertical)
                .constraints([Constraint::Length(2), Constraint::Min(0)])
                .split(area)[1]
        } else {
            area
        }
    }

    /// Renders the editor into `area`.
    #[allow(clippy::too_many_arguments)]
    pub fn render<G>(
        &self,
        f: &mut Frame,
        area: Rect,
        schema: &Schema,
        cfg: &Config,
        get: G,
        show_tabs: bool,
        body_active: bool,
    ) where
        G: Fn(&Config, &str) -> Value,
    {
        let row_active = body_active && matches!(self.popup, Popup::None) && !self.filter_focused();

        let filtered = self.filtered_fields(schema);
        let filter_active = !self.filter.value.is_empty();

        // Tab strip.
        if show_tabs {
            let v = Layout::default()
                .direction(Direction::Vertical)
                .constraints([Constraint::Length(2), Constraint::Min(0)])
                .split(area);
            let names: Vec<String> = schema.tabs.iter().map(|t| i18n::t(&t.name)).collect();
            if filter_active {
                // Compute highlighted: true if tab has any matching fields.
                let highlighted: Vec<bool> = (0..schema.tabs.len())
                    .map(|ti| filtered.iter().any(|ff| ff.tab_idx == ti))
                    .collect();
                Tabs {
                    items: &names,
                    selected: self.tab,
                    highlighted: Some(&highlighted),
                }
                .render(f, v[0]);
            } else {
                Tabs {
                    items: &names,
                    selected: self.tab,
                    highlighted: None,
                }
                .render(f, v[0]);
            }
        }
        let below_tabs = Self::body_area(area, show_tabs);

        // Split: filter input (3 lines) + field list (rest).
        let parts = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(3), Constraint::Min(0)])
            .split(below_tabs);
        let filter_focused =
            body_active && self.filter_focused() && matches!(self.popup, Popup::None);
        self.filter.render(f, parts[0], filter_focused);
        let body_area = parts[1];

        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(theme::border());
        f.render_widget(block, body_area);
        let inner = Rect {
            x: body_area.x + 1,
            y: body_area.y + 1,
            width: body_area.width.saturating_sub(2),
            height: body_area.height.saturating_sub(2),
        };

        let avail_h = inner.height as usize;
        let body_w = inner.width as usize;

        // Pre-wrap descriptions for filtered fields only.
        let descs: Vec<Vec<String>> = filtered
            .iter()
            .map(|ff| {
                let tab = &schema.tabs[ff.tab_idx];
                Self::wrap_desc(&i18n::t(&tab.fields[ff.field_idx].desc), body_w)
            })
            .collect();
        let heights: Vec<usize> = descs.iter().map(|d| 1 + d.len()).collect();

        // Track which filtered-field indices start a new tab group (for
        // section headers). A section header is shown when filter is active
        // and the tab_idx changes from the previous entry.
        let section_headers: Vec<bool> = filtered
            .iter()
            .enumerate()
            .map(|(i, ff)| filter_active && (i == 0 || ff.tab_idx != filtered[i - 1].tab_idx))
            .collect();

        // Scroll clamping within the filtered list.
        let row = self
            .grid
            .list_selected()
            .min(filtered.len().saturating_sub(1));
        let mut scroll = self.scroll.get().min(row);
        loop {
            let mut used = 0usize;
            let mut last = scroll;
            for i in scroll..heights.len() {
                let extra = if section_headers[i] { 1 } else { 0 };
                if used + heights[i] + extra > avail_h {
                    break;
                }
                used += heights[i] + extra;
                last = i;
            }
            if row <= last || scroll >= row {
                break;
            }
            scroll += 1;
        }
        self.scroll.set(scroll);

        let mut lines: Vec<Line> = Vec::with_capacity(avail_h);
        let mut used = 0usize;
        let mut last_visible = scroll;
        for vi in scroll..filtered.len() {
            let header_h = if section_headers[vi] { 1 } else { 0 };
            if used + heights[vi] + header_h > avail_h {
                break;
            }

            // Insert section header when crossing tab boundaries.
            if section_headers[vi] {
                let tab_name = i18n::t(&schema.tabs[filtered[vi].tab_idx].name);
                let avail = inner.width as usize;
                let label = format!(" {} ", tab_name);
                let label_w = label.width();
                let dash_left = 2;
                let dash_right = avail.saturating_sub(dash_left + label_w);
                let header = format!(
                    "{}{}{}",
                    "─".repeat(dash_left),
                    label,
                    "─".repeat(dash_right),
                );
                lines.push(Line::from(Span::styled(header, theme::accent())));
                used += 1;
            }

            let ff = &filtered[vi];
            let tab = &schema.tabs[ff.tab_idx];
            let fld = &tab.fields[ff.field_idx];
            let cur = get(cfg, &fld.key);
            let selected = vi == row;
            let focused = selected && row_active;
            let title_style: Style = if focused {
                theme::focused()
            } else {
                theme::accent()
            };
            let modified = cur != fld.default;
            let right = match &fld.ty {
                FieldType::Toggle => {
                    toggle::span(cur.as_bool().unwrap_or(false), modified, focused)
                }
                FieldType::Choice { options } => {
                    let cur_s = cur.as_str().unwrap_or("");
                    let label = options
                        .iter()
                        .find(|o| o.value == cur_s)
                        .map(|o| i18n::t(&o.label))
                        .unwrap_or_else(|| i18n::t("label_default"));
                    dropdown::summary(&label, modified, focused)
                }
                FieldType::LanguageChoice => {
                    let cur_s = cur.as_str().unwrap_or("");
                    let label = i18n::available_locales()
                        .into_iter()
                        .find(|(code, _)| code == cur_s)
                        .map(|(_, native)| native)
                        .unwrap_or_else(|| i18n::t("label_default"));
                    dropdown::summary(&label, modified, focused)
                }
                FieldType::GpuChoice => {
                    let cur_s = cur.as_str().unwrap_or("");
                    let label = if cur_s.is_empty() {
                        gpu::default_label(&i18n::t("label_default"))
                    } else {
                        let gpus = gpu::detect();
                        gpus.iter()
                            .find(|g| g.config_value() == cur_s)
                            .map(|g| g.display_label())
                            .unwrap_or_else(|| cur_s.to_string())
                    };
                    dropdown::summary(&label, modified, focused)
                }
                FieldType::Custom => {
                    let s = cur.as_str().unwrap_or("").to_string();
                    let is_empty = s.is_empty();
                    let shown = if is_empty { i18n::t("label_empty") } else { s };
                    let style = if focused {
                        theme::focused()
                    } else if is_empty {
                        theme::placeholder()
                    } else if modified {
                        theme::accent()
                    } else {
                        theme::base()
                    };
                    Span::styled(shown, style)
                }
            };
            let avail = inner.width as usize;
            let title = i18n::t(&fld.name);
            let pad = avail.saturating_sub(title.width() + right.content.width());
            lines.push(Line::from(vec![
                Span::styled(title, title_style),
                Span::raw(" ".repeat(pad)),
                right,
            ]));
            for d in &descs[vi] {
                lines.push(Line::from(Span::styled(format!("    {d}"), theme::base())));
            }
            used += heights[vi];
            last_visible = vi;
        }
        self.visible
            .set((last_visible + 1).saturating_sub(scroll).max(1));
        f.render_widget(Paragraph::new(lines), inner);

        match &self.popup {
            Popup::Select(s, _) => s.render(f, area),
            Popup::Input(ip, _) => ip.render(f, area),
            _ => {}
        }
    }

    /// Handles a key event. Returns `true` if the key was consumed (text
    /// input, popup, or navigation ate it), `false` if it should propagate
    /// to screen-level shortcuts.
    pub fn on_key<G, S>(
        &mut self,
        code: KeyCode,
        schema: &Schema,
        cfg: &mut Config,
        get: G,
        set: S,
    ) -> bool
    where
        G: Fn(&Config, &str) -> Value,
        S: Fn(&mut Config, &str, Value) -> Result<(), String>,
    {
        // Popup keys take priority — always consumed.
        match &mut self.popup {
            Popup::Select(sel, key) => {
                match code {
                    KeyCode::Esc => self.popup = Popup::None,
                    _ if sel.on_key(code) => {}
                    KeyCode::Enter => {
                        if let Some(fld) = schema
                            .tabs
                            .iter()
                            .flat_map(|t| t.fields.iter())
                            .find(|f| &f.key == key)
                        {
                            let new_value: Option<String> = match &fld.ty {
                                FieldType::Choice { options } => {
                                    options.get(sel.selected).map(|o| o.value.clone())
                                }
                                FieldType::LanguageChoice => i18n::available_locales()
                                    .get(sel.selected)
                                    .map(|(c, _)| c.clone()),
                                FieldType::GpuChoice => gpu_choice_value(sel.selected),
                                _ => None,
                            };
                            if let Some(v) = new_value {
                                let res = set(cfg, key, Value::String(v));
                                self.popup = Popup::None;
                                if let Err(msg) = res {
                                    self.pending_flash = Some((crate::app::FlashKind::Error, msg));
                                }
                                return true;
                            }
                        }
                        self.popup = Popup::None;
                    }
                    _ => {}
                }
                return true;
            }
            Popup::Input(ip, key) => {
                match code {
                    KeyCode::Esc => self.popup = Popup::None,
                    KeyCode::Enter => {
                        let val = ip.input.value.trim().to_string();
                        let key_owned = key.clone();
                        self.popup = Popup::None;
                        let res = set(cfg, &key_owned, Value::String(val));
                        if let Err(msg) = res {
                            self.pending_flash = Some((crate::app::FlashKind::Error, msg));
                        }
                    }
                    k => ip.input.on_key(k),
                }
                return true;
            }
            _ => {
                let filter_was_empty = self.filter.value.is_empty();

                // Filter input has focus — route typing keys there.
                if self.filter_focused() {
                    match code {
                        KeyCode::Esc => {
                            self.grid.set_focus(1, 0);
                        }
                        KeyCode::Enter | KeyCode::Down => {
                            self.grid.set_focus(1, 0);
                            self.grid.set_list_selected(0);
                            self.scroll.set(0);
                        }
                        KeyCode::Up => {
                            let n = self.filtered_fields(schema).len();
                            if n > 0 {
                                self.grid.set_focus(1, 0);
                                self.grid.set_list_selected(n - 1);
                            }
                        }
                        KeyCode::Tab | KeyCode::BackTab => {
                            self.grid.set_focus(1, 0);
                        }
                        KeyCode::Left => {
                            if !self.filter.at_start() {
                                self.filter.on_key(KeyCode::Left);
                            } else if self.filter.value.is_empty() {
                                let n = schema.tabs.len();
                                if n > 1 {
                                    self.tab = if self.tab == 0 { n - 1 } else { self.tab - 1 };
                                    self.grid.set_list_selected(0);
                                    self.scroll.set(0);
                                }
                            }
                        }
                        KeyCode::Right => {
                            if !self.filter.at_end() {
                                self.filter.on_key(KeyCode::Right);
                            } else if self.filter.value.is_empty() {
                                let n = schema.tabs.len();
                                if n > 1 {
                                    self.tab = (self.tab + 1) % n;
                                    self.grid.set_list_selected(0);
                                    self.scroll.set(0);
                                }
                            }
                        }
                        k => {
                            self.filter.on_key(k);
                            self.grid.set_list_selected(0);
                            self.scroll.set(0);
                            // Transition: filter was empty and is now non-empty → save tab.
                            if filter_was_empty && !self.filter.value.is_empty() {
                                self.saved_tab = Some(self.tab);
                            }
                            // Transition: filter was non-empty and is now empty → restore tab.
                            if !filter_was_empty && self.filter.value.is_empty() {
                                if let Some(t) = self.saved_tab.take() {
                                    self.tab = t;
                                }
                            }
                        }
                    }
                    return true;
                }
                // Field list has focus.
                let n = self.filtered_fields(schema).len();
                match code {
                    KeyCode::Tab | KeyCode::BackTab => {
                        self.grid.set_focus(0, 0);
                    }
                    KeyCode::Up => {
                        if self.grid.list_selected() == 0 {
                            self.grid.set_focus(0, 0);
                        } else {
                            self.grid.set_list_selected(self.grid.list_selected() - 1);
                        }
                    }
                    KeyCode::Down => {
                        if n > 0 && self.grid.list_selected() + 1 >= n {
                            self.grid.set_focus(0, 0);
                        } else if n > 0 {
                            self.grid.set_list_selected(self.grid.list_selected() + 1);
                        }
                    }
                    KeyCode::PageUp => {
                        self.grid.set_focus(0, 0);
                    }
                    KeyCode::PageDown => {
                        if n > 0 {
                            self.grid.set_list_selected(n - 1);
                        } else {
                            self.grid.set_focus(0, 0);
                        }
                    }
                    KeyCode::Right => {
                        if self.filter.value.is_empty() {
                            let nt = schema.tabs.len();
                            if nt > 1 {
                                self.tab = (self.tab + 1) % nt;
                                self.grid.set_list_selected(0);
                                self.scroll.set(0);
                            }
                        }
                    }
                    KeyCode::Left => {
                        if self.filter.value.is_empty() {
                            let nt = schema.tabs.len();
                            if nt > 1 {
                                self.tab = if self.tab == 0 { nt - 1 } else { self.tab - 1 };
                                self.grid.set_list_selected(0);
                                self.scroll.set(0);
                            }
                        }
                    }
                    KeyCode::Enter | KeyCode::Char(' ') => {
                        let filtered = self.filtered_fields(schema);
                        if let Some(ff) = filtered.get(self.grid.list_selected()) {
                            self.activate_field(ff.tab_idx, ff.field_idx, schema, cfg, &get, &set);
                        }
                    }
                    _ => {
                        return false;
                    }
                }
            }
        }
        true
    }

    fn activate_field<G, S>(
        &mut self,
        tab_idx: usize,
        field_idx: usize,
        schema: &Schema,
        cfg: &mut Config,
        get: &G,
        set: &S,
    ) where
        G: Fn(&Config, &str) -> Value,
        S: Fn(&mut Config, &str, Value) -> Result<(), String>,
    {
        let fld = match schema
            .tabs
            .get(tab_idx)
            .and_then(|t| t.fields.get(field_idx))
        {
            Some(f) => f.clone(),
            None => return,
        };
        let cur = get(cfg, &fld.key);
        match &fld.ty {
            FieldType::Toggle => {
                let new = !cur.as_bool().unwrap_or(false);
                let _ = set(cfg, &fld.key, Value::Boolean(new));
            }
            FieldType::Choice { options } => {
                let items: Vec<String> = options.iter().map(|o| i18n::t(&o.label)).collect();
                let cur_s = cur.as_str().unwrap_or("");
                let sel = options.iter().position(|o| o.value == cur_s).unwrap_or(0);
                self.popup = Popup::Select(
                    Select {
                        title: i18n::t(&fld.name),
                        items,
                        selected: sel,
                    },
                    fld.key.clone(),
                );
            }
            FieldType::LanguageChoice => {
                let locales = i18n::available_locales();
                let items: Vec<String> = locales.iter().map(|(_, n)| n.clone()).collect();
                let cur_s = cur.as_str().unwrap_or("");
                let sel = locales.iter().position(|(c, _)| c == cur_s).unwrap_or(0);
                self.popup = Popup::Select(
                    Select {
                        title: i18n::t(&fld.name),
                        items,
                        selected: sel,
                    },
                    fld.key.clone(),
                );
            }
            FieldType::GpuChoice => {
                let gpus = gpu::detect();
                let mut items: Vec<String> = Vec::with_capacity(gpus.len() + 1);
                items.push(gpu::default_label(&i18n::t("label_default")));
                items.extend(gpus.iter().map(|g| g.display_label()));
                let cur_s = cur.as_str().unwrap_or("");
                let sel = if cur_s.is_empty() {
                    0
                } else {
                    gpus.iter()
                        .position(|g| g.config_value() == cur_s)
                        .map(|i| i + 1)
                        .unwrap_or(0)
                };
                self.popup = Popup::Select(
                    Select {
                        title: i18n::t(&fld.name),
                        items,
                        selected: sel,
                    },
                    fld.key.clone(),
                );
            }
            FieldType::Custom => {
                self.popup = Popup::Input(
                    InputPopup::new(i18n::t(&fld.name), cur.as_str().unwrap_or("").to_string()),
                    fld.key.clone(),
                );
            }
        }
    }

    /// Handles a mouse event.
    #[allow(clippy::too_many_arguments)]
    pub fn on_mouse<G, S>(
        &mut self,
        m: MouseEvent,
        area: Rect,
        schema: &Schema,
        cfg: &mut Config,
        get: G,
        set: S,
        show_tabs: bool,
    ) where
        G: Fn(&Config, &str) -> Value,
        S: Fn(&mut Config, &str, Value) -> Result<(), String>,
    {
        if !matches!(m.kind, MouseEventKind::Down(MouseButton::Left)) {
            return;
        }

        // Popup hit-tests first.
        if let Popup::Select(sel, key) = &mut self.popup {
            let r = sel.popup_rect(area);
            let inside = m.column >= r.x
                && m.column < r.x + r.width
                && m.row >= r.y
                && m.row < r.y + r.height;
            if !inside {
                self.popup = Popup::None;
                return;
            }
            if let Some(idx) = sel.hit(area, m.column, m.row) {
                if idx != sel.selected {
                    sel.selected = idx;
                    return;
                }
                if let Some(fld) = schema
                    .tabs
                    .iter()
                    .flat_map(|t| t.fields.iter())
                    .find(|f| &f.key == key)
                {
                    let new_value: Option<String> = match &fld.ty {
                        FieldType::Choice { options } => options.get(idx).map(|o| o.value.clone()),
                        FieldType::LanguageChoice => {
                            i18n::available_locales().get(idx).map(|(c, _)| c.clone())
                        }
                        FieldType::GpuChoice => gpu_choice_value(idx),
                        _ => None,
                    };
                    if let Some(v) = new_value {
                        let res = set(cfg, key, Value::String(v));
                        self.popup = Popup::None;
                        if let Err(msg) = res {
                            self.pending_flash = Some((crate::app::FlashKind::Error, msg));
                        }
                    }
                }
            }
            return;
        }
        if matches!(self.popup, Popup::Input(_, _)) {
            let r = crate::widgets::popup::center(area, 60, 5);
            let inside = m.column >= r.x
                && m.column < r.x + r.width
                && m.row >= r.y
                && m.row < r.y + r.height;
            if !inside {
                self.popup = Popup::None;
            }
            return;
        }

        // Tab strip
        if show_tabs {
            let v = Layout::default()
                .direction(Direction::Vertical)
                .constraints([Constraint::Length(2), Constraint::Min(0)])
                .split(area);
            if m.row >= v[0].y && m.row < v[0].y + v[0].height {
                // No-op when filter is non-empty.
                if !self.filter.value.is_empty() {
                    return;
                }
                let names: Vec<String> = schema.tabs.iter().map(|t| i18n::t(&t.name)).collect();
                if let Some(h) = (crate::widgets::tabs::Tabs {
                    items: &names,
                    selected: self.tab,
                    highlighted: None,
                })
                .hit(v[0], m.column)
                {
                    use crate::widgets::tabs::HitResult;
                    let n = schema.tabs.len();
                    let new_tab = match h {
                        HitResult::Tab(t) => Some(t),
                        HitResult::PrevChevron if n > 1 => {
                            Some(if self.tab == 0 { n - 1 } else { self.tab - 1 })
                        }
                        HitResult::NextChevron if n > 1 => Some((self.tab + 1) % n),
                        _ => None,
                    };
                    if let Some(t) = new_tab {
                        self.tab = t;
                        self.grid.set_list_selected(0);
                        self.scroll.set(0);
                    }
                }
                return;
            }
        }
        let below_tabs = Self::body_area(area, show_tabs);
        let parts = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(3), Constraint::Min(0)])
            .split(below_tabs);

        // Click on filter input.
        if m.row >= parts[0].y && m.row < parts[0].y + parts[0].height {
            self.grid.set_focus(0, 0);
            return;
        }

        let body_area = parts[1];
        if m.row < body_area.y + 1 {
            return;
        }

        self.grid.set_focus(1, 0);

        let filtered = self.filtered_fields(schema);
        let filter_active = !self.filter.value.is_empty();
        let body_w = body_area.width.saturating_sub(2) as usize;
        let rel = (m.row - (body_area.y + 1)) as usize;
        let start = self.scroll.get();
        let mut used = 0usize;
        let mut vis_idx: Option<usize> = None;
        for vi in start..filtered.len() {
            let ff = &filtered[vi];
            let tab = &schema.tabs[ff.tab_idx];
            // Account for section header line.
            let has_header = filter_active && (vi == 0 || ff.tab_idx != filtered[vi - 1].tab_idx);
            if has_header {
                if rel < used + 1 {
                    // Clicked on a section header — not selectable.
                    return;
                }
                used += 1;
            }
            let h = Self::field_height(&i18n::t(&tab.fields[ff.field_idx].desc), body_w);
            if rel < used + h {
                vis_idx = Some(vi);
                break;
            }
            used += h;
        }
        let Some(vi) = vis_idx else {
            return;
        };
        if vi != self.grid.list_selected() {
            self.grid.set_list_selected(vi);
            return;
        }
        let ff = &filtered[vi];
        self.activate_field(ff.tab_idx, ff.field_idx, schema, cfg, &get, &set);
    }

    /// Handles a wheel-scroll event.
    pub fn scroll(&mut self, delta: i32, schema: &Schema) {
        if !matches!(self.popup, Popup::None) {
            if let Popup::Select(s, _) = &mut self.popup {
                if delta < 0 {
                    s.up();
                } else {
                    s.down();
                }
            }
            return;
        }
        let n = self.filtered_fields(schema).len();
        if n == 0 {
            return;
        }
        if delta < 0 {
            if self.filter_focused() {
                self.grid.set_focus(1, 0);
                self.grid.set_list_selected(n - 1);
                return;
            }
            if self.grid.list_selected() == 0 {
                self.grid.set_focus(0, 0);
                return;
            }
            self.grid.set_list_selected(self.grid.list_selected() - 1);
            return;
        }
        if delta > 0 {
            if self.filter_focused() {
                self.grid.set_focus(1, 0);
                self.grid.set_list_selected(0);
                self.scroll.set(0);
                return;
            }
            if self.grid.list_selected() + 1 >= n {
                self.grid.set_focus(0, 0);
                return;
            }
            self.grid.set_list_selected(self.grid.list_selected() + 1);
        }
    }
}

/// Maps a Select popup index back to the config value for a `GpuChoice` field.
///
/// Index 0 is the "Default" entry (empty string); indices 1.. correspond to
/// detected GPUs in `gpu::detect()` order.
fn gpu_choice_value(idx: usize) -> Option<String> {
    if idx == 0 {
        return Some(String::new());
    }
    let gpus = gpu::last_detected();
    gpus.get(idx - 1).map(|g| g.config_value())
}
