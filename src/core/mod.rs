//! Core, UI-agnostic services used by every screen.
//!
//! Each submodule owns one concern (configuration, paths, process spawning,
//! git, i18n, and so on) so the TUI layers stay focused on layout and input.

/// System clipboard access.
pub mod clipboard;
/// Read-only inspection of a ComfyUI installation.
pub mod comfy_info;
/// Persistent launcher and ComfyUI configuration files.
pub mod config;
/// Build environment variables for spawned child processes.
pub mod env;
/// Online extension catalog fetch and cache.
pub mod extension_registry;
/// Wrappers around the `git` command-line.
pub mod git;
/// Runtime GPU detection via the configured Python interpreter.
pub mod gpu;
/// Embedded localisation catalogues and lookup.
pub mod i18n;
/// Process-wide log buffer mirrored to a session log file.
pub mod log_bus;
/// Open URLs in the platform's default browser.
pub mod opener;
/// Filesystem paths owned by the launcher.
pub mod paths;
/// Pip install helpers.
pub mod pip;
/// Spawn child processes and stream their output.
pub mod process;
/// Python interpreter detection and virtualenv inspection.
pub mod python;
/// Schema definitions for ComfyUI and launcher settings.
pub mod schema;
/// Width-aware text utilities for terminal layout.
pub mod text;
/// Color palette and styling helpers.
pub mod theme;
