//! Reusable ratatui widgets used by the launcher screens.

/// Focusable button widget.
pub mod button;
/// Dropdown selector built on top of the popup menu.
pub mod dropdown;
/// Shared row/column focus grid for screen navigation.
pub mod focus_grid;
/// Single-line text input widget.
pub mod input;
/// Data-source-independent, width-aware log display.
pub mod log_display;
/// Vertical menu list widget.
pub mod menu;
/// Modal popup widgets (confirm, input, menu, notice, select).
pub mod popup;
/// Generic table widget.
pub mod table;
/// Top-bar tab strip widget.
pub mod tabs;
/// Boolean toggle widget.
pub mod toggle;
