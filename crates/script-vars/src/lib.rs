// GPL-3.0-or-later
//! Script variable sources: poll, listen, and (opt-in) subscribe.

use std::{
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
};

use anyhow::Result;
use eww_shared_util::VarName;
use simplexpr::dynval::DynVal;
use tokio::sync::{mpsc::UnboundedSender, Notify};
use yuck::config::script_var_definition::{
    ListenScriptVar, PollScriptVar, ScriptVarDefinition, SubscribeScriptVar, VarSource,
};

pub type VarUpdate = (VarName, DynVal);

/// Start all script vars. Returns a channel that emits (VarName, DynVal) updates.
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
) -> tokio::sync::mpsc::UnboundedReceiver<VarUpdate> {
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<VarUpdate>();

    // Kill any orphaned subprocesses left by a previous daemon run (e.g. after
    // SIGKILL or crash). Scans /proc for processes whose cmdline contains the
    // config scripts directory — those can only be our deflisten children.
    kill_orphaned_scripts(&config_dir);

    tracing::debug!("start_all: {} vars in {}", vars.len(), config_dir.display());
    for def in vars.values() {
        match def {
            ScriptVarDefinition::Poll(p) => {
                let p = p.clone();
                let tx = tx.clone();
                let shutdown = shutdown.resubscribe();
                let wo = windows_open.clone();
                let dir = config_dir.clone();
                tokio::spawn(run_poll(p, tx, shutdown, wo, dir));
            }
            ScriptVarDefinition::Listen(l) => {
                let l = l.clone();
                let tx = tx.clone();
                let shutdown = shutdown.resubscribe();
                let dir = config_dir.clone();
                tokio::spawn(run_listen(
                    l,
                    tx,
                    shutdown,
                    windows_open.clone(),
                    window_opened.clone(),
                    window_closed.clone(),
                    dir,
                ));
            }
            ScriptVarDefinition::Subscribe(s) => {
                let s = s.clone();
                let tx = tx.clone();
                let shutdown = shutdown.resubscribe();
                tokio::spawn(run_subscribe(s, tx, shutdown));
            }
        }
    }

    rx
}

// ── Poll ──────────────────────────────────────────────────────────────────────

