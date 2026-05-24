//! ComfyUI settings editor screen.

use super::settings_view::SettingsView;
use crate::core::config::Config;
use crate::core::schema::Schema;
use crossterm::event::KeyCode;
use ratatui::layout::Rect;
use ratatui::Frame;
use toml::Value;

/// Thin wrapper around `SettingsView` driven by the ComfyUI schema.
///
/// Adds the screen-specific `R` shortcut that resets every value to its
/// factory default.
pub struct ComfySettings {
    /// Underlying settings editor.
    pub view: SettingsView,
}

impl ComfySettings {
    /// Constructs a fresh ComfyUI settings screen.
    pub fn new() -> Self {
        Self {
            view: SettingsView::new(),
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
    pub fn render(
        &self,
        f: &mut Frame,
        area: Rect,
        schema: &Schema,
        cfg: &Config,
        body_active: bool,
    ) {
        let get = |cfg: &Config, key: &str| -> Value {
            if let Some(v) = cfg.get_comfy(key) {
                return v.clone();
            }
            for tab in &schema.tabs {
                for f in &tab.fields {
                    if f.key == key {
                        return f.default.clone();
                    }
                }
            }
            Value::String(String::new())
        };
        let show_tabs = schema.tabs.len() > 1;
        self.view
            .render(f, area, schema, cfg, get, show_tabs, body_active);
    }

    /// Handles a key event.
    pub fn on_key(&mut self, code: KeyCode, schema: &Schema, cfg: &mut Config) {
        let get = |cfg: &Config, key: &str| -> Value {
            if let Some(v) = cfg.get_comfy(key) {
                return v.clone();
            }
            for tab in &schema.tabs {
                for f in &tab.fields {
                    if f.key == key {
                        return f.default.clone();
                    }
                }
            }
            Value::String(String::new())
        };
        let set = |cfg: &mut Config, key: &str, v: Value| -> Result<(), String> {
            cfg.set_comfy(key, v);
            Ok(())
        };
        // Delegate to the view first. If it consumed the key (text input,
        // popup, or navigation), don't process screen-level shortcuts.
        if self.view.on_key(code, schema, cfg, get, set) {
            return;
        }
        // View didn't consume — handle screen-level shortcuts.
        if matches!(code, KeyCode::Char('r') | KeyCode::Char('R')) {
            cfg.reset_comfy_to_factory();
        }
    }

    /// Handles a mouse event.
    pub fn on_mouse(
        &mut self,
        m: crossterm::event::MouseEvent,
        area: Rect,
        schema: &Schema,
        cfg: &mut Config,
    ) {
        let get = |cfg: &Config, key: &str| -> Value {
            if let Some(v) = cfg.get_comfy(key) {
                return v.clone();
            }
            for tab in &schema.tabs {
                for f in &tab.fields {
                    if f.key == key {
                        return f.default.clone();
                    }
                }
            }
            Value::String(String::new())
        };
        let set = |cfg: &mut Config, key: &str, v: Value| -> Result<(), String> {
            cfg.set_comfy(key, v);
            Ok(())
        };
        let show_tabs = schema.tabs.len() > 1;
        self.view
            .on_mouse(m, area, schema, cfg, get, set, show_tabs);
    }

    /// Forwards a wheel-scroll delta to the editor.
    pub fn scroll(&mut self, delta: i32, schema: &Schema) {
        self.view.scroll(delta, schema);
    }
}
