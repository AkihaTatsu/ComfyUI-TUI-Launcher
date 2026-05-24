//! Schema definitions for the ComfyUI settings editor and the launcher
//! settings editor.
//!
//! Both schemas are embedded in the binary; they are never copied to disk
//! so upgrades pick up new fields automatically.

use crate::core::paths;
use anyhow::{Context, Result};
use serde::Deserialize;
use std::path::PathBuf;
use toml::Value;

/// Embedded ComfyUI settings schema.
pub const DEFAULT_SCHEMA: &str = include_str!("../../assets/comfyui_schema.toml");
/// Embedded launcher settings schema.
pub const DEFAULT_LAUNCHER_SCHEMA: &str = include_str!("../../assets/launcher_schema.toml");

/// One option of a `Choice` field.
#[derive(Debug, Clone, Deserialize)]
pub struct Choice {
    /// Value stored on disk and emitted on the command line.
    pub value: String,
    /// Display label shown in the picker.
    pub label: String,
}

/// Editor widget type for a `Field`.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum FieldType {
    /// Boolean toggle.
    Toggle,
    /// Single choice picked from a static list of options.
    Choice {
        /// Selectable options.
        #[serde(default, rename = "option")]
        options: Vec<Choice>,
    },
    /// Free-form text value.
    Custom,
    /// Single choice whose options are generated at render time from
    /// `i18n::available_locales()`. No options are listed in TOML.
    LanguageChoice,
    /// Single choice whose options are generated at render time from
    /// `gpu::detect()`. No options are listed in TOML.
    GpuChoice,
}

/// One editable field defined by the schema.
#[derive(Debug, Clone, Deserialize)]
pub struct Field {
    /// Unique key used to look up the value in storage.
    pub key: String,
    /// Display name shown next to the editor.
    pub name: String,
    /// Optional help text shown below the field.
    #[serde(default)]
    pub desc: String,
    /// Editor type.
    #[serde(flatten)]
    pub ty: FieldType,
    /// CLI argument template used when building the ComfyUI command line.
    #[serde(default)]
    pub cli: String,
    /// Default value when the user has not set one.
    #[serde(default = "default_value")]
    pub default: Value,
}

fn default_value() -> Value {
    Value::String(String::new())
}

/// One tab of fields in the schema.
#[derive(Debug, Clone, Deserialize)]
pub struct Tab {
    /// Display name of the tab.
    pub name: String,
    /// Fields displayed inside the tab.
    #[serde(default, rename = "field")]
    pub fields: Vec<Field>,
}

/// A parsed settings schema.
#[derive(Debug, Clone, Deserialize, Default)]
pub struct Schema {
    /// Tabs in display order.
    #[serde(default, rename = "tab")]
    pub tabs: Vec<Tab>,
}

/// Parses the embedded ComfyUI settings schema.
///
/// The returned `PathBuf` is always empty and kept for backwards
/// compatibility with older call sites that expected an on-disk path.
pub fn load_or_init() -> Result<(Schema, PathBuf)> {
    paths::ensure_config_dir().ok();
    paths::migrate_legacy_filenames();
    let s: Schema = toml::from_str(DEFAULT_SCHEMA).context("parse embedded schema")?;
    Ok((s, PathBuf::new()))
}

/// Parses the embedded launcher settings schema.
pub fn load_launcher_or_init() -> Result<Schema> {
    toml::from_str(DEFAULT_LAUNCHER_SCHEMA).context("parse embedded launcher schema")
}

/// Builds the CLI arguments to pass to ComfyUI for the supplied settings.
pub fn build_cli_args(
    schema: &Schema,
    values: &std::collections::BTreeMap<String, Value>,
) -> Vec<String> {
    let mut out = Vec::new();
    for tab in &schema.tabs {
        for f in &tab.fields {
            let v = values.get(&f.key).unwrap_or(&f.default);
            if v == &f.default {
                continue;
            }
            match &f.ty {
                FieldType::Toggle => {
                    if v.as_bool().unwrap_or(false) && !f.cli.is_empty() {
                        for tok in f.cli.split_whitespace() {
                            out.push(tok.to_string());
                        }
                    }
                }
                FieldType::Choice { .. } | FieldType::Custom => {
                    let s = value_to_string(v);
                    if s.is_empty() {
                        continue;
                    }
                    let rendered = f.cli.replace("{value}", &s);
                    for tok in rendered.split_whitespace() {
                        out.push(tok.to_string());
                    }
                }
                FieldType::LanguageChoice => {
                    // Launcher-side only; never appears on the ComfyUI
                    // command line.
                }
                FieldType::GpuChoice => {
                    let s = value_to_string(v);
                    out.extend(crate::core::gpu::config_value_to_cli_args(&s));
                }
            }
        }
    }
    out
}

fn value_to_string(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Integer(i) => i.to_string(),
        Value::Float(f) => f.to_string(),
        Value::Boolean(b) => b.to_string(),
        _ => String::new(),
    }
}
