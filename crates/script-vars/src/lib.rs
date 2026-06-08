// GPL-3.0-or-later
//! Script variable sources: poll, listen, and (opt-in) subscribe.

use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
};

use anyhow::Result;
use eww_shared_util::VarName;
use simplexpr::dynval::DynVal;
use tokio::sync::{Notify, mpsc::UnboundedSender};
use yuck::config::script_var_definition::{
    ListenScriptVar, PollScriptVar, ScriptVarDefinition, SubscribeScriptVar, VarSource,
};

pub type VarUpdate = (VarName, DynVal);

/// Manages script-var tasks; call [`ScriptVarSupervisor::restart`] after config reload.
pub struct ScriptVarSupervisor {
    generation: Arc<AtomicUsize>,
    pub var_tx: UnboundedSender<VarUpdate>,
}

fn generation_stale(generation: &Arc<AtomicUsize>, my_gen: usize) -> bool {
    generation.load(Ordering::SeqCst) != my_gen
}

/// Clear cached Rhai ASTs so the next poll/listen/widget call recompiles from disk.
#[cfg(feature = "rhai")]
pub fn invalidate_rhai_cache() {
    if let Some(engine) = meh_rhai_engine::global() {
        engine.invalidate_all();
    }
}

#[cfg(not(feature = "rhai"))]
pub fn invalidate_rhai_cache() {}

/// Start all script vars. Returns `(receiver, sender)` — the receiver feeds
/// `forward_var_updates`; the sender can be cloned into plugin-host so plugins
/// share the same update channel.
///
/// Poll and listen subprocesses are gated on `windows_open`: no subprocess runs
/// while nothing is visible.  Subscribe vars (inotify / DBus) always run once
/// started — they carry near-zero idle cost (kernel-level wait) and keep the
/// value current so the first window open gets an up-to-date value immediately.
pub fn start_all(
    vars: &std::collections::HashMap<VarName, ScriptVarDefinition>,
    shutdown: tokio::sync::broadcast::Receiver<()>,
    windows_open: Arc<AtomicBool>,
    window_opened: Arc<Notify>,
    window_closed: Arc<Notify>,
    config_dir: PathBuf,
) -> (
    ScriptVarSupervisor,
    tokio::sync::mpsc::UnboundedReceiver<VarUpdate>,
) {
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<VarUpdate>();
    let supervisor = ScriptVarSupervisor {
        generation: Arc::new(AtomicUsize::new(1)),
        var_tx: tx,
    };
    supervisor.spawn_tasks(
        vars,
        shutdown,
        windows_open,
        window_opened,
        window_closed,
        config_dir,
        1,
    );
    (supervisor, rx)
}

impl ScriptVarSupervisor {
    /// Stop all tasks from the previous generation and spawn fresh ones from `vars`.
    pub fn restart(
        &self,
        vars: &std::collections::HashMap<VarName, ScriptVarDefinition>,
        shutdown: tokio::sync::broadcast::Receiver<()>,
        windows_open: Arc<AtomicBool>,
        window_opened: Arc<Notify>,
        window_closed: Arc<Notify>,
        config_dir: PathBuf,
    ) {
        let my_gen = self.generation.fetch_add(1, Ordering::SeqCst) + 1;
        tracing::info!("script-vars: restarting generation {my_gen} ({} vars)", vars.len());
        self.spawn_tasks(
            vars,
            shutdown,
            windows_open,
            window_opened,
            window_closed,
            config_dir,
            my_gen,
        );
    }

