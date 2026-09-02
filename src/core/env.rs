//! Environment variable construction for spawned child processes.
//!
//! Translates the launcher's `[network]` configuration into the env vars
//! that pip, git, and Huggingface tooling honour, without ever touching
//! user-owned config files.

use crate::core::config::Network;
use std::collections::HashMap;

/// Environment variable read by the in-memory Huggingface URL rewrite wrapper.
pub const HF_FORCE_MIRROR_ENV: &str = "COMFYUI_TUI_HF_MIRROR";

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
    // HF honours one endpoint only — use the first valid item.
    if let Some(endpoint) = normalize_hf_endpoint(&n.hf_mirror) {
        e.insert("HF_ENDPOINT".into(), endpoint.clone());
        if n.hf_force_mirror {
            e.insert(HF_FORCE_MIRROR_ENV.into(), endpoint);
        }
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

/// Returns a canonical Huggingface endpoint, or `None` when unset/invalid.
pub fn normalize_hf_endpoint(s: &str) -> Option<String> {
    let mut raw = split_list(s).next()?.replace('\\', "/");
    raw = raw.trim().to_string();
    if raw.is_empty() {
        return None;
    }
    for scheme in ["https", "http"] {
        let single_slash = format!("{scheme}:/");
        let double_slash = format!("{scheme}://");
        if raw.starts_with(&single_slash) && !raw.starts_with(&double_slash) {
            raw = format!(
                "{double_slash}{}",
                raw[single_slash.len()..].trim_start_matches('/')
            );
            break;
        }
    }
    if !raw.contains("://") {
        raw = format!("https://{raw}");
    }
    while raw.ends_with('/') {
        raw.pop();
    }

    if !has_valid_http_url_shape(&raw) {
        return None;
    }
    Some(raw)
}

fn has_valid_http_url_shape(url: &str) -> bool {
    let Some((scheme, rest)) = url.split_once("://") else {
        return false;
    };
    if !matches!(scheme, "http" | "https") || rest.is_empty() {
        return false;
    }
    let host_port = rest
        .split(['/', '?', '#'])
        .next()
        .unwrap_or("")
        .trim_matches('/');
    if host_port.is_empty() || host_port.contains('@') || host_port.contains(char::is_whitespace) {
        return false;
    }
    let host = host_port.split(':').next().unwrap_or("");
    !host.is_empty() && host.contains('.') && !host.starts_with('.') && !host.ends_with('.')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_hf_endpoint() {
        assert_eq!(
            normalize_hf_endpoint("hf-mirror.com/"),
            Some("https://hf-mirror.com".into())
        );
        assert_eq!(
            normalize_hf_endpoint(" https://hf-mirror.com/// "),
            Some("https://hf-mirror.com".into())
        );
        assert_eq!(
            normalize_hf_endpoint("https:\\hf-mirror.com\\"),
            Some("https://hf-mirror.com".into())
        );
        assert_eq!(
            normalize_hf_endpoint("https://a.example/;https://b.example/"),
            Some("https://a.example".into())
        );
    }

    #[test]
    fn rejects_invalid_hf_endpoint() {
        for bad in [
            "",
            "   ",
            "ftp://hf-mirror.com",
            "https://",
            "https://local host",
        ] {
            assert_eq!(normalize_hf_endpoint(bad), None);
        }
    }

    #[test]
    fn force_mirror_requires_valid_endpoint() {
        let mut n = Network {
            hf_force_mirror: true,
            ..Network::default()
        };
        assert!(!build(&n).contains_key("HF_ENDPOINT"));
        assert!(!build(&n).contains_key(HF_FORCE_MIRROR_ENV));

        n.hf_mirror = "hf-mirror.com/".into();
        let env = build(&n);
        assert_eq!(
            env.get("HF_ENDPOINT"),
            Some(&"https://hf-mirror.com".into())
        );
        assert_eq!(
            env.get(HF_FORCE_MIRROR_ENV),
            Some(&"https://hf-mirror.com".into())
        );

        n.hf_force_mirror = false;
        let env = build(&n);
        assert_eq!(
            env.get("HF_ENDPOINT"),
            Some(&"https://hf-mirror.com".into())
        );
        assert!(!env.contains_key(HF_FORCE_MIRROR_ENV));
    }
}
