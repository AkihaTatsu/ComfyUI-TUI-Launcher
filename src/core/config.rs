//! Persistent launcher and ComfyUI configuration files.
//!
//! Two files live in the launcher config directory: `launcher_config.toml`
//! holds launcher-owned settings (general preferences plus network mirrors),
//! and `comfyui_config.toml` holds the user's overrides for the ComfyUI
//! settings schema as a flat top-level map.

use crate::core::paths;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;
use toml::Value;

/// Upper bound for user-configured Git workers inside one batch task.
pub const MAX_GIT_CONCURRENCY: usize = 32;

/// What the launcher should do after ComfyUI stops normally.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum NormalExitAction {
    /// End the launcher process.
    #[default]
    Exit,
    /// Start a fresh launcher TUI session.
    ReturnToLauncher,
}

impl NormalExitAction {
    /// Stable value persisted in `launcher_config.toml`.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Exit => "exit",
            Self::ReturnToLauncher => "return_to_launcher",
        }
    }

    /// Parses a persisted settings value.
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "exit" => Some(Self::Exit),
            "return_to_launcher" => Some(Self::ReturnToLauncher),
            _ => None,
        }
    }
}

impl<'de> Deserialize<'de> for NormalExitAction {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Ok(Self::parse(&value).unwrap_or_default())
    }
}

/// What the launcher should do after ComfyUI crashes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum CrashExitAction {
    /// End the launcher process.
    #[default]
    Exit,
    /// Start a fresh launcher TUI session.
    ReturnToLauncher,
    /// Relaunch ComfyUI, subject to the configured crash-loop limits.
    AutoRestart,
}

impl CrashExitAction {
    /// Stable value persisted in `launcher_config.toml`.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Exit => "exit",
            Self::ReturnToLauncher => "return_to_launcher",
            Self::AutoRestart => "auto_restart",
        }
    }

    /// Parses a persisted settings value.
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "exit" => Some(Self::Exit),
            "return_to_launcher" => Some(Self::ReturnToLauncher),
            "auto_restart" => Some(Self::AutoRestart),
            _ => None,
        }
    }
}

impl<'de> Deserialize<'de> for CrashExitAction {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Ok(Self::parse(&value).unwrap_or_default())
    }
}

// Defaults for `launcher_config.toml` mirror the entries in
// `launcher_schema.toml`. On first run the file is instantiated via serde's
// `Default` impls so the two stay in sync.

/// General launcher preferences stored in `launcher_config.toml`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct General {
    /// Absolute path to the ComfyUI installation root.
    #[serde(default)]
    pub comfyui_dir: String,
    /// Absolute path to the Python interpreter used to launch ComfyUI.
    #[serde(default)]
    pub python: String,
    /// Active UI language code.
    #[serde(default = "default_lang")]
    pub language: String,
    /// UI mode (`advanced` or `simple`).
    #[serde(default = "default_mode")]
    pub mode: String,
    /// Maximum number of repositories processed concurrently by Git-heavy
    /// extension scans and bulk operations.
    #[serde(default = "default_git_concurrency")]
    pub git_concurrency: usize,
    /// Action taken after a clean exit or an intentional stop such as Ctrl+C.
    #[serde(default)]
    pub normal_exit_action: NormalExitAction,
    /// Action taken after a non-zero exit, fatal signal, or launch failure.
    #[serde(default)]
    pub crash_exit_action: CrashExitAction,
    /// Seconds to wait before each restart (only used when
    /// `crash_exit_action` is `auto_restart`).
    #[serde(default = "default_restart_delay")]
    pub crash_restart_delay_secs: u64,
    /// Sliding window, in seconds, over which crashes are counted for
    /// crash-loop protection (only used for `auto_restart`).
    #[serde(default = "default_restart_window")]
    pub crash_restart_window_secs: u64,
    /// If the number of crashes within `crash_restart_window_secs` exceeds
    /// this, the supervisor gives up instead of restarting (only used for
    /// `auto_restart`).
    #[serde(default = "default_restart_max_fails")]
    pub crash_restart_max_fails: u32,
}
// Empty by default. The canonicalisation step in `load_or_init` replaces
// empty or unknown values with the first available locale so the locale
// list stays driven by `assets/i18n/`.
fn default_lang() -> String {
    String::new()
}
fn default_mode() -> String {
    "advanced".into()
}
fn default_git_concurrency() -> usize {
    1
}