    fn spawn_tasks(
        &self,
        vars: &std::collections::HashMap<VarName, ScriptVarDefinition>,
        shutdown: tokio::sync::broadcast::Receiver<()>,
        windows_open: Arc<AtomicBool>,
        window_opened: Arc<Notify>,
        window_closed: Arc<Notify>,
        config_dir: PathBuf,
        my_gen: usize,
    ) {
        #[cfg(feature = "rhai")]
        if my_gen == 1 && vars_need_rhai(vars) {
            meh_rhai_engine::init();
        }

        if my_gen == 1 {
            kill_orphaned_scripts(&config_dir);
        }

        tracing::debug!(
            "spawn_tasks gen={my_gen}: {} vars in {}",
            vars.len(),
            config_dir.display()
        );
        let generation = self.generation.clone();
        for def in vars.values() {
            match def {
                ScriptVarDefinition::Poll(p) => {
                    let p = p.clone();
                    let tx = self.var_tx.clone();
                    let shutdown = shutdown.resubscribe();
                    let wo = windows_open.clone();
                    let dir = config_dir.clone();
                    let generation_arc = generation.clone();
                    tokio::spawn(run_poll(p, tx, shutdown, wo, dir, generation_arc, my_gen));
                }
                ScriptVarDefinition::Listen(l) => {
                    let l = l.clone();
                    let tx = self.var_tx.clone();
                    let shutdown = shutdown.resubscribe();
                    let dir = config_dir.clone();
                    let generation_arc = generation.clone();
                    tokio::spawn(run_listen(
                        l,
                        tx,
                        shutdown,
                        windows_open.clone(),
                        window_opened.clone(),
                        window_closed.clone(),
                        dir,
                        generation_arc,
                        my_gen,
                    ));
                }
                ScriptVarDefinition::Subscribe(s) => {
                    let s = s.clone();
                    let tx = self.var_tx.clone();
                    let shutdown = shutdown.resubscribe();
                    let generation_arc = generation.clone();
                    tokio::spawn(run_subscribe(s, tx, shutdown, generation_arc, my_gen));
                }
            }
        }
    }
}

// ── Poll ──────────────────────────────────────────────────────────────────────

async fn run_poll(
    def: PollScriptVar,
    tx: UnboundedSender<VarUpdate>,
    mut shutdown: tokio::sync::broadcast::Receiver<()>,
    windows_open: Arc<AtomicBool>,
    config_dir: PathBuf,
    generation: Arc<AtomicUsize>,
    my_gen: usize,
) {
    let cmd = match &def.command {
        VarSource::Shell(_, s) => s.clone(),
        VarSource::Function(f) => {
            if let Ok(v) = f() {
                let _ = tx.send((def.name.clone(), v));
            }
            return;
        }
    };

    // Always fetch an initial value to populate var_state before any window opens.
    let mut last: Option<String> = None;
    if let Ok(out) = run_source(&cmd, &config_dir).await {
        let _ = tx.send((def.name.clone(), DynVal::from_string(out.clone())));
        last = Some(out);
    }

    let mut timer = tokio::time::interval(def.interval);
    timer.tick().await; // skip the first (immediate) tick

    loop {
        if generation_stale(&generation, my_gen) {
            break;
        }
        tokio::select! {
            _ = shutdown.recv() => break,
            _ = timer.tick() => {
                if !windows_open.load(Ordering::Relaxed) {
                    continue;
                }
                match run_source(&cmd, &config_dir).await {
                    Ok(out) => {
                        if last.as_deref() != Some(&out) {
                            let _ = tx.send((def.name.clone(), DynVal::from_string(out.clone())));
                            last = Some(out);
                        }
                    }
                    Err(e) => tracing::warn!("poll var `{}` error: {}", def.name, e),
                }
            }
        }
    }
}

// ── Listen ────────────────────────────────────────────────────────────────────

