//! Width-aware text utilities for terminal layout.
//!
//! Terminal cells are not Unicode characters: CJK ideographs, fullwidth
//! punctuation, and many emoji render in two cells, while combining marks
//! and zero-width joiners render in zero. These helpers wrap
//! `unicode_width` so every measuring or truncating site in the codebase
//! shares one implementation.

use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

/// Returns the display width of `s` in terminal cells.
pub fn width(s: &str) -> usize {
    UnicodeWidthStr::width(s)
}

/// Truncates `s` so its rendered width does not exceed `max_cells`.
///
/// When truncation happens, appends a single-cell `…`. For `max_cells == 0`
/// returns the empty string; for `max_cells == 1` returns the ellipsis
/// only. Zero-width and combining characters are counted as zero cells.
pub fn truncate_to_width(s: &str, max_cells: usize) -> String {
    if width(s) <= max_cells {
        return s.to_string();
    }
    if max_cells == 0 {
        return String::new();
    }
    if max_cells == 1 {
        return "…".to_string();
    }
    let budget = max_cells - 1; // room for the trailing …
    let mut out = String::new();
    let mut used = 0usize;
    for ch in s.chars() {
        let w = UnicodeWidthChar::width(ch).unwrap_or(0);
        if used + w > budget {
            break;
        }
        out.push(ch);
        used += w;
    }
    out.push('…');
    out
}

/// Pads `s` on the right with ASCII spaces so the result renders in
/// exactly `target_cells` cells.
///
/// When `s` is wider than the target, falls back to `truncate_to_width`,
/// which still produces a result of exactly `target_cells` cells.
pub fn pad_to_width(s: &str, target_cells: usize) -> String {
    let w = width(s);
    if w == target_cells {
        return s.to_string();
    }
    if w > target_cells {
        return truncate_to_width(s, target_cells);
    }
    let mut out = String::with_capacity(s.len() + (target_cells - w));
    out.push_str(s);
    for _ in 0..(target_cells - w) {
        out.push(' ');
    }
    out
}

/// Word-wraps `s` so each returned line renders in at most `max_cells`
/// cells.
///
/// Prefers breaking at ASCII spaces; falls back to a hard char-boundary
/// break when no space fits. Always returns at least one element so
/// per-field line-count math stays consistent.
pub fn wrap_to_width(s: &str, max_cells: usize) -> Vec<String> {
    if max_cells == 0 {
        return vec![String::new()];
    }
    if s.is_empty() {
        return vec![String::new()];
    }
    if width(s) <= max_cells {
        return vec![s.to_string()];
    }

    let mut out: Vec<String> = Vec::new();
    let mut cur = String::new();
    let mut cur_w = 0usize;
    // Byte index in `cur` of the most recent space, used as the preferred
    // break point.
    let mut last_space: Option<usize> = None;

    for ch in s.chars() {
        let cw = UnicodeWidthChar::width(ch).unwrap_or(0);
        if cur_w + cw > max_cells {
            // Prefer breaking at the most recent space.
            if let Some(sp) = last_space {
                // Push everything up to the space (excluding it); carry the rest.
                let carry: String = cur[sp + 1..].to_string();
                cur.truncate(sp);
                out.push(std::mem::take(&mut cur));
                cur = carry;
                cur_w = width(&cur);
                last_space = None;
            } else {
                // No space available; hard-break mid-word.
                out.push(std::mem::take(&mut cur));
                cur_w = 0;
                last_space = None;
            }
        }
        if ch == ' ' {
            last_space = Some(cur.len());
        }
        cur.push(ch);
        cur_w += cw;
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    if out.is_empty() {
        out.push(String::new());
    }
    out
}
