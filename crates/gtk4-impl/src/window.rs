// GPL-3.0-or-later
//! Window creation with gtk4-layer-shell.

use anyhow::Result;
use gtk4::prelude::*;
use gtk4_layer_shell::LayerShell;
use meh_core::EvalCtx;
use yuck::config::{
    backend_window_options::WlWindowFocusable,
    window_definition::{WindowDefinition, WindowStacking},
    window_geometry::AnchorAlignment,
};

use crate::{AnyBinding, build_widget, collect_bindings};

/// A GTK window together with all reactive bindings for its widget tree.
pub struct LiveWindow {
    pub gtk_win: gtk4::Window,
    pub bindings: Vec<AnyBinding>,
    /// Connector string of the monitor this window was explicitly placed on.
    /// `None` means no explicit assignment (compositor chooses).
    /// Used to detect monitor disconnect and reopen on reconnect.
    pub monitor_connector: Option<String>,
}

pub fn build_live_window(
    def: &WindowDefinition,
    ctx: &EvalCtx,
    monitor_override: Option<i32>,
) -> Result<LiveWindow> {
    let window = gtk4::Window::new();
    window.set_title(Some(&def.name));

    let (child_result, bindings) = collect_bindings(|| build_widget(&def.widget, ctx));
    let child = child_result?;
    window.set_child(Some(&child));

    let monitor_connector = setup_layer_shell(&window, def, ctx, monitor_override)?;

    // GTK4 automatically shrinks the Wayland input region to non-transparent
    // areas. For transparent windows (e.g. the click-catcher), this leaves the
    // surface with no input region and clicks pass through. Restore full-surface
    // input after map AND after the first idle tick so it survives GTK4's
    // initial rendering pass (which may recompute the region from visual content).
    window.connect_map(|win| {
        if let Some(surface) = win.surface() {
            let full = gtk4::cairo::Region::create_rectangle(&gtk4::cairo::RectangleInt::new(
                0,
                0,
                i32::MAX / 2,
                i32::MAX / 2,
            ));
            surface.set_input_region(&full);
        }
        let win = win.clone();
        gtk4::glib::idle_add_local_once(move || {
            if let Some(surface) = win.surface() {
                let full = gtk4::cairo::Region::create_rectangle(&gtk4::cairo::RectangleInt::new(
                    0,
                    0,
                    i32::MAX / 2,
                    i32::MAX / 2,
                ));
                surface.set_input_region(&full);
            }
        });
    });

    Ok(LiveWindow {
        gtk_win: window,
        bindings,
        monitor_connector,
    })
}

