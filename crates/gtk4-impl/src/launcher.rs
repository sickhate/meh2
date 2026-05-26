// GPL-3.0-or-later
//! Native application launcher widget.
//!
//! `(launcher :placeholder "…" :max-results 8 :window "launcher" :terminal "foot")`
//!
//! Uses `gio::AppInfo::all()` for desktop apps and PATH scanning for executables.
//! Attrs:
//!   `:placeholder`  — entry placeholder text  (default "Search applications…")
//!   `:max-results`  — cap on visible app results   (default 8)
//!   `:window`       — meh window name to close after launch / Escape (default "launcher")
//!   `:terminal`     — terminal to use for PATH executables, e.g. "foot" or "kitty"
//!                     (default ""; bins are run directly — suitable when they are GUI apps)

use std::{cell::Cell, rc::Rc};

use anyhow::Result;
use gtk4::{gdk::Key, gio, prelude::*};
use meh_core::EvalCtx;
use yuck::config::widget_use::BasicWidgetUse;

use crate::{apply_common_props, spawn_cmd};

#[derive(Clone)]
enum ResultItem {
    App(gio::AppInfo),
    Bin(String),
}

pub fn build_launcher(wu: &BasicWidgetUse, ctx: &EvalCtx) -> Result<gtk4::Widget> {
    let attrs = &wu.attrs;

    let placeholder = ctx.eval_attr_str(attrs, "placeholder")
        .unwrap_or_default();
    let max_results = ctx.eval_attr_str(attrs, "max-results")
        .and_then(|s| s.parse::<usize>().ok())
        .unwrap_or(8);
    let window_name = ctx.eval_attr_str(attrs, "window")
        .unwrap_or_else(|| "launcher".to_string());
    let show_run_command = ctx.eval_attr_bool(attrs, "show-run-command").unwrap_or(true);
    let show_bins        = ctx.eval_attr_bool(attrs, "show-bins").unwrap_or(true);
    let terminal: Rc<String> = Rc::new(
        ctx.eval_attr_str(attrs, "terminal").unwrap_or_default().trim().to_string()
    );

    let all_apps: Rc<Vec<gio::AppInfo>> = Rc::new(
        gio::AppInfo::all().into_iter().filter(|a| a.should_show()).collect(),
    );
    let all_bins: Rc<Vec<String>> = Rc::new(collect_path_bins());

    // ── Layout ───────────────────────────────────────────────────────────────
    let root = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
    root.add_css_class("launcher");

    let entry = gtk4::Entry::new();
    entry.set_placeholder_text(Some(&placeholder));
    entry.add_css_class("launcher-input");
    root.append(&entry);

    let results = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
    results.add_css_class("launcher-results");
    root.append(&results);

    // ── Shared mutable state ─────────────────────────────────────────────────
    // Ordered list of what's currently displayed (excludes the literal run row).
    let current: Rc<std::cell::RefCell<Vec<ResultItem>>> =
        Rc::new(std::cell::RefCell::new(Vec::new()));
    let selected: Rc<Cell<usize>> = Rc::new(Cell::new(0));

    // ── Entry → filter results ───────────────────────────────────────────────
    {
        let results     = results.clone();
        let all_apps    = all_apps.clone();
        let all_bins    = all_bins.clone();
        let current     = current.clone();
        let selected    = selected.clone();
        let window_name = window_name.clone();
        let terminal    = terminal.clone();

        entry.connect_changed(move |e| {
            let q = e.text().to_lowercase();

            let mut items: Vec<ResultItem> = Vec::new();

            if !q.is_empty() {
                // Desktop apps first.
                let apps: Vec<ResultItem> = all_apps.iter()
                    .filter(|a| {
                        a.display_name().to_lowercase().contains(&q)
                            || a.name().to_lowercase().contains(&q)
                    })
                    .take(max_results)
                    .cloned()
                    .map(ResultItem::App)
                    .collect();
                items.extend(apps);
                // PATH executables — shown only when :show-bins is true.
                if show_bins {
                    let bins: Vec<ResultItem> = all_bins.iter()
                        .filter(|b| b.to_lowercase().contains(&q))
                        .take(4)
                        .cloned()
                        .map(ResultItem::Bin)
                        .collect();
                    items.extend(bins);
                }
            }

            *current.borrow_mut() = items.clone();
            selected.set(0);

            while let Some(child) = results.first_child() {
                results.remove(&child);
            }
            for (i, item) in items.iter().enumerate() {
                let row = match item {
                    ResultItem::App(app) => make_app_row(app, i == 0, &window_name),
                    ResultItem::Bin(bin) => make_bin_row(bin, i == 0, &window_name, &terminal),
                };
                results.append(&row);
            }
            if show_run_command && !q.is_empty() {
                let is_sel = items.is_empty();
                results.append(&make_run_row(&e.text(), is_sel, &window_name));
            }
        });
    }

    // ── Keyboard navigation ───────────────────────────────────────────────────
    {
        let entry_kc    = entry.clone();
        let entry       = entry.clone();
        let results     = results.clone();
        let current     = current.clone();
        let selected    = selected.clone();
        let window_name = window_name.clone();
        let terminal    = terminal.clone();

        let kc = gtk4::EventControllerKey::new();
        kc.set_propagation_phase(gtk4::PropagationPhase::Capture);
        kc.connect_key_pressed(move |_, key, _, _| {
            use gtk4::glib::Propagation::{Proceed, Stop};
            let n = row_count(&results);

            match key {
                Key::Down | Key::KP_Down => {
                    if n == 0 { return Proceed; }
                    let cur = selected.get();
                    set_row_selected(&results, cur, false);
                    let next = (cur + 1).min(n - 1);
                    selected.set(next);
                    set_row_selected(&results, next, true);
                    Stop
                }
                Key::Up | Key::KP_Up => {
                    if n == 0 { return Proceed; }
                    let cur = selected.get();
                    set_row_selected(&results, cur, false);
                    let prev = cur.saturating_sub(1);
                    selected.set(prev);
                    set_row_selected(&results, prev, true);
                    Stop
                }
                Key::Return | Key::KP_Enter => {
                    let sel  = selected.get();
                    let items = current.borrow();
                    let text = entry.text().to_string();

                    if let Some(item) = items.get(sel) {
                        let cmd = match item {
                            ResultItem::App(app) => {
                                let _ = app.launch(&[], gio::AppLaunchContext::NONE);
                                None
                            }
                            ResultItem::Bin(bin) => Some(bin.clone()),
                        };
                        drop(items);
                        let wn   = window_name.clone();
                        let e    = entry.clone();
                        let term = terminal.clone();
                        gtk4::glib::idle_add_local_once(move || {
                            e.set_text("");
                            if let Some(c) = cmd { spawn_cmd(&bin_launch(&term, &c)); }
                            spawn_cmd(&format!("meh close {wn}"));
                        });
                    } else if show_run_command && !text.is_empty() {
                        // Literal run row — only reachable when :show-run-command true.
                        drop(items);
                        let wn = window_name.clone();
                        let e  = entry.clone();
                        gtk4::glib::idle_add_local_once(move || {
                            e.set_text("");
                            spawn_cmd(&text);
                            spawn_cmd(&format!("meh close {wn}"));
                        });
                    }
                    Stop
                }
                Key::Escape => {
                    let wn = window_name.clone();
                    let e  = entry.clone();
                    gtk4::glib::idle_add_local_once(move || {
                        e.set_text("");
                        spawn_cmd(&format!("meh close {wn}"));
                    });
                    Stop
                }
                _ => Proceed,
            }
        });
        entry_kc.add_controller(kc);
    }

    entry.connect_map(|e| { e.grab_focus(); });

    apply_common_props(&root, wu, ctx);
    Ok(root.upcast())
}

