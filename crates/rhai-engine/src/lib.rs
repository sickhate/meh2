// GPL-3.0-or-later
//! Sandboxed Rhai scripting engine for meh2.
//!
//! One global `RhaiEngine` instance lives for the daemon lifetime.
//! All scripts are compiled to AST once (cached); per-tick cost is a
//! `Scope` allocation + `eval_ast_with_scope` call (~50–500µs typical).
//!
//! Plugin scripts run under [`ScriptSandbox`] restrictions; config scripts
//! (`defpoll`, event handlers, user `(rhai-widget)`) are unrestricted.
//!
//! # Feature gate
//! This crate does nothing unless the `rhai` feature is enabled at the
//! workspace level. The stub type is always present so dependents compile
//! without conditional imports.

use std::{collections::HashMap, path::PathBuf};

// ── Rhai-free transfer types (always compiled) ────────────────────────────────

/// Widget tree produced by `call_fn_as_widget_data`.
/// Converted to `WidgetUse` in `gtk4-impl` for rendering.
#[derive(Debug, Clone)]
pub struct RhaiWidgetData {
    pub widget_type: String,
    /// Attribute key-value pairs (both serialised as strings).
    pub attrs: Vec<(String, String)>,
    pub children: Vec<RhaiWidgetData>,
}

/// A plugin-registered widget type.
/// Stored in `WIDGET_REGISTRY`; looked up by name in `gtk4-impl::build_basic`.
#[derive(Debug, Clone)]
pub struct RhaiWidgetDef {
    /// Absolute path to the plugin's main.rhai.
    pub script_path: PathBuf,
    /// Rhai function to call (e.g. `"render_sysinfo_pill"`).
    pub fn_name: String,
    /// Default vars to watch when `:watch` is not specified at the call site.
    pub default_watch: Vec<String>,
    /// Sandbox policy from the plugin manifest.
    pub sandbox: ScriptSandbox,
}

// ── Widget registry (always compiled) ────────────────────────────────────────

use once_cell::sync::OnceCell;

/// Set exactly once at daemon startup by `plugin-host::start_plugins`.
/// Maps widget name → definition. Immutable after init; safe to read without locking.
static WIDGET_REGISTRY: OnceCell<HashMap<String, RhaiWidgetDef>> = OnceCell::new();

/// Register all plugin widgets at daemon startup (call exactly once).
/// Silently no-ops if called more than once (idempotent after first set).
pub fn init_widget_registry(defs: HashMap<String, RhaiWidgetDef>) {
    let _ = WIDGET_REGISTRY.set(defs);
}

/// Look up a plugin-registered widget by name. Returns `None` if the registry
/// has not been initialised or the name is unknown.
pub fn get_widget_def(name: &str) -> Option<&'static RhaiWidgetDef> {
    WIDGET_REGISTRY.get()?.get(name)
}

// ── Stub (rhai feature disabled) ─────────────────────────────────────────────

#[cfg(not(feature = "rhai"))]
pub struct RhaiEngine;

#[cfg(not(feature = "rhai"))]
pub fn global() -> Option<std::sync::Arc<RhaiEngine>> {
    None
}

#[cfg(not(feature = "rhai"))]
pub fn init() -> std::sync::Arc<RhaiEngine> {
    std::sync::Arc::new(RhaiEngine)
}

#[cfg(not(feature = "rhai"))]
impl RhaiEngine {
    pub fn eval_file(
        &self,
        _path: &std::path::Path,
        _config_dir: &std::path::Path,
    ) -> anyhow::Result<String> {
        anyhow::bail!("meh2 built without `rhai` feature")
    }
    pub fn eval_inline(&self, _script: &str) -> anyhow::Result<String> {
        anyhow::bail!("meh2 built without `rhai` feature")
    }
    pub fn call_fn(
        &self,
        _path: &std::path::Path,
        _config_dir: &std::path::Path,
        _fn_name: &str,
    ) -> anyhow::Result<String> {
        anyhow::bail!("meh2 built without `rhai` feature")
    }
    pub fn call_fn_as_widget_data(
        &self,
        _path: &std::path::Path,
        _config_dir: &std::path::Path,
        _fn_name: &str,
        _vars: &HashMap<String, String>,
    ) -> anyhow::Result<crate::RhaiWidgetData> {
        anyhow::bail!("meh2 built without `rhai` feature")
    }
    pub fn invalidate(&self, _path: &std::path::Path) {}
    pub fn invalidate_all(&self) {}
}

// ── Real implementation ───────────────────────────────────────────────────────

mod sandbox;
#[cfg(feature = "rhai")]
mod inner;

pub use sandbox::ScriptSandbox;

#[cfg(feature = "rhai")]
pub use inner::{RhaiEngine, global, init};
