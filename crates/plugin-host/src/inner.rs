// GPL-3.0-or-later

use std::{
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::Duration,
};

use eww_shared_util::VarName;
use once_cell::sync::OnceCell;
use simplexpr::dynval::DynVal;
use tokio::sync::mpsc::UnboundedSender;

use crate::manifest::{PluginManifest, VarKind};

type VarTx = UnboundedSender<(VarName, DynVal)>;

// Paths of all loaded plugin scripts; used by `invalidate_all` on hot-reload.
static PLUGIN_SCRIPTS: OnceCell<Vec<PathBuf>> = OnceCell::new();

// ── Discovery ─────────────────────────────────────────────────────────────────

struct LoadedPlugin {
    dir:      PathBuf,
    script:   PathBuf,
    manifest: PluginManifest,
}

fn discover(config_dir: &Path) -> Vec<LoadedPlugin> {
    let share_dir = std::env::var("HOME")
        .map(|h| PathBuf::from(h).join(".local/share/meh2/plugins"))
        .unwrap_or_else(|_| PathBuf::from("/nonexistent"));

    let search = [config_dir.join("plugins"), share_dir];
    let mut plugins = Vec::new();

    for plugins_dir in &search {
        let Ok(entries) = std::fs::read_dir(plugins_dir) else { continue };
        for entry in entries.flatten() {
            let dir = entry.path();
            if !dir.is_dir() {
                continue;
            }
            let manifest_path = dir.join("plugin.toml");
            let script_path   = dir.join("main.rhai");

            if !manifest_path.exists() || !script_path.exists() {
                tracing::warn!(
                    "plugin-host: {} is missing plugin.toml or main.rhai, skipping",
                    dir.display()
                );
                continue;
            }

            let raw = match std::fs::read_to_string(&manifest_path) {
                Ok(s)  => s,
                Err(e) => {
                    tracing::error!("plugin-host: {}: read plugin.toml: {}", dir.display(), e);
                    continue;
                }
            };
            let manifest: PluginManifest = match toml::from_str(&raw) {
                Ok(m)  => m,
                Err(e) => {
                    tracing::error!("plugin-host: {}: parse plugin.toml: {}", dir.display(), e);
                    continue;
                }
            };

            tracing::info!(
                "plugin-host: loaded {} v{} ({} vars)",
                manifest.name,
                manifest.version,
                manifest.vars.len()
            );
            plugins.push(LoadedPlugin { dir, script: script_path, manifest });
        }
    }

    plugins
}

// ── Public API ────────────────────────────────────────────────────────────────

/// Discover plugins, compile their scripts, and spawn a poll task per declared var.
///
/// Updates are sent on `tx`, which is shared with `script_vars::start_all` so
/// all updates flow through the same `forward_var_updates` pipeline.
pub fn start_plugins(
    config_dir: &Path,
    tx: VarTx,
    shutdown: tokio::sync::broadcast::Receiver<()>,
    windows_open: Arc<AtomicBool>,
) {
    let plugins = discover(config_dir);

    // Register script paths for later invalidation.
    let script_paths: Vec<PathBuf> = plugins.iter().map(|p| p.script.clone()).collect();
    let _ = PLUGIN_SCRIPTS.set(script_paths);

    if plugins.is_empty() {
        return;
    }

    for plugin in plugins {
        for var_decl in &plugin.manifest.vars {
            if var_decl.kind != VarKind::Poll {
                // Only poll vars are supported in Phase 3.
                tracing::warn!(
                    "plugin-host: {}: var {} has unsupported type, skipping",
                    plugin.manifest.name,
                    var_decl.name
                );
                continue;
            }

            let interval   = Duration::from_secs(var_decl.interval.unwrap_or(60));
            let var_name   = VarName(var_decl.name.clone());
            let fn_name    = format!("get_{}", var_decl.name);
            let script     = plugin.script.clone();
            let plugin_dir = plugin.dir.clone();
            let tx2        = tx.clone();
            let sd2        = shutdown.resubscribe();
            let wo2        = windows_open.clone();

            tokio::spawn(run_plugin_var(
                script, plugin_dir, fn_name, var_name, interval, tx2, sd2, wo2,
            ));
        }
    }
}

/// Invalidate all plugin ASTs from the Rhai cache so next poll tick recompiles.
/// Call this when `meh2 reload` fires to pick up changed plugin scripts.
pub fn invalidate_all() {
    let Some(scripts) = PLUGIN_SCRIPTS.get() else { return };
    let Some(engine)  = meh_rhai_engine::global() else { return };
    for path in scripts {
        engine.invalidate(path);
    }
    tracing::debug!("plugin-host: invalidated {} plugin AST(s)", scripts.len());
}

// ── Poll task ─────────────────────────────────────────────────────────────────

async fn run_plugin_var(
    script:     PathBuf,
    plugin_dir: PathBuf,
    fn_name:    String,
    var_name:   VarName,
    interval:   Duration,
    tx:         VarTx,
    mut shutdown: tokio::sync::broadcast::Receiver<()>,
    windows_open: Arc<AtomicBool>,
) {
    let Some(engine) = meh_rhai_engine::global() else {
        tracing::warn!("plugin var {}: rhai engine not initialised", var_name);
        return;
    };

    // Initial fetch — always runs regardless of windows_open so var_state is
    // populated before the first window opens (same behaviour as defpoll).
    call_and_send(&engine, &script, &plugin_dir, &fn_name, &var_name, &tx).await;

    let mut timer = tokio::time::interval(interval);
    timer.tick().await; // skip the first immediate tick

    loop {
        tokio::select! {
            _ = shutdown.recv() => break,
            _ = timer.tick() => {
                if !windows_open.load(Ordering::Relaxed) {
                    continue;
                }
                call_and_send(&engine, &script, &plugin_dir, &fn_name, &var_name, &tx).await;
            }
        }
    }
}

async fn call_and_send(
    engine:     &Arc<meh_rhai_engine::RhaiEngine>,
    script:     &Path,
    plugin_dir: &Path,
    fn_name:    &str,
    var_name:   &VarName,
    tx:         &VarTx,
) {
    let engine     = engine.clone();
    let script     = script.to_path_buf();
    let plugin_dir = plugin_dir.to_path_buf();
    let fn_name    = fn_name.to_string();

    let result = tokio::task::spawn_blocking(move || {
        engine.call_fn(&script, &plugin_dir, &fn_name)
    })
    .await;

    match result {
        Ok(Ok(val))  => { let _ = tx.send((var_name.clone(), DynVal::from_string(val))); }
        Ok(Err(e))   => tracing::error!("plugin var {}: {}", var_name, e),
        Err(e)       => tracing::error!("plugin var {}: task panicked: {}", var_name, e),
    }
}