// ── Row builders ──────────────────────────────────────────────────────────────

fn make_app_row(app: &gio::AppInfo, selected: bool, window_name: &str) -> gtk4::Box {
    let row = gtk4::Box::new(gtk4::Orientation::Horizontal, 8);
    row.add_css_class("launcher-row");
    if selected { row.add_css_class("selected"); }

    if let Some(icon) = app.icon() {
        let img = gtk4::Image::from_gicon(&icon);
        img.set_icon_size(gtk4::IconSize::Large);
        img.add_css_class("launcher-icon");
        row.append(&img);
    }

    let label = gtk4::Label::new(Some(&app.display_name()));
    label.set_halign(gtk4::Align::Start);
    label.set_hexpand(true);
    label.add_css_class("launcher-name");
    row.append(&label);

    if let Some(desc) = app.description().filter(|d| !d.is_empty()) {
        let d = gtk4::Label::new(Some(&desc));
        d.set_halign(gtk4::Align::End);
        d.set_max_width_chars(30);
        d.set_ellipsize(gtk4::pango::EllipsizeMode::End);
        d.add_css_class("launcher-desc");
        row.append(&d);
    }

    let app_clone = app.clone();
    let wn = window_name.to_string();
    let gc = gtk4::GestureClick::new();
    gc.connect_released(move |_, _, _, _| {
        let _ = app_clone.launch(&[], gio::AppLaunchContext::NONE);
        let w = wn.clone();
        gtk4::glib::idle_add_local_once(move || {
            spawn_cmd(&format!("meh close {w}"));
        });
    });
    row.add_controller(gc);
    row
}

