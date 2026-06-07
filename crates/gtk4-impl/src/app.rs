// GPL-3.0-or-later
//! GTK4 application state and command handling.

use std::{
    collections::{HashMap, HashSet},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

use tokio::sync::Notify;

use anyhow::Result;
use eww_shared_util::VarName;
use gtk4::prelude::*;
use meh_core::{EvalCtx, IpcResponse, MehConfig, MehPaths};
use simplexpr::dynval::DynVal;

use crate::window::{self, LiveWindow};

/// Commands dispatched from the IPC server to the GTK main loop.
#[derive(Debug)]
pub enum Cmd {
    Open {
        window: String,
        toggle: bool,
        monitor: Option<i32>,
        resp: tokio::sync::oneshot::Sender<IpcResponse>,
    },
    Close {
        windows: Vec<String>,
        resp: tokio::sync::oneshot::Sender<IpcResponse>,
    },
    CloseAll {
        resp: tokio::sync::oneshot::Sender<IpcResponse>,
    },
    Reload {
        resp: tokio::sync::oneshot::Sender<IpcResponse>,
    },
    Update {
        vars: HashMap<String, String>,
        resp: tokio::sync::oneshot::Sender<IpcResponse>,
    },
    State {
        resp: tokio::sync::oneshot::Sender<IpcResponse>,
    },
    Get {
        var: String,
        resp: tokio::sync::oneshot::Sender<IpcResponse>,
    },
    ListWindows {
        resp: tokio::sync::oneshot::Sender<IpcResponse>,
    },
    /// Internal: batch script-var updates; no IPC response needed.
    SetVarBatch {
        vars: HashMap<VarName, DynVal>,
    },
    /// Internal: GDK monitors list changed; handle connect/disconnect.
    MonitorsChanged,
    Kill,
}

pub struct App {
    pub paths: MehPaths,
    pub config: MehConfig,
    pub open_windows: HashMap<String, LiveWindow>,
    pub css_provider: gtk4::CssProvider,
    pub main_loop: gtk4::glib::MainLoop,
    /// Shared flag: suppresses subprocess execution when no windows are open.
    pub windows_open: Arc<AtomicBool>,
    /// Notified when the first window opens so pending initial var values flush immediately.
    pub window_opened: Arc<Notify>,
    /// Notified when the last window closes so listen subprocesses can be killed.
    pub window_closed: Arc<Notify>,
    /// Windows waiting to be reopened after their monitor reconnects.
    /// Each entry is (window_name, monitor_connector).
    pub pending_reopen: Vec<(String, String)>,
}

impl App {
    pub fn new(
        paths: MehPaths,
        config: MehConfig,
        windows_open: Arc<AtomicBool>,
        window_opened: Arc<Notify>,
        window_closed: Arc<Notify>,
    ) -> Self {
        let css_provider = gtk4::CssProvider::new();
        let main_loop = gtk4::glib::MainLoop::new(None, false);
        Self {
            paths,
            config,
            open_windows: HashMap::new(),
            css_provider,
            main_loop,
            windows_open,
            window_opened,
            window_closed,
            pending_reopen: Vec::new(),
        }
    }

    pub fn apply_css(&self) {
        if let Some(css) = meh_core::compile_css(&self.paths) {
            self.css_provider.load_from_string(&css);
        }
        if let Some(display) = gdk::Display::default() {
            gtk4::style_context_add_provider_for_display(
                &display,
                &self.css_provider,
                gtk4::STYLE_PROVIDER_PRIORITY_USER,
            );
        }
    }

    // Copy live var values from the current config into a freshly loaded one.
    // Prevents slow-polling vars (e.g. USERNAME at 60 s) from going blank after reload.
    // The async updater overwrites each value on its next tick, so staleness is bounded.
    fn carry_var_state(&self, new_config: &mut MehConfig) {
        for (name, val) in &self.config.var_state.vars {
            let known = new_config.yuck.var_definitions.contains_key(name)
                || new_config.yuck.script_vars.contains_key(name);
            if known {
                new_config.var_state.set(name.clone(), val.clone());
            }
        }
    }

    #[cfg(not(feature = "granular-reload"))]
    pub fn reload_config(&mut self) -> Result<()> {
        let mut new_config = MehConfig::load(&self.paths)?;
        self.carry_var_state(&mut new_config);
        self.config = new_config;
        self.pending_reopen.clear();
        self.apply_css();
        let window_names: Vec<String> = self.open_windows.keys().cloned().collect();
        for name in &window_names {
            self.close_window(name);
        }
        for name in &window_names {
            if let Err(e) = self.open_window(name, false, None) {
                tracing::warn!("Failed to reopen window {}: {}", name, e);
            }
        }
        Ok(())
    }

    #[cfg(feature = "granular-reload")]
    pub fn reload_config(&mut self) -> Result<()> {
        let mut new_config = MehConfig::load(&self.paths)?;
        self.carry_var_state(&mut new_config);
        let window_names: Vec<String> = self.open_windows.keys().cloned().collect();

        let changed: Vec<String> = window_names
            .iter()
            .filter(|name| ir_hash(name, &self.config) != ir_hash(name, &new_config))
            .cloned()
            .collect();

        self.config = new_config;
        self.pending_reopen.clear();
        self.apply_css();

        if changed.is_empty() {
            tracing::debug!("granular reload: CSS updated, no window changes detected");
        } else {
            tracing::info!(
                "granular reload: rebuilding {} window(s): {:?}",
                changed.len(),
                changed
            );
            for name in &changed {
                self.close_window(name);
            }
            for name in &changed {
                if let Err(e) = self.open_window(name, false, None) {
                    tracing::warn!("Failed to reopen window {}: {}", name, e);
                }
            }
        }
        Ok(())
    }

    pub fn open_window(&mut self, name: &str, toggle: bool, monitor: Option<i32>) -> Result<()> {
        if self.open_windows.contains_key(name) {
            if toggle {
                self.close_window(name);
            }
            // Already open and not toggling — nothing to do.
            return Ok(());
        }

        let win_def = self
            .config
            .yuck
            .window_definitions
            .get(name)
            .ok_or_else(|| anyhow::anyhow!("No defwindow named `{}`", name))?
            .clone();

        let vars = &self.config.var_state.vars;
        let ctx = EvalCtx::new(vars, self.config.widget_defs.clone());

        let was_empty = self.open_windows.is_empty();
        let live = window::build_live_window(&win_def, &ctx, monitor)?;
        live.gtk_win.present();
        self.open_windows.insert(name.to_string(), live);
        if was_empty {
            self.windows_open.store(true, Ordering::Relaxed);
            // Flush any pending initial var values held in forward_var_updates.
            self.window_opened.notify_waiters();
        }
        Ok(())
    }

    pub fn close_window(&mut self, name: &str) {
        let closed = if let Some(live) = self.open_windows.remove(name) {
            // destroy(), not close(): GTK4 keeps every toplevel it created in an
            // internal registry until explicitly destroyed. close() only hides the
            // window, so repeated popup open/close leaks the whole widget tree.
            live.gtk_win.destroy();
            true
        } else {
            false
        };
        if self.open_windows.is_empty() {
            self.windows_open.store(false, Ordering::Relaxed);
            self.window_closed.notify_waiters();
            trim_heap();
        }
        if closed {
            // The closed popup's widget tree + decoded pixbufs have just been
            // dropped. glibc keeps that freed memory in its arenas (RSS only
            // grows), so image-heavy popups make the daemon climb without bound.
            // Trim here — not only when the map empties — because the bar window
            // stays open, so the map is never empty during normal popup use.
            trim_heap();
        }
    }

    pub fn close_all(&mut self) {
        let names: Vec<String> = self.open_windows.keys().cloned().collect();
        for name in names {
            self.close_window(&name);
        }
        self.pending_reopen.clear();
    }

    /// Called when GDK's monitor list changes (monitors connected or disconnected).
    ///
    /// Closes any window whose assigned monitor is no longer present and queues
    /// it for reopening. Reopens any queued window whose monitor just came back.
    pub fn handle_monitors_changed(&mut self) {
        let current: std::collections::HashSet<String> = gtk4::gdk::Display::default()
            .map(|d| {
                let list = d.monitors();
                (0..list.n_items())
                    .filter_map(|i| list.item(i)?.downcast::<gtk4::gdk::Monitor>().ok())
                    .filter_map(|m| m.connector().map(|c| c.to_string()))
                    .collect()
            })
            .unwrap_or_default();

        // Close windows whose monitor disappeared.
        let lost: Vec<(String, String)> = self
            .open_windows
            .iter()
            .filter_map(|(name, live)| {
                live.monitor_connector
                    .as_ref()
                    .filter(|c| !current.contains(*c))
                    .map(|c| (name.clone(), c.clone()))
            })
            .collect();

        for (name, connector) in lost {
            tracing::info!(
                "monitor `{}` disconnected — closing window `{}`",
                connector,
                name
            );
            self.close_window(&name);
            self.pending_reopen.push((name, connector));
        }

        // Reopen windows whose monitor came back.
        let to_reopen: Vec<(String, String)> = self
            .pending_reopen
            .iter()
            .filter(|(_, connector)| current.contains(connector))
            .cloned()
            .collect();

        for (name, connector) in &to_reopen {
            // Find the current numeric index for this connector.
            let idx = gtk4::gdk::Display::default()
                .and_then(|d| {
                    let list = d.monitors();
                    (0..list.n_items()).find(|&i| {
                        list.item(i)
                            .and_then(|o| o.downcast::<gtk4::gdk::Monitor>().ok())
                            .and_then(|m| m.connector())
                            .map(|c| c.as_str() == connector.as_str())
                            .unwrap_or(false)
                    })
                })
                .map(|i| i as i32);

            tracing::info!(
                "monitor `{}` reconnected — reopening window `{}`",
                connector,
                name
            );
            if let Err(e) = self.open_window(name, false, idx) {
                tracing::warn!(
                    "failed to reopen window `{}` on monitor `{}`: {}",
                    name,
                    connector,
                    e
                );
            }
        }

        self.pending_reopen
            .retain(|(name, _)| !to_reopen.iter().any(|(n, _)| n == name));
    }

    /// Reactive update: re-evaluate bindings whose expressions reference changed vars.
    pub fn update_bindings(&mut self, changed_vars: &HashSet<VarName>) {
        let global_vars = &self.config.var_state.vars;
        for live in self.open_windows.values_mut() {
            for binding in &mut live.bindings {
                if binding.intersects(changed_vars) {
                    binding.update_matching(changed_vars, global_vars);
                }
            }
        }
    }

    /// Full rebuild of all open window contents (used on `meh reload` or `meh update`).
    /// Rebuilds the widget tree from scratch and resets bindings.
    pub fn rebuild_open_windows(&mut self) {
        let names: Vec<String> = self.open_windows.keys().cloned().collect();
        for name in &names {
            self.close_window(name);
            if let Err(e) = self.open_window(name, false, None) {
                tracing::warn!("Failed to rebuild window {}: {}", name, e);
            }
        }
    }

    pub fn update_vars(&mut self, vars: &HashMap<String, String>) {
        let mut changed: HashSet<VarName> = HashSet::with_capacity(vars.len());
        for (k, v) in vars {
            let name = VarName(k.clone());
            if self.config.var_state.set(name.clone(), DynVal::from_string(v.clone())) {
                changed.insert(name);
            }
        }
        if !changed.is_empty() {
            self.update_bindings(&changed);
        }
    }

    pub fn handle_cmd(&mut self, cmd: Cmd) {
        match cmd {
            Cmd::Open {
                window,
                toggle,
                monitor,
                resp,
            } => {
                tracing::info!("handle_cmd: Open {}", window);
                let r = self
                    .open_window(&window, toggle, monitor)
                    .map(|_| IpcResponse::ok_empty())
                    .unwrap_or_else(|e| IpcResponse::err(e.to_string()));
                tracing::info!("handle_cmd: Open {} done: {:?}", window, r);
                let _ = resp.send(r);
            }
            Cmd::Close { windows, resp } => {
                for w in &windows {
                    self.close_window(w);
                }
                let _ = resp.send(IpcResponse::ok_empty());
            }
            Cmd::CloseAll { resp } => {
                self.close_all();
                let _ = resp.send(IpcResponse::ok_empty());
            }
            Cmd::Reload { resp } => {
                let r = self
                    .reload_config()
                    .map(|_| IpcResponse::ok("Reloaded."))
                    .unwrap_or_else(|e| IpcResponse::err(e.to_string()));
                let _ = resp.send(r);
            }
            Cmd::Update { vars, resp } => {
                self.update_vars(&vars);
                let _ = resp.send(IpcResponse::ok_empty());
            }
            Cmd::State { resp } => {
                let state = self
                    .config
                    .var_state
                    .vars
                    .iter()
                    .map(|(k, v)| format!("{} = {:?}", k, v.0))
                    .collect::<Vec<_>>()
                    .join("\n");
                let _ = resp.send(IpcResponse::ok(state));
            }
            Cmd::Get { var, resp } => match self.config.var_state.vars.get(&VarName(var.clone())) {
                Some(v) => {
                    let _ = resp.send(IpcResponse::ok(v.0.clone()));
                }
                None => {
                    let _ = resp.send(IpcResponse::err(format!("variable '{}' not found", var)));
                }
            },
            Cmd::ListWindows { resp } => {
                let names = self
                    .open_windows
                    .keys()
                    .cloned()
                    .collect::<Vec<_>>()
                    .join("\n");
                let _ = resp.send(IpcResponse::ok(names));
            }
            Cmd::SetVarBatch { vars } => {
                let mut changed: HashSet<VarName> = HashSet::with_capacity(vars.len());
                for (name, val) in vars {
                    if self.config.var_state.set(name.clone(), val) {
                        changed.insert(name);
                    }
                }
                if !changed.is_empty() {
                    self.update_bindings(&changed);
                }
            }
            Cmd::MonitorsChanged => {
                self.handle_monitors_changed();
            }
            Cmd::Kill => {
                self.close_all();
                self.main_loop.quit();
            }
        }
    }
}

use gtk4::gdk;

// ── Platform integration ──────────────────────────────────────────────────────

/// Call once after `gtk4::init()`. Returns the initial system dark-mode flag
/// using GTK settings only — libadwaita is deferred until an animation widget
/// or `connect_color_scheme` needs it.
#[cfg(feature = "animations")]
pub fn init_platform() -> Option<bool> {
    gtk4::Settings::default().map(|s| s.is_gtk_application_prefer_dark_theme())
}

#[cfg(not(feature = "animations"))]
pub fn init_platform() -> Option<bool> {
    None
}

/// Wire the system color-scheme change signal.  When the OS switches
/// dark/light, a `SetVarBatch` updating `MEH_DARK` is sent through `cmd_tx`.
/// No-op without the `animations` feature.
#[cfg(feature = "animations")]
pub fn connect_color_scheme(cmd_tx: tokio::sync::mpsc::UnboundedSender<Cmd>) {
    use gtk4::glib::prelude::ObjectExt;
    crate::widgets::ensure_adw_init();
    libadwaita::StyleManager::default().connect_notify_local(Some("dark"), move |mgr, _| {
        let val = DynVal::from_string(if mgr.is_dark() { "true" } else { "false" }.to_string());
        let mut map = HashMap::new();
        map.insert(VarName("MEH_DARK".to_string()), val);
        let _ = cmd_tx.send(Cmd::SetVarBatch { vars: map });
    });
}

#[cfg(not(feature = "animations"))]
pub fn connect_color_scheme(_cmd_tx: tokio::sync::mpsc::UnboundedSender<Cmd>) {}

// ── Granular hot-reload helpers ───────────────────────────────────────────────

#[cfg(feature = "granular-reload")]
fn strip_spans(val: &mut serde_json::Value) {
    match val {
        serde_json::Value::Object(map) => {
            let span_keys: Vec<String> = map
                .keys()
                .filter(|k| k.ends_with("span"))
                .cloned()
                .collect();
            for k in span_keys {
                map.remove(&k);
            }
            for v in map.values_mut() {
                strip_spans(v);
            }
        }
        serde_json::Value::Array(arr) => {
            for v in arr.iter_mut() {
                strip_spans(v);
            }
        }
        _ => {}
    }
}

#[cfg(feature = "granular-reload")]
fn collect_deps(
    wu: &yuck::config::widget_use::WidgetUse,
    defs: &std::collections::HashMap<String, yuck::config::widget_definition::WidgetDefinition>,
    visited: &mut std::collections::BTreeSet<String>,
    result: &mut std::collections::BTreeMap<
        String,
        yuck::config::widget_definition::WidgetDefinition,
    >,
) {
    use yuck::config::widget_use::WidgetUse;
    match wu {
        WidgetUse::Basic(b) => {
            if let Some(def) = defs.get(&b.name)
                && visited.insert(b.name.clone())
            {
                result.insert(b.name.clone(), def.clone());
                // Recurse into the widget body to find nested custom widgets.
                collect_deps(&def.widget, defs, visited, result);
            }
            for child in &b.children {
                collect_deps(child, defs, visited, result);
            }
        }
        WidgetUse::Loop(l) => {
            collect_deps(&l.body, defs, visited, result);
        }
        WidgetUse::Children(_) => {}
    }
}

/// Compute a stable semantic hash for a window and all its transitive widget
/// dependencies.  Span fields are stripped before hashing so that comment or
/// whitespace edits in unrelated parts of the config don't trigger a rebuild.
#[cfg(feature = "granular-reload")]
fn ir_hash(name: &str, config: &MehConfig) -> u64 {
    use std::{
        collections::{BTreeMap, BTreeSet},
        hash::{DefaultHasher, Hash, Hasher},
    };

    let Some(win_def) = config.yuck.window_definitions.get(name) else {
        return 0;
    };

    let mut visited = BTreeSet::new();
    let mut deps = BTreeMap::new();
    collect_deps(
        &win_def.widget,
        config.widget_defs.as_ref(),
        &mut visited,
        &mut deps,
    );

    let mut json_win = serde_json::to_value(win_def).unwrap_or_default();
    strip_spans(&mut json_win);

    let dep_vals: Vec<serde_json::Value> = deps
        .values()
        .map(|d| {
            let mut v = serde_json::to_value(d).unwrap_or_default();
            strip_spans(&mut v);
            v
        })
        .collect();

    let combined = serde_json::json!({ "window": json_win, "deps": dep_vals });
    let mut hasher = DefaultHasher::new();
    combined.to_string().hash(&mut hasher);
    hasher.finish()
}

// ── Heap trimming ───────────────────────────────────────────────────────────

/// Return free heap pages to the OS after popups close.
///
/// GTK image widgets decode pixbufs on the heap; when a popup is destroyed those
/// allocations are freed, but glibc's allocator retains the pages in its arenas,
/// so resident memory only ever grows as image-heavy popups are opened. A single
/// `malloc_trim(0)` releases the top-of-arena free space back to the kernel.
/// No-op on non-glibc targets.
#[cfg(all(target_os = "linux", target_env = "gnu"))]
fn trim_heap() {
    // Safety: malloc_trim is thread-safe and has no preconditions.
    unsafe {
        libc::malloc_trim(0);
    }
}

#[cfg(not(all(target_os = "linux", target_env = "gnu")))]
fn trim_heap() {}
