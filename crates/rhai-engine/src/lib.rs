// GPL-3.0-or-later
//! Sandboxed Rhai scripting engine for meh2.
//!
//! One global `RhaiEngine` instance lives for the daemon lifetime.
//! All scripts are compiled to AST once (cached); per-tick cost is a
//! `Scope` allocation + `eval_ast_with_scope` call (~50–500µs typical).
//!
//! # Feature gate
//! This crate does nothing unless the `rhai` feature is enabled at the
//! workspace level. The stub type is always present so dependents compile
//! without conditional imports.

/// Rhai-free widget tree produced by `call_fn_as_widget_data`.
/// Converted to `WidgetUse` in `gtk4-impl` for rendering.
#[derive(Debug, Clone)]
pub struct RhaiWidgetData {
    pub widget_type: String,
    /// Attribute key-value pairs (both serialised as strings).
    pub attrs:    Vec<(String, String)>,
    pub children: Vec<RhaiWidgetData>,
}

// ── Stub (rhai feature disabled) ─────────────────────────────────────────────

#[cfg(not(feature = "rhai"))]
pub struct RhaiEngine;

#[cfg(not(feature = "rhai"))]
pub fn global() -> Option<std::sync::Arc<RhaiEngine>> { None }

#[cfg(not(feature = "rhai"))]
pub fn init() -> std::sync::Arc<RhaiEngine> {
    std::sync::Arc::new(RhaiEngine)
}

#[cfg(not(feature = "rhai"))]
impl RhaiEngine {
    pub fn eval_file(&self, _path: &std::path::Path, _config_dir: &std::path::Path) -> anyhow::Result<String> {
        anyhow::bail!("meh2 built without `rhai` feature")
    }
    pub fn eval_inline(&self, _script: &str) -> anyhow::Result<String> {
        anyhow::bail!("meh2 built without `rhai` feature")
    }
    pub fn call_fn(&self, _path: &std::path::Path, _config_dir: &std::path::Path, _fn_name: &str) -> anyhow::Result<String> {
        anyhow::bail!("meh2 built without `rhai` feature")
    }
    pub fn call_fn_as_widget_data(&self, _path: &std::path::Path, _config_dir: &std::path::Path, _fn_name: &str, _vars: &std::collections::HashMap<String, String>) -> anyhow::Result<crate::RhaiWidgetData> {
        anyhow::bail!("meh2 built without `rhai` feature")
    }
    pub fn invalidate(&self, _path: &std::path::Path) {}
}

// ── Real implementation ───────────────────────────────────────────────────────

#[cfg(feature = "rhai")]
mod inner;
#[cfg(feature = "rhai")]
pub use inner::{RhaiEngine, global, init};
