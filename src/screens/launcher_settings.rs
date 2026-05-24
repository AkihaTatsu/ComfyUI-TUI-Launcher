//! Launcher general-preferences editor screen.

use super::settings_view::SettingsView;
use crate::core::config::Config;
use crate::core::paths::ComfyDirs;
use crate::core::schema::{self, Schema};
use crate::core::{i18n, python};
use crossterm::event::KeyCode;
use ratatui::layout::Rect;
use ratatui::Frame;
use toml::Value;

/// Thin wrapper around `SettingsView` driven by the launcher schema.
///
/// Field metadata (names, descriptions, choice options) lives entirely in
/// the schema. Only the value getter and setter live in this file.
pub struct LauncherSettings {
    /// Underlying settings editor.
    pub view: SettingsView,
    schema: Schema,
}

impl LauncherSettings {
    /// Constructs the screen, falling back to the embedded default schema
    /// when the on-disk schema cannot be parsed.
    pub fn new() -> Self {
        let schema = schema::load_launcher_or_init().unwrap_or_else(|_| {
            toml::from_str(schema::DEFAULT_LAUNCHER_SCHEMA).unwrap_or(Schema { tabs: vec![] })
        });
        Self {
            view: SettingsView::new(),
            schema,
        }
    }

    /// Forwards Esc handling to the underlying editor.
    pub fn eat_esc(&mut self) -> bool {
        self.view.eat_esc()
    }

    /// Drains and returns the most recent flash message, if any.
    pub fn take_flash(&mut self) -> Option<(crate::app::FlashKind, String)> {
        self.view.take_flash()
    }

    /// Renders the editor into `area`.
    pub fn render(&self, f: &mut Frame, area: Rect, cfg: &Config, body_active: bool) {
        self.view
            .render(f, area, &self.schema, cfg, get, false, body_active);
    }

    /// Handles a key event.
    pub fn on_key(&mut self, code: KeyCode, cfg: &mut Config) {
        let schema = self.schema.clone();
        self.view.on_key(code, &schema, cfg, get, set);
    }

    /// Handles a mouse event.
    pub fn on_mouse(&mut self, m: crossterm::event::MouseEvent, area: Rect, cfg: &mut Config) {
        let schema = self.schema.clone();
        self.view.on_mouse(m, area, &schema, cfg, get, set, false);
    }

    /// Forwards a wheel-scroll delta to the editor.
    pub fn scroll(&mut self, delta: i32) {
        self.view.scroll(delta, &self.schema);
    }
}

fn get(cfg: &Config, key: &str) -> Value {
    match key {
        "general.language" => Value::String(cfg.general.language.clone()),
        "general.comfyui_dir" => Value::String(cfg.general.comfyui_dir.clone()),
        "general.python" => Value::String(cfg.general.python.clone()),
        "network.pypi_mirror" => Value::String(cfg.network.pypi_mirror.clone()),
        "network.git_mirror" => Value::String(cfg.network.git_mirror.clone()),
        "network.hf_mirror" => Value::String(cfg.network.hf_mirror.clone()),
        "network.github_accel" => Value::String(cfg.network.github_accel.clone()),
        _ => Value::String(String::new()),
    }
}

fn set(cfg: &mut Config, key: &str, v: Value) -> Result<(), String> {
    match key {
        "general.language" => {
            if let Some(s) = v.as_str() {
                cfg.general.language = s.to_string();
                let _ = cfg.save();
            }
            Ok(())
        }
        "general.comfyui_dir" => {
            let s = v.as_str().unwrap_or("").trim().to_string();
            if !ComfyDirs::new(&s).is_valid() {
                return Err(i18n::t("popup_invalid_dir"));
            }
            cfg.general.comfyui_dir = s;
            let _ = cfg.save();
            Ok(())
        }
        "general.python" => {
            let raw = v.as_str().unwrap_or("").trim();
            let resolved = python::resolve(std::path::Path::new(raw));
            if python::validate(&resolved).is_none() {
                return Err(i18n::t("popup_invalid_py"));
            }
            cfg.general.python = resolved.display().to_string();
            let _ = cfg.save();
            Ok(())
        }
        // Mirror fields: canonicalise the semicolon-separated list to the
        // tight `a;b;c` form so the on-disk file is consistent regardless
        // of the user's input style.
        "network.pypi_mirror" => {
            cfg.network.pypi_mirror =
                crate::core::env::normalize_semicolon_list(v.as_str().unwrap_or(""));
            let _ = cfg.save();
            Ok(())
        }
        "network.git_mirror" => {
            cfg.network.git_mirror =
                crate::core::env::normalize_semicolon_list(v.as_str().unwrap_or(""));
            let _ = cfg.save();
            Ok(())
        }
        "network.hf_mirror" => {
            cfg.network.hf_mirror =
                crate::core::env::normalize_semicolon_list(v.as_str().unwrap_or(""));
            let _ = cfg.save();
            Ok(())
        }
        "network.github_accel" => {
            cfg.network.github_accel =
                crate::core::env::normalize_semicolon_list(v.as_str().unwrap_or(""));
            let _ = cfg.save();
            Ok(())
        }
        _ => Ok(()),
    }
}
