//! Shared row/column focus grid for TUI screen navigation.
//!
//! Every screen defines its focusable layout as a list of rows, each either
//! a fixed-column row or a scrollable list. The grid handles all cycling
//! logic (Up/Down/Left/Right/PgUp/PgDn/scroll) so screens only supply
//! the row definitions and react to the resulting focus position.

/// Describes a row's column structure.
#[derive(Clone, Debug)]
pub enum RowKind {
    /// A fixed row with the given number of columns.
    Fixed(usize),
    /// A scrollable list row. The grid tracks the selected index and scroll
    /// offset; the screen sets the current list length before navigation.
    List,
}

/// Centralized focus grid managing row/column navigation.
pub struct FocusGrid {
    rows: Vec<RowKind>,
    row: usize,
    col: usize,
    last_col: Vec<usize>,
    list_selected: usize,
    list_scroll: usize,
    list_len: usize,
    visible_rows: usize,
}

impl FocusGrid {
    /// Constructs a grid from the given row definitions.
    pub fn new(rows: Vec<RowKind>) -> Self {
        let n = rows.len();
        Self {
            rows,
            row: 0,
            col: 0,
            last_col: vec![0; n],
            list_selected: 0,
            list_scroll: 0,
            list_len: 0,
            visible_rows: 1,
        }
    }

    /// Sets the current list length. Call before navigation each frame.
    pub fn set_list_len(&mut self, len: usize) {
        self.list_len = len;
        if self.in_list() && self.list_selected >= len && len > 0 {
            self.list_selected = len - 1;
        }
    }

    /// Sets the visible row count for scroll clamping.
    pub fn set_visible_rows(&mut self, v: usize) {
        self.visible_rows = v.max(1);
    }

    // ── Accessors ─────────────────────────────────────────────────────

    /// Returns the current row index.
    pub fn row(&self) -> usize {
        self.row
    }

    /// Returns the current column index within the current row.
    pub fn col(&self) -> usize {
        self.col
    }

    /// Returns the selected index within a `List` row.
    pub fn list_selected(&self) -> usize {
        self.list_selected
    }

    /// Returns the scroll offset within a `List` row.
    pub fn list_scroll(&self) -> usize {
        self.list_scroll
    }

    /// Returns whether the current row is a `List`.
    pub fn in_list(&self) -> bool {
        matches!(self.rows.get(self.row), Some(RowKind::List))
    }

    // ── Navigation ────────────────────────────────────────────────────

    /// Moves up one step. Within a list: selects the previous item;
    /// at the first item or on a fixed row: cycles to the previous row.
    pub fn move_up(&mut self) {
        if self.in_list() && self.list_selected > 0 {
            self.list_selected -= 1;
            self.ensure_visible();
            return;
        }
        self.go_prev_row();
    }

    /// Moves down one step. Within a list: selects the next item;
    /// at the last item or on a fixed row: cycles to the next row.
    pub fn move_down(&mut self) {
        if self.in_list() && self.list_len > 0 && self.list_selected + 1 < self.list_len {
            self.list_selected += 1;
            self.ensure_visible();
            return;
        }
        self.go_next_row();
    }

    /// Moves left within a fixed row's columns. Returns `true` if
    /// consumed (column moved), `false` if at the left edge (propagate
    /// to tab switching or other handler).
    pub fn move_left(&mut self) -> bool {
        if let Some(RowKind::Fixed(n)) = self.rows.get(self.row) {
            if self.col > 0 {
                self.col -= 1;
                self.last_col[self.row] = self.col;
                return true;
            }
            if *n <= 1 {
                return false;
            }
        }
        false
    }

    /// Moves right within a fixed row's columns. Returns `true` if
    /// consumed, `false` if at the right edge.
    pub fn move_right(&mut self) -> bool {
        if let Some(RowKind::Fixed(n)) = self.rows.get(self.row) {
            if self.col + 1 < *n {
                self.col += 1;
                self.last_col[self.row] = self.col;
                return true;
            }
        }
        false
    }

    /// Jumps to the first row.
    pub fn page_up(&mut self) {
        self.save_col();
        self.row = 0;
        self.restore_col();
        if self.in_list() {
            self.list_selected = 0;
            self.list_scroll = 0;
        }
    }

    /// Jumps to the last row. If the last row is a list, selects its
    /// last item.
    pub fn page_down(&mut self) {
        self.save_col();
        self.row = self.rows.len().saturating_sub(1);
        self.restore_col();
        if self.in_list() && self.list_len > 0 {
            self.list_selected = self.list_len - 1;
            self.ensure_visible();
        }
    }

    /// Wheel scroll — same cycling as move_up/move_down.
    pub fn scroll(&mut self, delta: i32) {
        if delta < 0 {
            self.move_up();
        } else {
            self.move_down();
        }
    }

    /// Directly sets the row and column (e.g., after a mouse click).
    pub fn set_focus(&mut self, row: usize, col: usize) {
        self.save_col();
        self.row = row.min(self.rows.len().saturating_sub(1));
        self.col = col;
        self.last_col[self.row] = self.col;
    }

    /// Directly sets the list selection (e.g., after a mouse click on
    /// a specific list item).
    pub fn set_list_selected(&mut self, sel: usize) {
        self.list_selected = sel;
    }

    /// Directly sets the list scroll offset.
    pub fn set_list_scroll(&mut self, scr: usize) {
        self.list_scroll = scr;
    }

    /// Clamps scroll so the selected list item stays visible.
    pub fn ensure_visible(&mut self) {
        let v = self.visible_rows.max(1);
        if self.list_selected < self.list_scroll {
            self.list_scroll = self.list_selected;
        }
        let max_off = self.list_selected.saturating_sub(v - 1);
        if self.list_scroll < max_off {
            self.list_scroll = max_off;
        }
        if self.list_scroll > self.list_selected {
            self.list_scroll = self.list_selected;
        }
    }

    // ── Internal ──────────────────────────────────────────────────────

    fn save_col(&mut self) {
        if self.row < self.last_col.len() {
            self.last_col[self.row] = self.col;
        }
    }

    fn restore_col(&mut self) {
        self.col = self.last_col.get(self.row).copied().unwrap_or(0);
        // Clamp to the actual column count of the new row.
        if let Some(RowKind::Fixed(n)) = self.rows.get(self.row) {
            self.col = self.col.min(n.saturating_sub(1));
        } else {
            self.col = 0;
        }
    }

    fn go_prev_row(&mut self) {
        self.save_col();
        if self.row == 0 {
            self.row = self.rows.len().saturating_sub(1);
        } else {
            self.row -= 1;
        }
        self.restore_col();
        // If landing on a List row, select the last item.
        if self.in_list() && self.list_len > 0 {
            self.list_selected = self.list_len - 1;
            self.ensure_visible();
        }
    }

    fn go_next_row(&mut self) {
        self.save_col();
        if self.row + 1 >= self.rows.len() {
            self.row = 0;
        } else {
            self.row += 1;
        }
        self.restore_col();
        // If landing on a List row, select the first item.
        if self.in_list() {
            self.list_selected = 0;
            self.list_scroll = 0;
        }
    }
}
