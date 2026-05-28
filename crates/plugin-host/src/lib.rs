// GPL-3.0-or-later
//! Rhai plugin host — discovers, loads, and drives plugin vars.
//!
//! Plugins live in `~/.config/meh2/plugins/<name>/` or
//! `~/.local/share/meh2/plugins/<name>/`. Each plugin directory must contain:
//!
//! - `plugin.toml` — manifest declaring the plugin's name, version, and vars.
//! - `main.rhai`   — Rhai script exporting `fn get_<VARNAME>() -> String` for
//!                   each declared var.
//!
//! # Feature gate
//! This module is a no-op when the `rhai-plugins` feature is disabled.

mod manifest;
pub use manifest::{Permissions, PluginManifest, VarDecl, VarKind};

// ── Stub (rhai-plugins feature disabled) ─────────────────────────────────────

#[cfg(not(feature = "rhai-plugins"))]
pub fn start_plugins(
    _config_dir: &std::path::Path,
    _tx: tokio::sync::mpsc::UnboundedSender<(eww_shared_util::VarName, simplexpr::dynval::DynVal)>,
    _shutdown: tokio::sync::broadcast::Receiver<()>,
    _windows_open: std::sync::Arc<std::sync::atomic::AtomicBool>,
) {
}

#[cfg(not(feature = "rhai-plugins"))]
pub fn invalidate_all() {}

// ── Real implementation ───────────────────────────────────────────────────────

#[cfg(feature = "rhai-plugins")]
mod inner;

#[cfg(feature = "rhai-plugins")]
pub use inner::{invalidate_all, start_plugins};
