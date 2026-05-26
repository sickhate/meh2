// GPL-3.0-or-later
//! tokio runtime, unix-socket IPC server, and daemon lifecycle.

use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::Duration,
};

use anyhow::Result;
use eww_shared_util::VarName;
use meh_core::{ipc_read, ipc_write, IpcCmd, IpcResponse, MehConfig, MehPaths};
use simplexpr::dynval::DynVal;
use tokio::sync::mpsc::UnboundedSender;

// ── Application lifecycle ─────────────────────────────────────────────────────

static EXIT_SENDER: once_cell::sync::OnceCell<tokio::sync::broadcast::Sender<()>> =
    once_cell::sync::OnceCell::new();

fn exit_sender() -> &'static tokio::sync::broadcast::Sender<()> {
    EXIT_SENDER.get_or_init(|| tokio::sync::broadcast::channel(4).0)
}

pub fn send_exit() {
    let _ = exit_sender().send(());
}

async fn recv_exit() -> Result<()> {
    exit_sender()
        .subscribe()
        .recv()
        .await
        .map_err(|e| anyhow::anyhow!("{}", e))?;
    Ok(())
}

// ── Server entry point ────────────────────────────────────────────────────────

pub fn run(paths: MehPaths, daemonize: bool) -> Result<()> {
    // Enforce singleton: kill any previous daemon instance for this config dir.
    if let Ok(pid_str) = std::fs::read_to_string(&paths.pid_file) {
        if let Ok(raw) = pid_str.trim().parse::<i32>() {
            let pid = nix::unistd::Pid::from_raw(raw);
            let _ = nix::sys::signal::kill(pid, nix::sys::signal::Signal::SIGTERM);
            let deadline = std::time::Instant::now() + std::time::Duration::from_millis(800);
            while std::time::Instant::now() < deadline {
                if nix::sys::signal::kill(pid, None).is_err() { break; }
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            let _ = nix::sys::signal::kill(pid, nix::sys::signal::Signal::SIGKILL);
        }
        let _ = std::fs::remove_file(&paths.pid_file);
    }

    if daemonize {
        double_fork()?;
    }

    // Write our PID so the next startup can kill us cleanly.
    let _ = std::fs::write(&paths.pid_file, std::process::id().to_string());

    // GTK4 must be initialised on the "main" thread.
    gtk4::init()?;

    // Initialise platform integrations (libadwaita when compiled in).
    // Returns the initial dark-mode state so we can pre-populate MEH_DARK.
    let initial_dark = meh_gtk4::init_platform();

    let mut config = match MehConfig::load(&paths) {
        Ok(c) => c,
        Err(e) => {
            tracing::error!("Failed to load config: {}", e);
            MehConfig::default()
        }
    };

    // Inject built-in system vars so they are available from the first window open.
    if let Some(is_dark) = initial_dark {
        config.var_state.set(
            VarName("MEH_DARK".to_string()),
            DynVal::from_string(if is_dark { "true" } else { "false" }.to_string()),
        );
    }

    // Shared flag: tokio var-forwarder checks this before sending SetVarBatch.
    // When false the GTK main thread stays parked (zero idle CPU).
    let windows_open = Arc::new(AtomicBool::new(false));
    // Notified when the first window opens, so forward_var_updates can flush
    // pending initial values without waiting for the next poll tick.
    let window_opened = Arc::new(tokio::sync::Notify::new());
    // Notified when the last window closes, so listen subprocesses can be killed.
    let window_closed = Arc::new(tokio::sync::Notify::new());

    // Build App (GTK-thread-local)
    let mut app = meh_gtk4::App::new(paths.clone(), config, windows_open.clone(), window_opened.clone(), window_closed.clone());
    app.apply_css();

    // Channel for IPC commands (tokio unbounded; received in GTK thread via spawn_local)
    let (cmd_tx, mut cmd_rx) = tokio::sync::mpsc::unbounded_channel::<meh_gtk4::Cmd>();

    // Clone script vars for the async thread
    let script_vars = app.config.yuck.script_vars.clone();
    let config_dir  = paths.config_dir.clone();

    // Spawn the tokio runtime on a background thread
    let cmd_tx2 = cmd_tx.clone();
    let socket = paths.socket_file.clone();
    std::thread::Builder::new()
        .name("meh-async".to_string())
        .spawn(move || {
            let rt = tokio::runtime::Builder::new_multi_thread()
                .thread_name("meh-tokio")
                .enable_all()
                .build()
                .expect("tokio runtime");
            meh_gtk4::set_tokio_handle(rt.handle().clone());
            rt.block_on(async move {
                let ipc = tokio::spawn(run_ipc_server(socket, cmd_tx2.clone()));
                let sig = tokio::spawn({
                    let tx = cmd_tx2.clone();
                    async move {
                        recv_exit().await.ok();
                        let _ = tx.send(meh_gtk4::Cmd::Kill);
                    }
                });
                // Start script vars and forward updates to GTK thread, rate-limited.
                let var_rx = meh_script_vars::start_all(
                    &script_vars,
                    exit_sender().subscribe(),
                    windows_open.clone(),
                    window_opened.clone(),
                    window_closed.clone(),
                    config_dir,
                );
                let tx_vars = cmd_tx2.clone();
                let vars_fwd = tokio::spawn(forward_var_updates(var_rx, tx_vars, windows_open.clone(), window_opened.clone()));
                let _ = tokio::join!(ipc, sig, vars_fwd);
            });
        })?;

    // Wire system dark-mode change → MEH_DARK var update.
    meh_gtk4::connect_color_scheme(cmd_tx.clone());

    // React to monitor connect/disconnect: send MonitorsChanged into the cmd loop.
    let cmd_tx_mon = cmd_tx.clone();
    if let Some(display) = gtk4::gdk::Display::default() {
        use gtk4::gio::prelude::ListModelExt;
        use gtk4::prelude::DisplayExt;
        display.monitors().connect_items_changed(move |_, _, _, _| {
            let _ = cmd_tx_mon.send(meh_gtk4::Cmd::MonitorsChanged);
        });
    }

    // Set up signal handlers
    simple_signal::set_handler(
        &[simple_signal::Signal::Int, simple_signal::Signal::Term],
        move |_| {
            send_exit();
            let _ = cmd_tx.send(meh_gtk4::Cmd::Kill);
        },
    );

    // Drive IPC commands on the GTK main context via a 16 ms timer.
    //
    // Using spawn_local + tokio mpsc causes the GLib async executor
    // (TaskSource/WakerSource) to spin between wakeups on some glib versions.
    // A simple timeout_add_local drains the channel synchronously on each tick
    // and avoids the executor entirely — no waker races, measured ~0.1% overhead.
    // 16ms keeps IPC latency under one frame; raising to 50ms saves 0.1% but
    // adds perceptible lag to meh open/close.
    let main_loop = app.main_loop.clone();
    let main_loop2 = main_loop.clone();
    tracing::info!("cmd loop starting");
    gtk4::glib::timeout_add_local(Duration::from_millis(16), move || {
        loop {
            match cmd_rx.try_recv() {
                Ok(meh_gtk4::Cmd::Kill) => {
                    tracing::info!("cmd loop: Kill received");
                    main_loop2.quit();
                    return gtk4::glib::ControlFlow::Break;
                }
                Ok(cmd) => {
                    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        app.handle_cmd(cmd);
                    }));
                    if let Err(e) = result {
                        let msg = e.downcast_ref::<String>()
                            .map(|s| s.as_str())
                            .or_else(|| e.downcast_ref::<&str>().copied())
                            .unwrap_or("unknown panic");
                        tracing::error!("panic in handle_cmd: {}", msg);
                    }
                }
                Err(tokio::sync::mpsc::error::TryRecvError::Empty) => break,
                Err(tokio::sync::mpsc::error::TryRecvError::Disconnected) => {
                    tracing::info!("cmd loop: channel closed");
                    main_loop2.quit();
                    return gtk4::glib::ControlFlow::Break;
                }
            }
        }
        gtk4::glib::ControlFlow::Continue
    });

    main_loop.run();
    tracing::info!("meh daemon exiting");

    // Clean up socket and pidfile
    let _ = std::fs::remove_file(&paths.socket_file);
    let _ = std::fs::remove_file(&paths.pid_file);
    Ok(())
}

