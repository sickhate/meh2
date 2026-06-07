// GPL-3.0-or-later
//! Tokio handle, config dir, and event-handler command dispatch.

use gtk4::prelude::*;

pub(crate) static TOKIO_HANDLE: once_cell::sync::OnceCell<tokio::runtime::Handle> =
    once_cell::sync::OnceCell::new();

pub fn set_tokio_handle(handle: tokio::runtime::Handle) {
    let _ = TOKIO_HANDLE.set(handle);
}

pub(crate) static CONFIG_DIR: once_cell::sync::OnceCell<std::path::PathBuf> =
    once_cell::sync::OnceCell::new();

pub fn set_config_dir(dir: std::path::PathBuf) {
    let _ = CONFIG_DIR.set(dir);
}

pub fn spawn_cmd(cmd: &str) {
    let s = cmd.trim();

    // Route Rhai event handlers to the engine; fall back to shell for everything else.
    let is_rhai = s.ends_with(".rhai") || s.starts_with("rhai:");

    if is_rhai {
        #[cfg(feature = "rhai")]
        {
            let engine = meh_rhai_engine::global().unwrap_or_else(|| meh_rhai_engine::init());
            let cmd = cmd.to_owned();
            let cdir = CONFIG_DIR
                .get()
                .cloned()
                .unwrap_or_else(|| std::path::PathBuf::from("."));

            if let Some(handle) = TOKIO_HANDLE.get() {
                handle.spawn(async move {
                    let result = tokio::task::spawn_blocking(move || {
                        let s = cmd.trim();
                        if let Some(inline) = s.strip_prefix("rhai:") {
                            engine.eval_inline(inline.trim())
                        } else {
                            engine.eval_file(std::path::Path::new(s), &cdir)
                        }
                    })
                    .await;

                    match result {
                        Ok(Ok(out)) if !out.is_empty() => {
                            let _ = tokio::process::Command::new("sh")
                                .arg("-c")
                                .arg(&out)
                                .spawn();
                        }
                        Ok(Err(e)) => tracing::warn!("rhai onclick: {e}"),
                        _ => {}
                    }
                });
                return;
            }
        }
        #[cfg(not(feature = "rhai"))]
        {
            tracing::warn!("rhai event handler ignored (engine unavailable): {s}");
            return;
        }
    }

    let cmd = cmd.to_owned();
    gtk4::glib::spawn_future_local(async move {
        let _ = tokio::process::Command::new("sh")
            .arg("-c")
            .arg(&cmd)
            .spawn();
    });
}

#[cfg(feature = "animations")]
use libadwaita::prelude::AnimationExt;

use gtk4::glib;
