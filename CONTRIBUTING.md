# Contributing

Thanks for your interest!

## Build & run

```sh
cargo run                 # debug build
cargo build --release     # optimized binary in target/release/
cargo fmt                 # format
cargo clippy -- -D warnings
```

## Project layout

- `src/core/` — non-UI logic (config, schema, git, python detection, process management).
- `src/widgets/` — reusable UI primitives. Each widget lives in its own file and must not depend on screens or other widgets.
- `src/screens/` — one file per screen; depends only on `core` and `widgets`.
- `assets/` — embedded resources: default config, settings schema, i18n catalogs.

## Adding a new ComfyUI flag

Edit `assets/settings_schema.toml`. The TUI picks up the change on next launch (or, for end users, by editing the extracted `settings_schema.toml` in their config dir).

```toml
[[tab.field]]
key = "my_new_flag"
name = "My New Flag"
desc = "Short description"
type = "toggle"            # or "choice" / "custom"
cli  = "--my-new-flag"
default = false
```

## Adding a translation

Copy `assets/i18n/en.toml` to a new locale code (e.g. `fr.toml`) and translate values. Keys must match.