/// Forward script-var updates to the GTK thread, capped at ~30 fps.
///
/// Batches all updates that arrive in a 33 ms window into a single
/// `SetVarBatch` message (HashMap keeps only the latest value per name).
/// When no windows are open, pending updates are held but not forwarded,
/// keeping the GTK main thread parked at zero idle CPU.
///
/// `window_opened` is notified when the first window opens, allowing
/// pending initial values to be flushed without waiting for the next poll.
async fn forward_var_updates(
    mut var_rx: tokio::sync::mpsc::UnboundedReceiver<(VarName, DynVal)>,
    tx: UnboundedSender<meh_gtk4::Cmd>,
    windows_open: Arc<AtomicBool>,
    window_opened: Arc<tokio::sync::Notify>,
) {
    let mut pending: std::collections::HashMap<VarName, DynVal> = std::collections::HashMap::new();

    loop {
        tokio::select! {
            // Normal path: a new var update arrived.
            update = var_rx.recv() => {
                match update {
                    Some((name, val)) => { pending.insert(name, val); }
                    None => return,
                }
                // Drain any already-queued updates (no-block).
                while let Ok((name, val)) = var_rx.try_recv() {
                    pending.insert(name, val);
                }
                // Sleep the remainder of the 33ms frame budget.
                tokio::time::sleep(Duration::from_millis(33)).await;
                // Drain anything that arrived during the sleep.
                while let Ok((name, val)) = var_rx.try_recv() {
                    pending.insert(name, val);
                }
            }
            // Fast-flush path: windows just opened with pending initial values.
            // Guards against the case where all polls are gated (no new updates
            // arrive) but initial values are sitting in pending.
            _ = window_opened.notified(), if !pending.is_empty() => {}
        }

        if pending.is_empty() || !windows_open.load(Ordering::Relaxed) {
            continue;
        }

        let batch = std::mem::take(&mut pending);
        if tx.send(meh_gtk4::Cmd::SetVarBatch { vars: batch }).is_err() {
            return;
        }
    }
}

