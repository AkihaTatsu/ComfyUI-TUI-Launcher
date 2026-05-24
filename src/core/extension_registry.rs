//! Online catalog of installable ComfyUI extensions.
//!
//! Fetches `custom-node-list.json` from ComfyUI-Manager, caches it on disk,
//! and reports install state against the user's `custom_nodes` directory.

use crate::core::paths;
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;

/// URL of the official extension catalog.
pub const REGISTRY_URL: &str =
    "https://raw.githubusercontent.com/ltdrdata/ComfyUI-Manager/main/custom-node-list.json";

/// One entry in the extension catalog.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegistryEntry {
    /// Display title.
    pub title: String,
    /// Author name as reported by the catalog.
    pub author: String,
    /// Git URL used to install the extension.
    pub reference: String,
    /// Short description.
    pub description: String,
}

/// Installation state of a catalog entry on the local machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstallStatus {
    /// Not present in `custom_nodes`.
    NotInstalled,
    /// Installed and enabled.
    Installed,
    /// Installed but disabled (folder suffixed with `.disabled`).
    Disabled,
}

/// Returns the on-disk path of the cached catalog JSON.
pub fn cache_path() -> PathBuf {
    paths::cache_dir().join("extensions_cache.json")
}

/// Loads the cached catalog if it exists and parses cleanly.
pub fn load_cache() -> Option<Vec<RegistryEntry>> {
    let text = std::fs::read_to_string(cache_path()).ok()?;
    serde_json::from_str(&text).ok()
}

/// Writes the catalog cache to disk on a best-effort basis.
pub fn save_cache(items: &[RegistryEntry]) {
    if let Ok(text) = serde_json::to_string(items) {
        let _ = paths::ensure_cache_dir();
        let _ = std::fs::write(cache_path(), text);
    }
}

/// Fetches the catalog over HTTPS, honouring proxy variables from `env_vars`.
pub fn fetch_blocking(env_vars: &HashMap<String, String>) -> Result<Vec<RegistryEntry>> {
    let mut builder = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(20))
        .timeout_read(Duration::from_secs(60));
    if let Some(p) = env_vars
        .get("HTTPS_PROXY")
        .or_else(|| env_vars.get("HTTP_PROXY"))
        .or_else(|| env_vars.get("https_proxy"))
        .or_else(|| env_vars.get("http_proxy"))
    {
        if let Ok(proxy) = ureq::Proxy::new(p) {
            builder = builder.proxy(proxy);
        }
    }
    let agent = builder.build();
    let body: String = agent.get(REGISTRY_URL).call()?.into_string()?;
    let raw: serde_json::Value = serde_json::from_str(&body)?;
    let arr = raw["custom_nodes"].as_array().cloned().unwrap_or_default();
    let mut out: Vec<RegistryEntry> = arr
        .into_iter()
        .filter_map(|v| {
            let title = v["title"].as_str().unwrap_or("").trim().to_string();
            let reference = v["reference"].as_str().unwrap_or("").trim().to_string();
            if title.is_empty() || reference.is_empty() {
                return None;
            }
            Some(RegistryEntry {
                title,
                author: v["author"].as_str().unwrap_or("").trim().to_string(),
                reference,
                description: v["description"].as_str().unwrap_or("").trim().to_string(),
            })
        })
        .collect();
    out.sort_by_key(|a| a.title.to_lowercase());
    Ok(out)
}

/// Normalises a git URL for comparison: lowercase, drop scheme, strip
/// trailing `.git`, and strip trailing `/`.
fn normalize_url(s: &str) -> String {
    let s = s.trim().to_lowercase();
    let s = s
        .strip_prefix("https://")
        .or_else(|| s.strip_prefix("http://"))
        .map(str::to_string)
        .unwrap_or(s);
    let s = s.strip_suffix('/').map(str::to_string).unwrap_or(s);
    let s = s.strip_suffix(".git").map(str::to_string).unwrap_or(s);
    s
}

/// Returns the install status of a registry entry against the list of
/// installed extensions currently known to the Extensions tab.
pub fn status_for(
    entry: &RegistryEntry,
    installed: &[crate::screens::version_mgmt::extensions_tab::Extension],
) -> InstallStatus {
    let want = normalize_url(&entry.reference);
    for ext in installed {
        if !ext.managed {
            continue;
        }
        if normalize_url(&ext.remote) == want {
            return if ext.disabled {
                InstallStatus::Disabled
            } else {
                InstallStatus::Installed
            };
        }
    }
    InstallStatus::NotInstalled
}