async fn run_listen(
    def: ListenScriptVar,
    tx: UnboundedSender<VarUpdate>,
    mut shutdown: tokio::sync::broadcast::Receiver<()>,
    windows_open: Arc<AtomicBool>,
    window_opened: Arc<Notify>,
    window_closed: Arc<Notify>,
    config_dir: PathBuf,
    generation: Arc<AtomicUsize>,
    my_gen: usize,
) {
    use tokio::io::{AsyncBufReadExt, BufReader};

    let _ = tx.send((def.name.clone(), def.initial_value.clone()));

    // Rhai listen: poll-style loop — gated like defpoll when no windows are open.
    if is_rhai_source(&def.command) {
        let mut last: Option<String> = None;
        if let Ok(out) = run_rhai(&def.command, &config_dir) {
            let _ = tx.send((def.name.clone(), DynVal::from_string(out.clone())));
            last = Some(out);
        }
        let mut timer = tokio::time::interval(std::time::Duration::from_secs(1));
        timer.tick().await;
        loop {
            if generation_stale(&generation, my_gen) {
                return;
            }
            tokio::select! {
                _ = shutdown.recv() => return,
                _ = timer.tick() => {
                    if !windows_open.load(Ordering::Relaxed) {
                        continue;
                    }
                    match run_rhai(&def.command, &config_dir) {
                        Ok(out) => {
                            if last.as_deref() != Some(&out) {
                                let _ = tx.send((def.name.clone(), DynVal::from_string(out.clone())));
                                last = Some(out);
                            }
                        }
                        Err(e) => tracing::warn!("listen var `{}` rhai error: {}", def.name, e),
                    }
                }
            }
        }
    }

    // Shell listen: long-running subprocess while windows are open. When the last
    // window closes the child (and its process group) is killed and restarted on
    // the next open — same gating model as defpoll, without leaving orphans.
    loop {
        if generation_stale(&generation, my_gen) {
            return;
        }
        while !windows_open.load(Ordering::Relaxed) {
            if generation_stale(&generation, my_gen) {
                return;
            }
            tokio::select! {
                _ = shutdown.recv() => return,
                _ = window_opened.notified() => {}
            }
        }

        let mut child = match tokio::process::Command::new("sh")
            .arg("-c")
            .arg(&def.command)
            .current_dir(&config_dir)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .process_group(0)
            .spawn()
        {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!("listen var `{}` spawn failed: {}", def.name, e);
                tokio::select! {
                    _ = shutdown.recv() => return,
                    _ = tokio::time::sleep(std::time::Duration::from_secs(5)) => {}
                }
                continue;
            }
        };

        let stdout = child.stdout.take().expect("piped stdout");
        let mut lines = BufReader::new(stdout).lines();
        let mut keep_running = true;

        while keep_running {
            tokio::select! {
                _ = shutdown.recv() => {
                    kill_group(&mut child);
                    let _ = child.kill().await;
                    return;
                }
                _ = window_closed.notified() => {
                    if !windows_open.load(Ordering::Relaxed) {
                        kill_group(&mut child);
                        let _ = child.kill().await;
                        let _ = child.wait().await;
                        keep_running = false;
                    }
                }
                line = lines.next_line() => {
                    match line {
                        Ok(Some(l)) => {
                            if windows_open.load(Ordering::Relaxed) {
                                let _ = tx.send((def.name.clone(), DynVal::from_string(l)));
                            }
                        }
                        Ok(None) | Err(_) => {
                            kill_group(&mut child);
                            let _ = child.wait().await;
                            keep_running = false;
                        }
                    }
                }
            }
        }

        if windows_open.load(Ordering::Relaxed) {
            tokio::select! {
                _ = shutdown.recv() => return,
                _ = tokio::time::sleep(std::time::Duration::from_millis(500)) => {}
            }
        }
    }
}

// ── Subscribe ─────────────────────────────────────────────────────────────────