// ── IPC server ────────────────────────────────────────────────────────────────

async fn run_ipc_server(
    socket_path: std::path::PathBuf,
    cmd_tx: UnboundedSender<meh_gtk4::Cmd>,
) -> Result<()> {
    // Remove stale socket
    let _ = tokio::fs::remove_file(&socket_path).await;

    let listener = tokio::net::UnixListener::bind(&socket_path)?;
    tracing::info!("IPC server listening at {}", socket_path.display());

    loop {
        tokio::select! {
            Ok(()) = recv_exit() => break,
            Ok((stream, _)) = listener.accept() => {
                let tx = cmd_tx.clone();
                tokio::spawn(handle_connection(stream, tx));
            }
        }
    }
    Ok(())
}

async fn handle_connection(
    stream: tokio::net::UnixStream,
    cmd_tx: UnboundedSender<meh_gtk4::Cmd>,
) {
    let (mut reader, mut writer) = tokio::io::split(stream);

    let cmd: IpcCmd = match ipc_read(&mut reader).await {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!("IPC read error: {}", e);
            return;
        }
    };

    let resp = dispatch_cmd(cmd, &cmd_tx).await;

    if let Err(e) = ipc_write(&mut writer, &resp).await {
        tracing::warn!("IPC write error: {}", e);
    }
}

async fn dispatch_cmd(
    cmd: IpcCmd,
    cmd_tx: &UnboundedSender<meh_gtk4::Cmd>,
) -> IpcResponse {
    match cmd {
        IpcCmd::Ping => IpcResponse::ok("pong"),

        IpcCmd::Kill => {
            send_exit();
            let _ = cmd_tx.send(meh_gtk4::Cmd::Kill);
            IpcResponse::ok_empty()
        }

        other => {
            let (resp_tx, resp_rx) = tokio::sync::oneshot::channel();
            let gtk_cmd = ipc_to_gtk_cmd(other, resp_tx);
            let _ = cmd_tx.send(gtk_cmd);
            match tokio::time::timeout(std::time::Duration::from_secs(5), resp_rx).await {
                Ok(Ok(r)) => r,
                Ok(Err(_)) => IpcResponse::err("no response from daemon"),
                Err(_) => IpcResponse::err("daemon timed out"),
            }
        }
    }
}

fn ipc_to_gtk_cmd(
    cmd: IpcCmd,
    resp: tokio::sync::oneshot::Sender<IpcResponse>,
) -> meh_gtk4::Cmd {
    match cmd {
        IpcCmd::Open { window, toggle, monitor } => {
            meh_gtk4::Cmd::Open { window, toggle, monitor, resp }
        }
        IpcCmd::Close { windows } => meh_gtk4::Cmd::Close { windows, resp },
        IpcCmd::CloseAll => meh_gtk4::Cmd::CloseAll { resp },
        IpcCmd::Reload => meh_gtk4::Cmd::Reload { resp },
        IpcCmd::Update { vars } => meh_gtk4::Cmd::Update { vars, resp },
        IpcCmd::State => meh_gtk4::Cmd::State { resp },
        IpcCmd::Get { var } => meh_gtk4::Cmd::Get { var, resp },
        IpcCmd::ListWindows => meh_gtk4::Cmd::ListWindows { resp },
        // Ping and Kill handled before this function
        _ => unreachable!(),
    }
}

// ── Daemonize (double-fork) ───────────────────────────────────────────────────

fn double_fork() -> Result<()> {
    use nix::unistd::{fork, setsid, ForkResult};

    match unsafe { fork()? } {
        ForkResult::Child => {}
        ForkResult::Parent { .. } => std::process::exit(0),
    }

    setsid()?;

    match unsafe { fork()? } {
        ForkResult::Child => {}
        ForkResult::Parent { .. } => std::process::exit(0),
    }

    Ok(())
}
