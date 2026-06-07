// GPL-3.0-or-later

use std::{
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use eww_shared_util::VarName;
use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use once_cell::sync::OnceCell;
use simplexpr::dynval::DynVal;
use tokio::sync::mpsc::UnboundedSender;

use crate::manifest::{PluginManifest, VarKind};

type VarTx = UnboundedSender<(VarName, DynVal)>;

// Paths of all loaded plugin scripts; used by `invalidate_all` on hot-reload.
static PLUGIN_SCRIPTS: OnceCell<Vec<PathBuf>> = OnceCell::new();

// ── Discovery ─────────────────────────────────────────────────────────────────

struct LoadedPlugin {
    dir: PathBuf,
    script: PathBuf,
    manifest: PluginManifest,
    sandbox: meh_rhai_engine::ScriptSandbox,
}

fn discover(config_dir: &Path) -> Vec<LoadedPlugin> {
    let share_dir = std::env::var("HOME")
        .map(|h| PathBuf::from(h).join(".local/share/meh2/plugins"))
        .unwrap_or_else(|_| PathBuf::from("/nonexistent"));

    let search = [config_dir.join("plugins"), share_dir];
    let mut plugins = Vec::new();

    for plugins_dir in &search {
        let Ok(entries) = std::fs::read_dir(plugins_dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let dir = entry.path();
            if !dir.is_dir() {
                continue;
            }
            let manifest_path = dir.join("plugin.toml");
            let script_path = dir.join("main.rhai");

            if !manifest_path.exists() || !script_path.exists() {
                tracing::warn!(
                    "plugin-host: {} is missing plugin.toml or main.rhai, skipping",
                    dir.display()
                );
                continue;
            }

            let raw = match std::fs::read_to_string(&manifest_path) {
                Ok(s) => s,
                Err(e) => {
                    tracing::error!("plugin-host: {}: read plugin.toml: {}", dir.display(), e);
                    continue;
                }
            };
            let manifest: PluginManifest = match toml::from_str(&raw) {
                Ok(m) => m,
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
            let sandbox = meh_rhai_engine::ScriptSandbox::for_plugin(
                &dir,
                manifest.permissions.allow_shell,
                &manifest.permissions.read_files,
            );
            plugins.push(LoadedPlugin {
                dir,
                script: script_path,
                manifest,
                sandbox,
            });
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

    meh_rhai_engine::init();

    let script_paths_for_watch: Vec<PathBuf> = plugins.iter().map(|p| p.script.clone()).collect();

    // Build the widget registry from all discovered plugins before spawning tasks.
    let mut widget_registry: std::collections::HashMap<String, meh_rhai_engine::RhaiWidgetDef> =
        std::collections::HashMap::new();

    for plugin in &plugins {
        for w in &plugin.manifest.widgets {
            if widget_registry.contains_key(&w.name) {
                tracing::warn!(
                    "plugin-host: widget `{}` declared by `{}` conflicts with an earlier plugin — skipping",
                    w.name,
                    plugin.manifest.name,
                );
                continue;
            }
            tracing::info!(
                "plugin-host: registered widget `{}` from `{}`",
                w.name,
                plugin.manifest.name
            );
            widget_registry.insert(
                w.name.clone(),
                meh_rhai_engine::RhaiWidgetDef {
                    script_path: plugin.script.clone(),
                    fn_name: w.fn_name.clone(),
                    default_watch: w.default_watch.clone(),
                    sandbox: plugin.sandbox.clone(),
                },
            );
        }
    }

    meh_rhai_engine::init_widget_registry(widget_registry);

    for plugin in plugins {
        for var_decl in &plugin.manifest.vars {
            if var_decl.kind != VarKind::Poll {
                tracing::warn!(
                    "plugin-host: {}: var {} has unsupported kind, skipping",
                    plugin.manifest.name,
                    var_decl.name
                );
                continue;
            }

            let interval = Duration::from_secs(var_decl.interval.unwrap_or(60));
            let var_name = VarName(var_decl.name.clone());
            let fn_name = format!("get_{}", var_decl.name);
            let script = plugin.script.clone();
            let plugin_dir = plugin.dir.clone();
            let tx2 = tx.clone();
            let sd2 = shutdown.resubscribe();
            let wo2 = windows_open.clone();

            let sandbox = plugin.sandbox.clone();
            tokio::spawn(run_plugin_var(
                script, plugin_dir, fn_name, var_name, interval, tx2, sd2, wo2, sandbox,
            ));
        }
    }

    spawn_file_watcher(script_paths_for_watch);
}

/// Invalidate all plugin ASTs from the Rhai cache so next poll tick recompiles.
/// Call this when `meh2 reload` fires to pick up changed plugin scripts.
pub fn invalidate_all() {
    let Some(scripts) = PLUGIN_SCRIPTS.get() else {
        return;
    };
    let Some(engine) = meh_rhai_engine::global() else {
        return;
    };
    for path in scripts {
        engine.invalidate(path);
    }
    tracing::debug!("plugin-host: invalidated {} plugin AST(s)", scripts.len());
}

// ── File watcher (auto-invalidate on script change) ───────────────────────────

fn spawn_file_watcher(scripts: Vec<PathBuf>) {
    let (tx, mut rx) = tokio::sync::mpsc::channel::<PathBuf>(32);

    let watcher_result = RecommendedWatcher::new(
        move |result: notify::Result<Event>| {
            let Ok(event) = result else { return };
            if !matches!(event.kind, EventKind::Modify(_) | EventKind::Create(_)) {
                return;
            }
            for path in event.paths {
                if path.file_name().map(|n| n == "main.rhai").unwrap_or(false) {
                    let _ = tx.blocking_send(path);
                }
            }
        },
        notify::Config::default(),
    );

    let mut watcher = match watcher_result {
        Ok(w) => w,
        Err(e) => {
            tracing::warn!("plugin-host: file watcher unavailable: {}", e);
            return;
        }
    };

    for script in &scripts {
        if let Some(dir) = script.parent()
            && let Err(e) = watcher.watch(dir, RecursiveMode::NonRecursive)
        {
            tracing::warn!("plugin-host: cannot watch {}: {}", dir.display(), e);
        }
    }

    tokio::spawn(async move {
        let _watcher = watcher; // keep alive for the duration of this task
        while let Some(path) = rx.recv().await {
            if let Some(engine) = meh_rhai_engine::global() {
                engine.invalidate(&path);
                tracing::info!("plugin-host: auto-reloaded {}", path.display());
            }
        }
    });
}

// ── Poll task ─────────────────────────────────────────────────────────────────

#[allow(clippy::too_many_arguments)]
async fn run_plugin_var(
    script: PathBuf,
    plugin_dir: PathBuf,
    fn_name: String,
    var_name: VarName,
    interval: Duration,
    tx: VarTx,
    mut shutdown: tokio::sync::broadcast::Receiver<()>,
    windows_open: Arc<AtomicBool>,
    sandbox: meh_rhai_engine::ScriptSandbox,
) {
    let engine = meh_rhai_engine::global().unwrap_or_else(|| meh_rhai_engine::init());

    // Initial fetch — always runs regardless of windows_open so var_state is
    // populated before the first window opens (same behaviour as defpoll).
    let mut last = call_and_send(
        &engine,
        &script,
        &plugin_dir,
        &fn_name,
        &var_name,
        &tx,
        None,
        Some(sandbox.clone()),
    )
    .await;

    let mut timer = tokio::time::interval(interval);
    timer.tick().await; // skip the first immediate tick

    loop {
        tokio::select! {
            _ = shutdown.recv() => break,
            _ = timer.tick() => {
                if !windows_open.load(Ordering::Relaxed) {
                    continue;
                }
                last = call_and_send(
                    &engine,
                    &script,
                    &plugin_dir,
                    &fn_name,
                    &var_name,
                    &tx,
                    last,
                    Some(sandbox.clone()),
                )
                .await;
            }
        }
    }
}

async fn call_and_send(
    engine: &Arc<meh_rhai_engine::RhaiEngine>,
    script: &Path,
    plugin_dir: &Path,
    fn_name: &str,
    var_name: &VarName,
    tx: &VarTx,
    last: Option<String>,
    sandbox: Option<meh_rhai_engine::ScriptSandbox>,
) -> Option<String> {
    let engine = engine.clone();
    let script = script.to_path_buf();
    let plugin_dir = plugin_dir.to_path_buf();
    let fn_name = fn_name.to_string();

    let result = tokio::task::spawn_blocking(move || {
        engine.call_fn_sandboxed(&script, &plugin_dir, &fn_name, sandbox)
    })
    .await;

    match result {
        Ok(Ok(val)) => {
            if last.as_deref() != Some(&val) {
                let _ = tx.send((var_name.clone(), DynVal::from_string(val.clone())));
                Some(val)
            } else {
                last
            }
        }
        Ok(Err(e)) => {
            tracing::error!("plugin var {}: {}", var_name, e);
            last
        }
        Err(e) => {
            tracing::error!("plugin var {}: task panicked: {}", var_name, e);
            last
        }
    }
}