async fn run_subscribe(
    def: SubscribeScriptVar,
    tx: UnboundedSender<VarUpdate>,
    shutdown: tokio::sync::broadcast::Receiver<()>,
    generation: Arc<AtomicUsize>,
    my_gen: usize,
) {
    use yuck::config::script_var_definition::SubscribeSource;

    #[cfg(not(any(feature = "inotify-vars", feature = "dbus-vars")))]
    let _ = (generation, my_gen);

    // Always emit the initial value immediately so var_state is populated.
    let _ = tx.send((def.name.clone(), def.initial_value.clone()));

    match &def.source {
        SubscribeSource::File { .. } => {
            #[cfg(feature = "inotify-vars")]
            {
                if let Err(e) = run_subscribe_file(def, tx, shutdown, generation, my_gen).await {
                    tracing::warn!("subscribe file error: {e}");
                }
            }
            #[cfg(not(feature = "inotify-vars"))]
            {
                let _ = (tx, shutdown);
                tracing::warn!(
                    "subscribe var `{}` uses :file but meh was built without `inotify-vars` feature",
                    def.name
                );
            }
        }
        SubscribeSource::Dbus { .. } => {
            #[cfg(feature = "dbus-vars")]
            {
                if let Err(e) = run_subscribe_dbus(def, tx, shutdown, generation, my_gen).await {
                    tracing::warn!("subscribe dbus error: {e}");
                }
            }
            #[cfg(not(feature = "dbus-vars"))]
            {
                let _ = (tx, shutdown);
                tracing::warn!(
                    "subscribe var `{}` uses :dbus-service but meh was built without `dbus-vars` feature",
                    def.name
                );
            }
        }
    }
}

// ── inotify implementation ────────────────────────────────────────────────────

/// Expand a leading `~/` or bare `~` to the user's home directory.
/// `~username` forms are left unchanged (uncommon, not needed here).
#[cfg(feature = "inotify-vars")]
fn expand_tilde(path: &str) -> std::path::PathBuf {
    if path == "~" {
        return std::env::var("HOME")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|_| std::path::PathBuf::from(path));
    }
    if let Some(rest) = path.strip_prefix("~/")
        && let Ok(home) = std::env::var("HOME")
    {
        return std::path::PathBuf::from(home).join(rest);
    }
    std::path::PathBuf::from(path)
}

/// Two-phase watcher state:
/// - `File`   — watching the target file directly (normal case)
/// - `Parent` — target doesn't exist yet; watching the parent dir for creation
#[cfg(feature = "inotify-vars")]
#[derive(Clone, Copy, PartialEq)]
enum WatchState {
    File,
    Parent,
}

