//! Read-only inspection of an on-disk ComfyUI installation.

use crate::core::paths::ComfyDirs;
use std::path::{Path, PathBuf};

/// Returns the ComfyUI version string declared in `<root>/comfyui_version.py`.
///
/// Returns `None` if the file is missing or the `__version__` line cannot be
/// parsed.
pub fn comfyui_version(root: &Path) -> Option<String> {
    let text = std::fs::read_to_string(root.join("comfyui_version.py")).ok()?;
    for line in text.lines() {
        let line = line.trim();
        if let Some(rhs) = line.strip_prefix("__version__") {
            // Trim spaces, the `=`, more spaces, then the surrounding quotes.
            let rhs = rhs.trim_start().trim_start_matches('=').trim();
            let trimmed = rhs.trim_matches(|c: char| c == '"' || c == '\'');
            if !trimmed.is_empty() {
                return Some(trimmed.to_string());
            }
        }
    }
    None
}

/// Returns `(enabled, disabled)` counts of loadable custom-node directories
/// under `<root>/custom_nodes/`.
///
/// A directory counts when it contains an `__init__.py` and its name does
/// not start with `.` or `__`. A trailing `.disabled` marks it as disabled.
pub fn count_custom_nodes(root: &Path) -> (usize, usize) {
    let dir = ComfyDirs::new(root).custom_nodes();
    let mut enabled = 0usize;
    let mut disabled = 0usize;
    let rd = match std::fs::read_dir(&dir) {
        Ok(r) => r,
        Err(_) => return (0, 0),
    };
    for ent in rd.flatten() {
        let p = ent.path();
        if !p.is_dir() {
            continue;
        }
        let raw = p
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_string();
        let is_disabled = raw.ends_with(".disabled");
        let logical = raw.strip_suffix(".disabled").unwrap_or(&raw);
        if logical.starts_with('.') || logical.starts_with("__") {
            continue;
        }
        if !p.join("__init__.py").is_file() {
            continue;
        }
        if is_disabled {
            disabled += 1;
        } else {
            enabled += 1;
        }
    }
    (enabled, disabled)
}

/// Parses `<root>/extra_model_paths.yaml` and returns
/// `(group, category, absolute_path)` triples, one per category and per path.
///
/// Handles top-level groups (`name:` at column 0), nested key/value pairs at
/// any indent, and `|` block scalars whose lines are at a deeper indent than
/// the key. Lines beginning with `#` are treated as comments.
pub fn extra_model_paths(root: &Path) -> Vec<(String, String, PathBuf)> {
    let path = root.join("extra_model_paths.yaml");
    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(_) => return Vec::new(),
    };

    // First pass: collect groups → key → Vec<value strings>. The current
    // group is tracked by index to avoid borrow-checker conflicts.
    #[allow(clippy::type_complexity)]
    let mut groups: Vec<(String, Vec<(String, Vec<String>)>)> = Vec::new();
    let mut cur_group_idx: Option<usize> = None;
    // (key, key_indent) — once we see a line indented deeper, lines belong to it.
    let mut pending_block: Option<(String, usize)> = None;

    for raw in text.lines() {
        let line_no_comment = strip_comment(raw);
        // Determine indent
        let indent = line_no_comment.chars().take_while(|c| *c == ' ').count();
        let stripped = line_no_comment.trim();
        if stripped.is_empty() {
            continue;
        }

        // Continuation of a `|` block scalar.
        if let Some((k, key_indent)) = &pending_block {
            if indent > *key_indent {
                if let Some(gi) = cur_group_idx {
                    let entries = &mut groups[gi].1;
                    if let Some((_, vs)) = entries.iter_mut().find(|(kk, _)| kk == k) {
                        vs.push(stripped.to_string());
                    }
                }
                continue;
            } else {
                pending_block = None;
            }
        }

        // Top-level group: `name:` at column 0, value side is empty.
        if indent == 0 {
            if let Some(name) = stripped.strip_suffix(':') {
                let name = name.trim();
                if !name.is_empty() {
                    groups.push((name.to_string(), Vec::new()));
                    cur_group_idx = Some(groups.len() - 1);
                    continue;
                }
            }
            // bare non-group top-level line — ignore.
            continue;
        }

        // Key/value at deeper indent — belongs to current group.
        let Some(gi) = cur_group_idx else {
            continue;
        };
        // Split key:value
        let Some(colon) = stripped.find(':') else {
            continue;
        };
        let (k, v) = stripped.split_at(colon);
        let k = k.trim().to_string();
        let v = v[1..].trim().to_string();
        if v == "|" {
            // Block scalar — value lines follow at deeper indent.
            groups[gi].1.push((k.clone(), Vec::new()));
            pending_block = Some((k, indent));
        } else {
            groups[gi].1.push((k, vec![v]));
        }
    }

    // Second pass — resolve `base_path` per group, emit triples.
    let mut out: Vec<(String, String, PathBuf)> = Vec::new();
    for (group, entries) in &groups {
        let base = entries
            .iter()
            .find(|(k, _)| k == "base_path")
            .and_then(|(_, vs)| vs.first().cloned())
            .map(PathBuf::from);
        for (k, vs) in entries {
            if k == "base_path" || k == "is_default" {
                continue;
            }
            for raw_path in vs {
                if raw_path.is_empty() {
                    continue;
                }
                let p = PathBuf::from(raw_path);
                let abs = if p.is_absolute() {
                    p
                } else if let Some(b) = &base {
                    join_resolved(b, &p, root)
                } else {
                    root.join(&p)
                };
                out.push((group.clone(), k.clone(), abs));
            }
        }
    }
    out
}

/// Resolves `rel` against `base`, falling back to `root` when `base` is
/// itself a relative path.
fn join_resolved(base: &Path, rel: &Path, root: &Path) -> PathBuf {
    let abs_base = if base.is_absolute() {
        base.to_path_buf()
    } else {
        root.join(base)
    };
    abs_base.join(rel)
}

fn strip_comment(line: &str) -> String {
    // `#` outside a value starts a comment. Quoted `#` is not handled because
    // model paths do not contain `#`.
    if let Some(i) = line.find('#') {
        line[..i].to_string()
    } else {
        line.to_string()
    }
}