fn make_bin_row(bin: &str, selected: bool, window_name: &str, terminal: &str) -> gtk4::Box {
    let row = gtk4::Box::new(gtk4::Orientation::Horizontal, 8);
    row.add_css_class("launcher-row");
    row.add_css_class("launcher-bin-row");
    if selected { row.add_css_class("selected"); }

    let prefix = gtk4::Label::new(Some("$"));
    prefix.add_css_class("launcher-run-prefix");
    row.append(&prefix);

    let label = gtk4::Label::new(Some(bin));
    label.set_halign(gtk4::Align::Start);
    label.set_hexpand(true);
    label.add_css_class("launcher-name");
    row.append(&label);

    let hint = gtk4::Label::new(Some("executable"));
    hint.set_halign(gtk4::Align::End);
    hint.add_css_class("launcher-desc");
    row.append(&hint);

    let cmd  = bin.to_string();
    let wn   = window_name.to_string();
    let term = terminal.to_string();
    let gc = gtk4::GestureClick::new();
    gc.connect_released(move |_, _, _, _| {
        let c = cmd.clone();
        let w = wn.clone();
        let t = term.clone();
        gtk4::glib::idle_add_local_once(move || {
            spawn_cmd(&bin_launch(&t, &c));
            spawn_cmd(&format!("meh close {w}"));
        });
    });
    row.add_controller(gc);
    row
}

fn make_run_row(cmd: &str, selected: bool, window_name: &str) -> gtk4::Box {
    let row = gtk4::Box::new(gtk4::Orientation::Horizontal, 8);
    row.add_css_class("launcher-row");
    row.add_css_class("launcher-run-row");
    if selected { row.add_css_class("selected"); }

    let prefix = gtk4::Label::new(Some("$"));
    prefix.add_css_class("launcher-run-prefix");
    row.append(&prefix);

    let label = gtk4::Label::new(Some(cmd));
    label.set_halign(gtk4::Align::Start);
    label.set_hexpand(true);
    label.add_css_class("launcher-name");
    row.append(&label);

    let hint = gtk4::Label::new(Some("run command"));
    hint.set_halign(gtk4::Align::End);
    hint.add_css_class("launcher-desc");
    row.append(&hint);

    let cmd_owned = cmd.to_string();
    let wn = window_name.to_string();
    let gc = gtk4::GestureClick::new();
    gc.connect_released(move |_, _, _, _| {
        let c = cmd_owned.clone();
        let w = wn.clone();
        gtk4::glib::idle_add_local_once(move || {
            spawn_cmd(&c);
            spawn_cmd(&format!("meh close {w}"));
        });
    });
    row.add_controller(gc);
    row
}

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Returns `<terminal> -e <cmd>` when a terminal is configured, else just `<cmd>`.
fn bin_launch(terminal: &str, cmd: &str) -> String {
    if terminal.is_empty() {
        cmd.to_string()
    } else {
        format!("{terminal} -e {cmd}")
    }
}

fn collect_path_bins() -> Vec<String> {
    use std::os::unix::fs::PermissionsExt;
    let path_var = std::env::var("PATH").unwrap_or_default();
    let mut seen = std::collections::HashSet::new();
    for dir in path_var.split(':') {
        let Ok(entries) = std::fs::read_dir(dir) else { continue };
        for entry in entries.flatten() {
            let Ok(meta) = entry.metadata() else { continue };
            if !meta.is_file() && !meta.file_type().is_symlink() { continue }
            if meta.permissions().mode() & 0o111 == 0 { continue }
            if let Some(name) = entry.file_name().to_str() {
                seen.insert(name.to_string());
            }
        }
    }
    let mut v: Vec<String> = seen.into_iter().collect();
    v.sort();
    v
}

fn row_count(results: &gtk4::Box) -> usize {
    let mut n = 0usize;
    let mut c = results.first_child();
    while let Some(w) = c { n += 1; c = w.next_sibling(); }
    n
}

fn set_row_selected(results: &gtk4::Box, idx: usize, add: bool) {
    let mut i = 0usize;
    let mut c = results.first_child();
    while let Some(w) = c {
        if i == idx {
            if add { w.add_css_class("selected"); }
            else   { w.remove_css_class("selected"); }
            return;
        }
        i += 1;
        c = w.next_sibling();
    }
}