#[cfg(feature = "inotify-vars")]
async fn run_subscribe_file(
    def: SubscribeScriptVar,
    tx: UnboundedSender<VarUpdate>,
    mut shutdown: tokio::sync::broadcast::Receiver<()>,
    generation: Arc<AtomicUsize>,
    my_gen: usize,
) -> Result<()> {
    use notify::{Config, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
    use yuck::config::script_var_definition::SubscribeSource;

    let SubscribeSource::File { path } = &def.source else {
        unreachable!()
    };
    let path = expand_tilde(path);

    // Bridge notify's sync callback into an async channel.
    let (event_tx, mut event_rx) =
        tokio::sync::mpsc::unbounded_channel::<notify::Result<notify::Event>>();
    let mut watcher = RecommendedWatcher::new(
        {
            let event_tx = event_tx.clone();
            move |res| {
                let _ = event_tx.send(res);
            }
        },
        Config::default(),
    )?;

    // Determine initial watch state. The :initial value is already emitted by
    // run_subscribe() before this function is called, so we only need to emit
    // the live file content when the file actually exists.
    let mut state = if path.exists() {
        watcher.watch(&path, RecursiveMode::NonRecursive)?;
        emit_file_contents(&def.name, &path, &tx).await;
        WatchState::File
    } else {
        watch_parent_dir(&mut watcher, &path)?;
        WatchState::Parent
    };

    loop {
        if generation_stale(&generation, my_gen) {
            break;
        }
        tokio::select! {
            _ = shutdown.recv() => break,
            msg = event_rx.recv() => {
                let evt = match msg {
                    Some(Ok(e))  => e,
                    Some(Err(e)) => { tracing::warn!("inotify error `{}`: {e}", def.name); continue; }
                    None         => break,
                };

                match evt.kind {
                    // File was removed (or renamed away) while we were watching it directly.
                    // Transition back to parent-dir watch so we catch when it reappears.
                    EventKind::Remove(_) if state == WatchState::File => {
                        let _ = watcher.unwatch(&path);
                        if let Err(e) = watch_parent_dir(&mut watcher, &path) {
                            tracing::warn!("subscribe `{}`: lost file, can't watch parent: {e}", def.name);
                        } else {
                            state = WatchState::Parent;
                            tracing::debug!("subscribe `{}`: file removed, watching parent", def.name);
                        }
                        continue;
                    }

                    // File created or modified — proceed to emit.
                    EventKind::Modify(_) | EventKind::Create(_) => {}

                    // Anything else (access, metadata, etc.) — ignore.
                    _ => continue,
                }

                // While watching the parent dir, check if our file just appeared.
                if state == WatchState::Parent
                    && evt.paths.iter().any(|p| p == &path)
                    && path.exists()
                {
                    // Transition: unwatch parent, watch file.
                    if let Some(parent) = path.parent() { let _ = watcher.unwatch(parent); }
                    match watcher.watch(&path, RecursiveMode::NonRecursive) {
                        Ok(())   => state = WatchState::File,
                        Err(e)   => tracing::warn!("subscribe `{}`: re-watch after create: {e}", def.name),
                    }
                }

                emit_file_contents(&def.name, &path, &tx).await;
            }
        }
    }
    Ok(())
}

/// Watch the parent directory of `path` so we get notified when `path` is created.
/// Logs a warning (non-fatal) if the parent itself doesn't exist.
#[cfg(feature = "inotify-vars")]
fn watch_parent_dir(watcher: &mut impl notify::Watcher, path: &std::path::Path) -> Result<()> {
    let parent = path.parent().unwrap_or(std::path::Path::new("."));
    if !parent.exists() {
        tracing::warn!(
            "defsubscribe :file `{}` — parent dir `{}` does not exist; \
             var will remain at :initial until it is created",
            path.display(),
            parent.display(),
        );
        return Ok(());
    }
    watcher.watch(parent, notify::RecursiveMode::NonRecursive)?;
    Ok(())
}

#[cfg(feature = "inotify-vars")]
async fn emit_file_contents(
    name: &eww_shared_util::VarName,
    path: &std::path::Path,
    tx: &UnboundedSender<VarUpdate>,
) {
    match tokio::fs::read_to_string(path).await {
        Ok(s) => {
            let _ = tx.send((name.clone(), DynVal::from_string(s.trim_end().to_string())));
        }
        Err(e) => tracing::warn!("defsubscribe :file read `{}`: {e}", path.display()),
    }
}

// ── DBus implementation ───────────────────────────────────────────────────────

#[cfg(feature = "dbus-vars")]
async fn run_subscribe_dbus(
    def: SubscribeScriptVar,
    tx: UnboundedSender<VarUpdate>,
    mut shutdown: tokio::sync::broadcast::Receiver<()>,
    generation: Arc<AtomicUsize>,
    my_gen: usize,
) -> Result<()> {
    use futures::StreamExt;
    use yuck::config::script_var_definition::{DbusKind, SubscribeSource};
    use zbus::{MatchRule, MessageStream};

    let SubscribeSource::Dbus {
        bus,
        service,
        object,
        interface,
        property,
    } = &def.source
    else {
        unreachable!()
    };

    let conn = match bus {
        DbusKind::System => zbus::Connection::system().await,
        DbusKind::Session => zbus::Connection::session().await,
    }?;

    // Read and emit the initial property value.
    match get_dbus_property(&conn, service, object, interface, property).await {
        Ok(val) => {
            let _ = tx.send((def.name.clone(), val));
        }
        Err(e) => tracing::warn!("subscribe var `{}` initial get failed: {e}", def.name),
    }

    // Subscribe to PropertiesChanged on the specific service + object.
    let rule = MatchRule::builder()
        .msg_type(zbus::MessageType::Signal)
        .sender(service.as_str())?
        .path(object.as_str())?
        .interface("org.freedesktop.DBus.Properties")?
        .member("PropertiesChanged")?
        .build();

    let mut stream = MessageStream::for_match_rule(rule, &conn, None).await?;

    loop {
        if generation_stale(&generation, my_gen) {
            break;
        }
        tokio::select! {
            _ = shutdown.recv() => break,
            msg = stream.next() => {
                let msg = match msg {
                    Some(Ok(m)) => m,
                    Some(Err(e)) => {
                        tracing::warn!("subscribe dbus stream error for `{}`: {e}", def.name);
                        continue;
                    }
                    None => break,
                };

                // Body signature: (sa{sv}as)
                // s  = interface name
                // a{sv} = changed_properties
                // as = invalidated_properties
                type ChangedBody = (
                    String,
                    std::collections::HashMap<String, zbus::zvariant::OwnedValue>,
                    Vec<String>,
                );
                let (sig_iface, changed, invalidated): ChangedBody = match msg.body::<ChangedBody>() {
                    Ok(v) => v,
                    Err(_) => continue,
                };

                if sig_iface != *interface {
                    continue;
                }

                if let Some(val) = changed.get(property.as_str()) {
                    let _ = tx.send((def.name.clone(), dynval_from_zvariant(val)));
                } else if invalidated.iter().any(|p| p == property) {
                    // Property invalidated — re-read via Get.
                    match get_dbus_property(&conn, service, object, interface, property).await {
                        Ok(val) => {
                            let _ = tx.send((def.name.clone(), val));
                        }
                        Err(e) => tracing::warn!("subscribe var `{}` re-read failed: {e}", def.name),
                    }
                }
            }
        }
    }
    Ok(())
}

#[cfg(feature = "dbus-vars")]
async fn get_dbus_property(
    conn: &zbus::Connection,
    service: &str,
    object: &str,
    interface: &str,
    property: &str,
) -> Result<DynVal> {
    let reply = conn
        .call_method(
            Some(service),
            object,
            Some("org.freedesktop.DBus.Properties"),
            "Get",
            &(interface, property),
        )
        .await?;
    let val: zbus::zvariant::OwnedValue = reply.body::<zbus::zvariant::OwnedValue>()?;
    Ok(dynval_from_zvariant(&val))
}

/// Convert a zbus `Value` (or `OwnedValue` via Deref) to a `DynVal` string.
/// Complex types (arrays, dicts, structs) return empty string — use `deflisten`
/// with dbus-monitor for those.
#[cfg(feature = "dbus-vars")]
fn dynval_from_zvariant(val: &zbus::zvariant::Value<'_>) -> DynVal {
    use zbus::zvariant::Value;
    DynVal::from_string(match val {
        Value::U8(v) => v.to_string(),
        Value::Bool(v) => v.to_string(),
        Value::I16(v) => v.to_string(),
        Value::U16(v) => v.to_string(),
        Value::I32(v) => v.to_string(),
        Value::U32(v) => v.to_string(),
        Value::I64(v) => v.to_string(),
        Value::U64(v) => v.to_string(),
        Value::F64(v) => v.to_string(),
        Value::Str(v) => v.to_string(),
        Value::ObjectPath(v) => v.to_string(),
        // Unwrap nested variant (Properties.Get returns v wrapping the real type)
        Value::Value(inner) => return dynval_from_zvariant(inner),
        _ => String::new(),
    })
}

// ── Helpers ───────────────────────────────────────────────────────────────────

/// On daemon startup, kill any processes whose cmdline contains the config
/// scripts directory — these are orphans from a previous daemon run that was
/// killed with SIGKILL or crashed before it could clean up its children.
fn kill_orphaned_scripts(config_dir: &std::path::Path) {
    let scripts_dir = config_dir.join("scripts");
    let needle = scripts_dir.to_string_lossy().into_owned();
    // inotifywait for /tmp/meh/ triggers (e.g. cal_trigger) don't contain the
    // scripts dir in cmdline, so match on the tmp dir too.
    let needle2 = "/tmp/meh/".to_string();

    let Ok(proc) = std::fs::read_dir("/proc") else {
        return;
    };
    for entry in proc.flatten() {
        let name = entry.file_name();
        if !name.to_string_lossy().chars().all(|c| c.is_ascii_digit()) {
            continue;
        }
        let Ok(pid_n) = name.to_string_lossy().parse::<i32>() else {
            continue;
        };

        let cmdline_path = format!("/proc/{}/cmdline", pid_n);
        let Ok(raw) = std::fs::read(&cmdline_path) else {
            continue;
        };
        // /proc/<pid>/cmdline is NUL-separated; convert to spaces for matching.
        let cmdline = raw
            .iter()
            .map(|&b| if b == 0 { b' ' } else { b })
            .collect::<Vec<_>>();
        let cmdline = String::from_utf8_lossy(&cmdline);

        if cmdline.contains(needle.as_str()) || cmdline.contains(needle2.as_str()) {
            let pid = nix::unistd::Pid::from_raw(pid_n);
            // SIGKILL — orphans are leftovers from a crashed/killed daemon; no
            // graceful shutdown needed. SIGTERM is often ignored by shells blocked
            // in inotifywait or similar kernel waits.
            let _ = nix::sys::signal::kill(pid, nix::sys::signal::Signal::SIGKILL);
            let _ = nix::sys::signal::killpg(pid, nix::sys::signal::Signal::SIGKILL);
        }
    }
}

/// Send SIGTERM to every process in the child's process group before the
/// caller follows up with SIGKILL via `child.kill()`.  Because we spawn with
/// `.process_group(0)` the child's PGID equals its PID, so killpg reaches
/// grandchildren (inotifywait, playerctl --follow, nmcli monitor, …) that
/// would otherwise be orphaned when only the shell wrapper is killed.
fn kill_group(child: &mut tokio::process::Child) {
    if let Some(pid) = child.id() {
        let pgid = nix::unistd::Pid::from_raw(pid as i32);
        let _ = nix::sys::signal::killpg(pgid, nix::sys::signal::Signal::SIGTERM);
    }
}

/// Route to Rhai or shell based on the source string.
async fn run_source(cmd: &str, config_dir: &std::path::Path) -> Result<String> {
    if is_rhai_source(cmd) {
        tokio::task::spawn_blocking({
            let cmd = cmd.to_owned();
            let dir = config_dir.to_path_buf();
            move || run_rhai(&cmd, &dir)
        })
        .await?
    } else {
        run_shell(cmd, config_dir).await
    }
}

/// True when the source string refers to a Rhai script (`.rhai` extension or `rhai:` prefix).
fn is_rhai_source(cmd: &str) -> bool {
    let s = cmd.trim();
    s.ends_with(".rhai") || s.starts_with("rhai:")
}

/// True when any configured script var uses a Rhai poll/listen source.
#[cfg(feature = "rhai")]
fn vars_need_rhai(vars: &std::collections::HashMap<VarName, ScriptVarDefinition>) -> bool {
    vars.values().any(|def| match def {
        ScriptVarDefinition::Poll(p) => match &p.command {
            VarSource::Shell(_, s) => is_rhai_source(s),
            VarSource::Function(_) => false,
        },
        ScriptVarDefinition::Listen(l) => is_rhai_source(&l.command),
        ScriptVarDefinition::Subscribe(_) => false,
    })
}

/// Execute a Rhai source synchronously (call from `spawn_blocking`).
fn run_rhai(cmd: &str, config_dir: &std::path::Path) -> Result<String> {
    #[cfg(feature = "rhai")]
    {
        let engine = meh_rhai_engine::global().unwrap_or_else(|| meh_rhai_engine::init());
        let s = cmd.trim();
        if let Some(inline) = s.strip_prefix("rhai:") {
            engine.eval_inline(inline.trim())
        } else {
            engine.eval_file(std::path::Path::new(s), config_dir)
        }
    }
    #[cfg(not(feature = "rhai"))]
    {
        let _ = (cmd, config_dir);
        anyhow::bail!("meh2 built without `rhai` feature")
    }
}

async fn run_shell(cmd: &str, config_dir: &std::path::Path) -> Result<String> {
    let out = tokio::process::Command::new("sh")
        .arg("-c")
        .arg(cmd)
        .current_dir(config_dir)
        .output()
        .await?;
    Ok(String::from_utf8_lossy(&out.stdout).trim_end().to_string())
}
