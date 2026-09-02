# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.2.0] - 2026-09-02

### Added
- Cross-platform clipboard support for local desktops and remote terminal
  sessions, including SSH, Mosh, tmux, Zellij, GNU Screen, and Herdr.
- OSC 52 clipboard transport with tmux `set-buffer -w` integration and native
  clipboard fallback handling.
- Optional forced Hugging Face mirror rewriting for ComfyUI downloads.
- Reusable cached log display, in-place progress updates, and terminal-control
  sequence sanitization.
- Automated tests for clipboard multiplexers and compile checks for FreeBSD,
  NetBSD, and OpenBSD targets.

### Changed
- Moved repository discovery and version-management refreshes off the UI thread
  to keep the interface responsive during Git and extension operations.
- Made extension synchronization bounded and parallel while keeping dependency
  installation serialized for shared Python environments.
- Improved tab navigation, keyboard paging, narrow-terminal handling, and
  launcher information refreshes.
- Unified process output, task outcomes, and partial-failure reporting across
  launcher and version-management operations.

### Fixed
- Prevented clipboard operations in remote sessions from connecting to stale or
  forwarded X11 and Wayland displays.
- Preserved Git mirror configuration when adding runtime `safe.directory`
  settings.
- Corrected Windows process launch/restart behavior and working-directory
  inheritance.
- Prevented ANSI escapes and carriage-return progress output from corrupting the
  terminal UI.

## [0.1.2] - 2026-06-26

### Changed
- Maintenance release.

## [0.1.1] - 2026-06-22

### Fixed
- Windows compatibility and restart behavior.

## [0.1.0] - 2026-05-24

### Added
- Initial TUI scaffold (Rust + ratatui).
- Main Launcher, ComfyUI Settings, Version Management, General Settings, Console screens.
- First-launch tutorial: ComfyUI directory + Python interpreter selection.
- TOML-driven dynamic settings schema, extracted to the user config dir on first run.
- English + Simplified Chinese i18n catalogs (English default).

[Unreleased]: https://github.com/AkihaTatsu/ComfyUI-TUI-Launcher/compare/v0.2.0...HEAD
[0.2.0]: https://github.com/AkihaTatsu/ComfyUI-TUI-Launcher/compare/v0.1.2...v0.2.0
[0.1.2]: https://github.com/AkihaTatsu/ComfyUI-TUI-Launcher/compare/v0.1.1...v0.1.2
[0.1.1]: https://github.com/AkihaTatsu/ComfyUI-TUI-Launcher/compare/v0.1.0...v0.1.1
[0.1.0]: https://github.com/AkihaTatsu/ComfyUI-TUI-Launcher/releases/tag/v0.1.0