/// Applies the supported range to persisted or programmatically supplied
/// concurrency values.
pub fn normalize_git_concurrency(value: usize) -> usize {
    value.clamp(1, MAX_GIT_CONCURRENCY)
}
fn default_restart_delay() -> u64 {
    2
}
fn default_restart_window() -> u64 {
    60
}
fn default_restart_max_fails() -> u32 {
    5
}

// Manual `Default` so the numeric crash-restart fields fall back to the same
// values as their serde `default` fns (a derived `Default` would zero them,
// which would be written verbatim into a fresh `launcher_config.toml`).
impl Default for General {
    fn default() -> Self {
        Self {
            comfyui_dir: String::new(),
            python: String::new(),
            language: default_lang(),
            mode: default_mode(),
            git_concurrency: default_git_concurrency(),
            normal_exit_action: NormalExitAction::default(),
            crash_exit_action: CrashExitAction::default(),
            crash_restart_delay_secs: default_restart_delay(),
            crash_restart_window_secs: default_restart_window(),
            crash_restart_max_fails: default_restart_max_fails(),
        }
    }
}

/// Network mirror, proxy, and acceleration settings stored in
/// `launcher_config.toml`.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Network {
    /// PyPI index URL(s), semicolon-separated when multiple are supplied.
    ///
    /// The first item goes to `PIP_INDEX_URL`; the rest go to
    /// `PIP_EXTRA_INDEX_URL`. Empty leaves pip's default behaviour.
    #[serde(default, deserialize_with = "de_pypi_mirror")]
    pub pypi_mirror: String,
    /// Git URL `insteadOf` rules, semicolon-separated.
    ///
    /// Each rule is `<mirror>=<original>`; a bare URL is shorthand for
    /// `<bare>=https://github.com/`. The rules are injected via
    /// `GIT_CONFIG_*` env vars so spawned git commands substitute
    /// transparently.
    #[serde(default, deserialize_with = "de_git_mirror")]
    pub git_mirror: String,
    /// Huggingface endpoint URL.
    ///
    /// The first semicolon-separated item wins because Huggingface honours
    /// only one `HF_ENDPOINT`.
    #[serde(default, deserialize_with = "de_hf_mirror")]
    pub hf_mirror: String,
    /// When enabled, ComfyUI launches through an in-memory Python wrapper
    /// that rewrites Huggingface URLs to the configured mirror.
    #[serde(default)]
    pub hf_force_mirror: bool,
    /// GitHub acceleration proxy URL prefix(es), semicolon-separated.
    ///
    /// The first item is exported as `GH_ACCEL`.
    #[serde(default, deserialize_with = "de_github_accel")]
    pub github_accel: String,
    /// Value for `HTTP_PROXY` / `http_proxy`.
    #[serde(default)]
    pub http_proxy: String,
    /// Value for `HTTPS_PROXY` / `https_proxy`.
    #[serde(default)]
    pub https_proxy: String,
    /// Value for `NO_PROXY` / `no_proxy`.
    #[serde(default)]
    pub no_proxy: String,
}

// Legacy bool → URL migration. Older releases stored these four fields as
// bools; the deserialisers accept either shape: a `true` legacy value
// becomes the previously baked-in default URL; `false` becomes empty.
fn de_pypi_mirror<'de, D: serde::Deserializer<'de>>(d: D) -> Result<String, D::Error> {
    legacy_or_str(d, "https://pypi.tuna.tsinghua.edu.cn/simple")
}
fn de_hf_mirror<'de, D: serde::Deserializer<'de>>(d: D) -> Result<String, D::Error> {
    legacy_or_str(d, "https://hf-mirror.com")
}
fn de_git_mirror<'de, D: serde::Deserializer<'de>>(d: D) -> Result<String, D::Error> {
    legacy_or_str(d, "")
}
fn de_github_accel<'de, D: serde::Deserializer<'de>>(d: D) -> Result<String, D::Error> {
    legacy_or_str(d, "")
}
fn legacy_or_str<'de, D: serde::Deserializer<'de>>(
    d: D,
    on_true: &str,
) -> Result<String, D::Error> {
    let raw = match toml::Value::deserialize(d)? {
        toml::Value::String(s) => s,
        toml::Value::Boolean(true) => on_true.to_string(),
        toml::Value::Boolean(false) => String::new(),
        _ => String::new(),
    };
    // Canonicalise to `a;b;c` form so user-typed spaces do not leak through
    // subsequent save and parse cycles.
    Ok(crate::core::env::normalize_semicolon_list(&raw))
}