/// Returns the connector string of the monitor the window was placed on, if any.
fn setup_layer_shell(
    window: &gtk4::Window,
    def: &WindowDefinition,
    ctx: &EvalCtx,
    monitor_override: Option<i32>,
) -> Result<Option<String>> {
    window.init_layer_shell();

    let vars = ctx.all_vars();

    // Layer
    let stacking = def
        .eval_stacking(&vars)
        .unwrap_or(WindowStacking::Foreground);
    let layer = match stacking {
        WindowStacking::Foreground => gtk4_layer_shell::Layer::Top,
        WindowStacking::Background => gtk4_layer_shell::Layer::Background,
        WindowStacking::Bottom => gtk4_layer_shell::Layer::Bottom,
        WindowStacking::Overlay => gtk4_layer_shell::Layer::Overlay,
    };
    window.set_layer(layer);

    // Monitor — resolved before geometry so we can use monitor dimensions for
    // percentage-based size/offset calculations.
    use yuck::config::monitor::MonitorIdentifier;
    let monitor_spec = monitor_override
        .map(MonitorIdentifier::Numeric)
        .or_else(|| def.eval_monitor(&vars).ok().flatten());

    let mut placed_connector: Option<String> = None;
    let mut monitor_geo: Option<gtk4::gdk::Rectangle> = None;

    let resolved_monitor: Option<gtk4::gdk::Monitor> = monitor_spec.and_then(|spec| {
        let display = gtk4::gdk::Display::default()?;
        let monitors = display.monitors();
        let count = monitors.n_items();
        (0..count).find_map(|i| {
            monitors
                .item(i)?
                .downcast::<gtk4::gdk::Monitor>()
                .ok()
                .and_then(|mon| {
                    let matches = match &spec {
                        MonitorIdentifier::Numeric(n) => i as i32 == *n,
                        MonitorIdentifier::Name(name) => {
                            mon.model().as_deref() == Some(name.as_str())
                                || mon.connector().as_deref() == Some(name.as_str())
                                || mon.description().as_deref() == Some(name.as_str())
                                || mon
                                    .description()
                                    .map(|d| d.contains(name.as_str()))
                                    .unwrap_or(false)
                        }
                        MonitorIdentifier::Primary => i == 0,
                        MonitorIdentifier::List(ids) => ids.iter().any(|id| match id {
                            MonitorIdentifier::Numeric(n) => i as i32 == *n,
                            MonitorIdentifier::Name(name) => {
                                mon.model().as_deref() == Some(name.as_str())
                                    || mon.connector().as_deref() == Some(name.as_str())
                            }
                            _ => false,
                        }),
                    };
                    if matches { Some(mon) } else { None }
                })
        })
    });

    if let Some(mon) = resolved_monitor {
        monitor_geo = Some(mon.geometry());
        placed_connector = mon.connector().map(|c| c.to_string());
        window.set_monitor(Some(&mon));
    } else if let Some(display) = gtk4::gdk::Display::default() {
        // Fall back to the first available monitor for percentage calculations.
        if let Some(obj) = display.monitors().item(0) {
            monitor_geo = obj
                .downcast::<gtk4::gdk::Monitor>()
                .ok()
                .map(|m| m.geometry());
        }
    }

    let mon_w = monitor_geo.map(|g| g.width()).unwrap_or(1920);
    let mon_h = monitor_geo.map(|g| g.height()).unwrap_or(1080);

    // Resizable
    window.set_resizable(def.eval_resizable(&vars).unwrap_or(true));

    // Geometry — use actual monitor dimensions so percentage values are correct.
    if let Some(geom_def) = &def.geometry
        && let Ok(geom) = geom_def.eval(&vars)
    {
        let anchor = geom.anchor_point;
        let top = anchor.y == AnchorAlignment::START;
        let bottom = anchor.y == AnchorAlignment::END;
        let left = anchor.x == AnchorAlignment::START;
        let right = anchor.x == AnchorAlignment::END;

        window.set_anchor(gtk4_layer_shell::Edge::Top, top);
        window.set_anchor(gtk4_layer_shell::Edge::Bottom, bottom);
        window.set_anchor(gtk4_layer_shell::Edge::Left, left);
        window.set_anchor(gtk4_layer_shell::Edge::Right, right);

        // Margins from offset
        let ox = geom.offset.x.pixels_relative_to(mon_w);
        let oy = geom.offset.y.pixels_relative_to(mon_h);
        if left {
            window.set_margin(gtk4_layer_shell::Edge::Left, ox);
        } else {
            window.set_margin(gtk4_layer_shell::Edge::Right, ox.abs());
        }
        if top {
            window.set_margin(gtk4_layer_shell::Edge::Top, oy);
        } else {
            window.set_margin(gtk4_layer_shell::Edge::Bottom, oy.abs());
        }

        // Size — percentage values resolve against the monitor dimensions.
        let w = geom.size.x.pixels_relative_to(mon_w);
        let h = geom.size.y.pixels_relative_to(mon_h);
        if w > 0 || h > 0 {
            window.set_default_size(if w > 0 { w } else { -1 }, if h > 0 { h } else { -1 });
        }
    }

    // Backend options
    if let Ok(opts) = def.backend_options.eval(&vars) {
        if opts.wayland.exclusive {
            window.auto_exclusive_zone_enable();
        }
        let kb_mode = match opts.wayland.focusable {
            WlWindowFocusable::None => gtk4_layer_shell::KeyboardMode::None,
            WlWindowFocusable::Exclusive => gtk4_layer_shell::KeyboardMode::Exclusive,
            WlWindowFocusable::OnDemand => gtk4_layer_shell::KeyboardMode::OnDemand,
        };
        window.set_keyboard_mode(kb_mode);
        if let Some(ns) = &opts.wayland.namespace {
            window.set_namespace(Some(ns));
        }
    }

    Ok(placed_connector)
}