async fn run_poll(
    def: PollScriptVar,
    tx: UnboundedSender<VarUpdate>,
    mut shutdown: tokio::sync::broadcast::Receiver<()>,
    windows_open: Arc<AtomicBool>,
    config_dir: PathBuf,
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
    if let Ok(out) = run_shell(&cmd, &config_dir).await {
        let _ = tx.send((def.name.clone(), DynVal::from_string(out)));
    }

    let mut timer = tokio::time::interval(def.interval);
    timer.tick().await; // skip the first (immediate) tick

    loop {
        tokio::select! {
            _ = shutdown.recv() => break,
            _ = timer.tick() => {
                if !windows_open.load(Ordering::Relaxed) {
                    continue;
                }
                match run_shell(&cmd, &config_dir).await {
                    Ok(out) => { let _ = tx.send((def.name.clone(), DynVal::from_string(out))); }
                    Err(e)  => tracing::warn!("poll var `{}` error: {}", def.name, e),
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
    _windows_open: Arc<AtomicBool>,
    _window_opened: Arc<Notify>,
    _window_closed: Arc<Notify>,
    config_dir: PathBuf,
) {
    use tokio::io::{AsyncBufReadExt, BufReader};

    let _ = tx.send((def.name.clone(), def.initial_value.clone()));

    // Listen vars run for the lifetime of the daemon — no window gating.
    // Killing and restarting on every window open/close causes subprocess
    // accumulation; the subprocess is already cheap to keep alive.
    loop {
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
                // Back off before retrying so we don't spin on a bad command.
                tokio::select! {
                    _ = shutdown.recv() => return,
                    _ = tokio::time::sleep(std::time::Duration::from_secs(5)) => {}
                }
                continue;
            }
        };

        let stdout = child.stdout.take().unwrap();
        let mut lines = BufReader::new(stdout).lines();

        loop {
            tokio::select! {
                _ = shutdown.recv() => {
                    kill_group(&mut child);
                    let _ = child.kill().await;
                    return;
                }
                line = lines.next_line() => {
                    match line {
                        Ok(Some(l)) => { let _ = tx.send((def.name.clone(), DynVal::from_string(l))); }
                        Ok(None) | Err(_) => {
                            // Process exited — clean up and restart.
                            kill_group(&mut child);
                            let _ = child.wait().await;
                            break;
                        }
                    }
                }
            }
        }

        // Brief pause before restart to avoid spinning on a command that exits immediately.
        tokio::select! {
            _ = shutdown.recv() => return,
            _ = tokio::time::sleep(std::time::Duration::from_millis(500)) => {}
        }
    }
}

// ── Subscribe ─────────────────────────────────────────────────────────────────

async fn run_subscribe(
    def: SubscribeScriptVar,
    tx: UnboundedSender<VarUpdate>,
    shutdown: tokio::sync::broadcast::Receiver<()>,
) {
    use yuck::config::script_var_definition::SubscribeSource;

    // Always emit the initial value immediately so var_state is populated.
    let _ = tx.send((def.name.clone(), def.initial_value.clone()));

    match &def.source {
        SubscribeSource::File { .. } => {
            #[cfg(feature = "inotify-vars")]
            {
                if let Err(e) = run_subscribe_file(def, tx, shutdown).await {
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
                if let Err(e) = run_subscribe_dbus(def, tx, shutdown).await {
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

#[cfg(feature = "inotify-vars")]
async fn run_subscribe_file(
    def: SubscribeScriptVar,
    tx: UnboundedSender<VarUpdate>,
    mut shutdown: tokio::sync::broadcast::Receiver<()>,
) -> Result<()> {
    use notify::{Config, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
    use yuck::config::script_var_definition::SubscribeSource;

    let SubscribeSource::File { path } = &def.source else {
        unreachable!()
    };
    let path = std::path::PathBuf::from(path);

    // Read and emit the current file contents on startup.
    if let Ok(contents) = tokio::fs::read_to_string(&path).await {
        let _ = tx.send((def.name.clone(), DynVal::from_string(contents.trim_end().to_string())));
    }

    // Bridge notify's sync callback into an async channel.
    let (event_tx, mut event_rx) = tokio::sync::mpsc::unbounded_channel::<notify::Result<notify::Event>>();
    let mut watcher = RecommendedWatcher::new(
        move |res| {
            let _ = event_tx.send(res);
        },
        Config::default(),
    )?;
    watcher.watch(&path, RecursiveMode::NonRecursive)?;

    loop {
        tokio::select! {
            _ = shutdown.recv() => break,
            evt = event_rx.recv() => {
                let evt = match evt {
                    Some(Ok(e)) => e,
                    Some(Err(e)) => {
                        tracing::warn!("inotify error for `{}`: {e}", def.name);
                        continue;
                    }
                    None => break,
                };
                // Emit on any write/create/rename event.
                let relevant = matches!(
                    evt.kind,
                    EventKind::Modify(_) | EventKind::Create(_) | EventKind::Access(_)
                );
                if relevant {
                    match tokio::fs::read_to_string(&path).await {
                        Ok(contents) => {
                            let _ = tx.send((
                                def.name.clone(),
                                DynVal::from_string(contents.trim_end().to_string()),
                            ));
                        }
                        Err(e) => tracing::warn!("read `{}`: {e}", path.display()),
                    }
                }
            }
        }
    }
    Ok(())
}

// ── DBus implementation ───────────────────────────────────────────────────────

#[cfg(feature = "dbus-vars")]
async fn run_subscribe_dbus(
    def: SubscribeScriptVar,
    tx: UnboundedSender<VarUpdate>,
    mut shutdown: tokio::sync::broadcast::Receiver<()>,
) -> Result<()> {
    use futures::StreamExt;
    use yuck::config::script_var_definition::{DbusKind, SubscribeSource};
    use zbus::{MatchRule, MessageStream};

    let SubscribeSource::Dbus { bus, service, object, interface, property } = &def.source else {
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
        .call_method(Some(service), object, Some("org.freedesktop.DBus.Properties"), "Get", &(interface, property))
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
    let needle      = scripts_dir.to_string_lossy().into_owned();
    // inotifywait for /tmp/meh/ triggers (e.g. cal_trigger) don't contain the
    // scripts dir in cmdline, so match on the tmp dir too.
    let needle2     = "/tmp/meh/".to_string();

    let Ok(proc) = std::fs::read_dir("/proc") else { return };
    for entry in proc.flatten() {
        let name = entry.file_name();
        if !name.to_string_lossy().chars().all(|c| c.is_ascii_digit()) {
            continue;
        }
        let Ok(pid_n) = name.to_string_lossy().parse::<i32>() else { continue };

        let cmdline_path = format!("/proc/{}/cmdline", pid_n);
        let Ok(raw) = std::fs::read(&cmdline_path) else { continue };
        // /proc/<pid>/cmdline is NUL-separated; convert to spaces for matching.
        let cmdline = raw.iter().map(|&b| if b == 0 { b' ' } else { b }).collect::<Vec<_>>();
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

async fn run_shell(cmd: &str, config_dir: &std::path::Path) -> Result<String> {
    let out = tokio::process::Command::new("sh")
        .arg("-c")
        .arg(cmd)
        .current_dir(config_dir)
        .output()
        .await?;
    Ok(String::from_utf8_lossy(&out.stdout).trim_end().to_string())
}