/// On-disk shape of `launcher_config.toml`.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct LauncherFile {
    #[serde(default)]
    general: General,
    #[serde(default)]
    network: Network,
    // Older versions stored ComfyUI settings here under `[comfy_settings]`.
    // `load_or_init` migrates them out to `comfyui_config.toml` and clears
    // this slot.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    comfy_settings: Option<BTreeMap<String, Value>>,
}

/// In-memory merged view of both configuration files.
#[derive(Debug, Clone, Default)]
pub struct Config {
    /// Launcher general preferences.
    pub general: General,
    /// Launcher network settings.
    pub network: Network,
    /// User overrides for the ComfyUI settings schema.
    pub comfy_settings: BTreeMap<String, Value>,
    /// Path to `launcher_config.toml`. The ComfyUI overrides always live at
    /// `paths::comfyui_config_file()`.
    pub path: PathBuf,
}

impl Config {
    /// Loads both configuration files, creating them on first run and
    /// applying any one-time migrations.
    pub fn load_or_init() -> Result<Self> {
        paths::ensure_config_dir().ok();
        paths::migrate_legacy_filenames();

        // launcher_config.toml: `[general]` + `[network]`. On first run,
        // instantiate via serde defaults and persist a fresh file.
        let lpath = paths::config_file();
        let mut lfile: LauncherFile = if lpath.exists() {
            let ltext = std::fs::read_to_string(&lpath).context("read launcher config")?;
            let mut parsed: LauncherFile =
                toml::from_str(&ltext).context("parse launcher config")?;
            migrate_legacy_exit_action(&ltext, &mut parsed);
            parsed
        } else {
            let fresh = LauncherFile::default();
            let _ = std::fs::write(&lpath, toml::to_string_pretty(&fresh).unwrap_or_default());
            fresh
        };

        // Canonicalise the language code. If the stored value matches no
        // available locale (legacy codes or typos), translate common
        // aliases and otherwise fall back to the first available locale so
        // the picker renders the real native name.
        let available: Vec<String> = crate::core::i18n::available_locales()
            .into_iter()
            .map(|(c, _)| c)
            .collect();
        if !available.iter().any(|c| c == &lfile.general.language) {
            // Try common legacy aliases before giving up. If the alias
            // does not exist either, fall through to the first available
            // locale.
            let aliased = match lfile.general.language.as_str() {
                "en" | "en_US" => Some("en-US"),
                "zh" | "zh_CN" => Some("zh-CN"),
                "en_UK" | "en-GB" | "en_GB" => Some("en-UK"),
                _ => None,
            };
            lfile.general.language = aliased
                .filter(|c| available.iter().any(|a| a == *c))
                .map(|s| s.to_string())
                .or_else(|| available.first().cloned())
                .unwrap_or_default();
        }
        lfile.general.git_concurrency = normalize_git_concurrency(lfile.general.git_concurrency);
        // Persist the canonical form so the file matches the in-memory
        // value next time it is read. Best-effort.
        {
            let normalised = toml::to_string_pretty(&lfile).unwrap_or_default();
            let _ = std::fs::write(&lpath, normalised);
        };

        // comfyui_config.toml: flat top-level `key = value`.
        let cpath = paths::comfyui_config_file();
        let mut comfy: BTreeMap<String, Value> = if cpath.exists() {
            let t = std::fs::read_to_string(&cpath).context("read comfyui config")?;
            toml::from_str(&t).context("parse comfyui config")?
        } else {
            BTreeMap::new()
        };

        // One-time migration: lift legacy `[comfy_settings]` out of the
        // launcher file into the dedicated ComfyUI file.
        if let Some(legacy) = lfile.comfy_settings.take() {
            if !legacy.is_empty() && comfy.is_empty() {
                comfy = legacy;
                // Persist the move immediately so older launchers do not
                // rewrite it on next save.
                let _ = std::fs::write(&cpath, toml::to_string_pretty(&comfy).unwrap_or_default());
            }
            // Rewrite launcher_config.toml without the legacy section.
            let _ = std::fs::write(&lpath, toml::to_string_pretty(&lfile).unwrap_or_default());
        }

        Ok(Self {
            general: lfile.general,
            network: lfile.network,
            comfy_settings: comfy,
            path: lpath,
        })
    }

