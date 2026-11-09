# Patched tray-icon

Unmodified copy of [`tray-icon` 0.24.1](https://crates.io/crates/tray-icon/0.24.1) (the
version Tauri 2.11 pins), except for `src/platform_impl/gtk/mod.rs`: on Linux, the tooltip
is applied as the AppIndicator **title** (in `new` and `set_tooltip`), because upstream's
GTK backend drops tooltips entirely and desktops like Cinnamon show the title on hover.
Search for `PATCHED (now-playing-flagger)` to find both changes.

Wired in via `[patch.crates-io]` in `src-tauri/Cargo.toml`. Cargo silently ignores the patch
(with a warning) if Tauri starts requiring a different tray-icon version -- when that happens,
re-copy the new version from crates.io and re-apply the two changes, or drop this directory
if upstream has gained Linux tooltip support by then. Windows/macOS code is untouched.
