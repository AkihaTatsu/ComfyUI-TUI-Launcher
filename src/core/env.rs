//! Environment variable construction for spawned child processes.
//!
//! Translates the launcher's `[network]` configuration into the env vars
//! that pip, git, and Huggingface tooling honour, without ever touching
//! user-owned config files.

use crate::core::config::Network;
use std::collections::HashMap;

/// Builds the environment variables to inject into child processes based on
/// the supplied `Network` settings.
///
/// The mirror and acceleration fields are free-form strings, semicolon-
/// separated when multiple values are supplied. An empty field means no
/// override.
pub fn build(n: &Network) -> HashMap<String, String> {
    let mut e: HashMap<String, String> = HashMap::new();

    // ── PyPI ───────────────────────────────────────────────────────────
    // First entry → PIP_INDEX_URL; remaining → PIP_EXTRA_INDEX_URL
    // (space-separated, the format pip accepts).
    let pypi: Vec<&str> = split_list(&n.pypi_mirror).collect();
    if let Some(first) = pypi.first() {
        e.insert("PIP_INDEX_URL".into(), first.to_string());
        if pypi.len() > 1 {
            e.insert("PIP_EXTRA_INDEX_URL".into(), pypi[1..].join(" "));
        }
    }

    // ── Huggingface ────────────────────────────────────────────────────
    // HF honours one endpoint only — use the first item.
    if let Some(first) = split_list(&n.hf_mirror).next() {
        e.insert("HF_ENDPOINT".into(), first.to_string());
    }

    // ── Git insteadOf rules ────────────────────────────────────────────
    // Each `;`-item is `<mirror>=<original>`; a bare URL defaults to
    // substituting GitHub. Injected via `GIT_CONFIG_COUNT` /
    // `GIT_CONFIG_KEY_<n>` / `GIT_CONFIG_VALUE_<n>` so spawned git
    // commands inherit the rules without touching user config files.
    let rules: Vec<(String, String)> = split_list(&n.git_mirror)
        .filter_map(|item| {
            let item = item.trim();
            if item.is_empty() {
                return None;
            }
            if let Some((mirror, original)) = item.split_once('=') {
                Some((mirror.trim().to_string(), original.trim().to_string()))
            } else {
                Some((item.to_string(), "https://github.com/".to_string()))
            }
        })
        .collect();
    if !rules.is_empty() {
        e.insert("GIT_CONFIG_COUNT".into(), rules.len().to_string());
        for (i, (mirror, original)) in rules.iter().enumerate() {
            e.insert(
                format!("GIT_CONFIG_KEY_{i}"),
                format!("url.{mirror}.insteadOf"),
            );
            e.insert(format!("GIT_CONFIG_VALUE_{i}"), original.clone());
        }
    }

    // ── GitHub acceleration ────────────────────────────────────────────
    // First item only — kept as a hint env var for tools that honour it.
    if let Some(first) = split_list(&n.github_accel).next() {
        e.insert("GH_ACCEL".into(), first.to_string());
    }

    // ── Proxies (verbatim string fields, unchanged) ────────────────────
    if !n.http_proxy.is_empty() {
        e.insert("HTTP_PROXY".into(), n.http_proxy.clone());
        e.insert("http_proxy".into(), n.http_proxy.clone());
    }
    if !n.https_proxy.is_empty() {
        e.insert("HTTPS_PROXY".into(), n.https_proxy.clone());
        e.insert("https_proxy".into(), n.https_proxy.clone());
    }
    if !n.no_proxy.is_empty() {
        e.insert("NO_PROXY".into(), n.no_proxy.clone());
        e.insert("no_proxy".into(), n.no_proxy.clone());
    }
    e
}

/// Splits a `;`-separated string into trimmed non-empty items.
fn split_list(s: &str) -> impl Iterator<Item = &str> {
    s.split(';').map(str::trim).filter(|t| !t.is_empty())
}

/// Returns the canonical wire form of a semicolon-separated list.
///
/// Each item is trimmed, empty items are dropped, and the remainder is
/// rejoined with a single `;` and no surrounding spaces. Applied on both
/// save and load so the on-disk file always uses the tight `a;b;c` form.
pub fn normalize_semicolon_list(s: &str) -> String {
    split_list(s).collect::<Vec<&str>>().join(";")
}