    /// Persists both configuration files to disk.
    pub fn save(&self) -> Result<()> {
        // launcher_config.toml — values only, no comfy bag.
        let lfile = LauncherFile {
            general: self.general.clone(),
            network: self.network.clone(),
            comfy_settings: None,
        };
        std::fs::write(&self.path, toml::to_string_pretty(&lfile)?)?;
        // comfyui_config.toml — flat user values only.
        let cpath = paths::comfyui_config_file();
        std::fs::write(&cpath, toml::to_string_pretty(&self.comfy_settings)?)?;
        Ok(())
    }

    /// Sets one ComfyUI setting and persists the change.
    pub fn set_comfy(&mut self, key: &str, v: Value) {
        self.comfy_settings.insert(key.to_string(), v);
        let _ = self.save();
    }

    /// Clears every ComfyUI setting override and persists the change.
    pub fn reset_comfy_to_factory(&mut self) {
        self.comfy_settings.clear();
        let _ = self.save();
    }

    /// Returns the current value for a ComfyUI setting, if one was stored.
    pub fn get_comfy<'a>(&'a self, key: &str) -> Option<&'a Value> {
        self.comfy_settings.get(key)
    }
}

/// Migrates the former crash-only boolean without overriding an explicitly
/// configured replacement field. The next canonical save drops the unknown
/// legacy key from the serialized file.
fn migrate_legacy_exit_action(text: &str, file: &mut LauncherFile) {
    let Ok(raw) = toml::from_str::<Value>(text) else {
        return;
    };
    let Some(general) = raw.get("general").and_then(Value::as_table) else {
        return;
    };
    if general.contains_key("crash_exit_action") {
        return;
    }
    if general
        .get("crash_auto_restart")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        file.general.crash_exit_action = CrashExitAction::AutoRestart;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_general_config_defaults_git_concurrency_to_one() {
        let file: LauncherFile = toml::from_str("[general]\nmode = 'advanced'\n")
            .expect("legacy launcher config should parse");
        assert_eq!(file.general.git_concurrency, 1);
        assert_eq!(file.general.normal_exit_action, NormalExitAction::Exit);
        assert_eq!(file.general.crash_exit_action, CrashExitAction::Exit);
    }

    #[test]
    fn git_concurrency_is_bounded() {
        assert_eq!(normalize_git_concurrency(0), 1);
        assert_eq!(normalize_git_concurrency(1), 1);
        assert_eq!(normalize_git_concurrency(MAX_GIT_CONCURRENCY + 10), 32);
    }

    #[test]
    fn legacy_crash_restart_true_migrates_to_auto_restart() {
        let text = "[general]\ncrash_auto_restart = true\n";
        let mut file: LauncherFile = toml::from_str(text).expect("legacy config should parse");
        migrate_legacy_exit_action(text, &mut file);
        assert_eq!(file.general.crash_exit_action, CrashExitAction::AutoRestart);
        assert!(!toml::to_string(&file)
            .expect("migrated config should serialize")
            .contains("crash_auto_restart"));
    }

    #[test]
    fn explicit_crash_action_wins_over_legacy_toggle() {
        let text =
            "[general]\ncrash_auto_restart = true\ncrash_exit_action = 'return_to_launcher'\n";
        let mut file: LauncherFile = toml::from_str(text).expect("mixed config should parse");
        migrate_legacy_exit_action(text, &mut file);
        assert_eq!(
            file.general.crash_exit_action,
            CrashExitAction::ReturnToLauncher
        );
    }
}
