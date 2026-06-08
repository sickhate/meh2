// GPL-3.0-or-later
//! Widget tree construction, common props, and event handlers.

use std::collections::HashMap;

use anyhow::{Result, bail};
use eww_shared_util::VarName;
use gtk4::{glib, prelude::*};
use meh_core::EvalCtx;
use simplexpr::dynval::DynVal;
use yuck::config::{
    widget_definition::WidgetDefinition,
    widget_use::{BasicWidgetUse, LoopWidgetUse, WidgetUse},
};

use crate::bindings::{collect_bindings, maybe_bind, register_loop_binding};
use crate::launcher;
#[cfg(feature = "rhai")]
use crate::rhai_widget;
use crate::runtime::spawn_cmd;
use crate::widgets::{self, make_box_widget, parse_align, parse_orientation};
#[cfg(feature = "animations")]
use crate::widgets::{ensure_adw_init, parse_adw_easing};

#[cfg(feature = "animations")]
use libadwaita::prelude::AnimationExt;

// ── Widget builder ────────────────────────────────────────────────────────────

pub fn build_widget(wu: &WidgetUse, ctx: &EvalCtx) -> Result<gtk4::Widget> {
    match wu {
        WidgetUse::Basic(basic) => build_basic(basic, ctx),
        WidgetUse::Loop(lp) => build_loop(lp, ctx),
        WidgetUse::Children(_) => {
            // (children) outside a defwidget — render empty box as placeholder
            Ok(gtk4::Box::new(gtk4::Orientation::Horizontal, 0).upcast())
        }
    }
}

pub(crate) fn populate_loop_container(container: &gtk4::Box, lp: &LoopWidgetUse, ctx: &EvalCtx) {
    let items_val = match ctx.eval_expr(&lp.elements_expr) {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!("loop expr error: {}", e);
            return;
        }
    };
    let items = items_val.as_json_array().unwrap_or_default();
    for item in items {
        let item_str = match &item {
            serde_json::Value::String(s) => s.clone(),
            other => other.to_string(),
        };
        let mut extra = HashMap::new();
        // Loop variable always takes the full item value (JSON object/string/number).
        extra.insert(
            lp.element_name.clone(),
            DynVal::from_string(item_str.clone()),
        );
        // Flatten object fields into scope for convenience, but never overwrite
        // the loop variable itself (e.g. `day` loop var + `day` field in the object).
        if let serde_json::Value::Object(map) = &item {
            for (k, v) in map {
                let key = VarName(k.clone());
                if key == lp.element_name {
                    continue;
                }
                let val_str = match v {
                    serde_json::Value::String(s) => s.clone(),
                    other => other.to_string(),
                };
                extra.insert(key, DynVal::from_string(val_str));
            }
        }
        let child_ctx = ctx.child_scope(extra);
        match build_widget(&lp.body, &child_ctx) {
            Ok(w) => {
                container.append(&w);
            }
            Err(e) => tracing::warn!("loop child error: {}", e),
        }
    }
}

fn build_loop(lp: &LoopWidgetUse, ctx: &EvalCtx) -> Result<gtk4::Widget> {
    build_loop_oriented(lp, ctx, gtk4::Orientation::Vertical)
}

pub(crate) fn build_loop_oriented(
    lp: &LoopWidgetUse,
    ctx: &EvalCtx,
    orientation: gtk4::Orientation,
) -> Result<gtk4::Widget> {
    let items_val = ctx.eval_expr(&lp.elements_expr)?;
    let container = gtk4::Box::new(orientation, 0);
    let (_, child_bindings) =
        collect_bindings(|| populate_loop_container(&container, lp, ctx));
    register_loop_binding(lp, ctx, container.clone(), items_val.0, child_bindings);
    Ok(container.upcast())
}

fn build_basic(wu: &BasicWidgetUse, ctx: &EvalCtx) -> Result<gtk4::Widget> {
    // Check if it's a user-defined widget
    if let Some(def) = ctx.widget_defs.get(&wu.name) {
        return expand_defwidget(def, wu, ctx);
    }

    let w: gtk4::Widget = match wu.name.as_str() {
        "box" => widgets::build_box(wu, ctx)?.upcast(),
        "centerbox" => widgets::build_centerbox(wu, ctx)?.upcast(),
        "eventbox" => widgets::build_eventbox(wu, ctx)?.upcast(),
        "label" => widgets::build_label(wu, ctx)?.upcast(),
        "button" => widgets::build_button(wu, ctx)?.upcast(),
        "image" => widgets::build_image(wu, ctx)?,
        "scale" => widgets::build_scale(wu, ctx)?.upcast(),
        "progress" => widgets::build_progress(wu, ctx)?.upcast(),
        "circular-progress" => widgets::build_circular_progress(wu, ctx)?,
        "scroll" => widgets::build_scroll(wu, ctx)?.upcast(),
        "overlay" => widgets::build_overlay(wu, ctx)?.upcast(),
        "revealer" => widgets::build_revealer(wu, ctx)?.upcast(),
        "stack" => widgets::build_stack(wu, ctx)?.upcast(),
        "expander" => widgets::build_expander(wu, ctx)?.upcast(),
        "checkbox" => widgets::build_checkbox(wu, ctx)?.upcast(),
        "input" => widgets::build_input(wu, ctx)?.upcast(),
        "calendar" => widgets::build_calendar(wu, ctx)?.upcast(),
        "combo-box-text" => widgets::build_combo_box_text(wu, ctx)?.upcast(),
        "color-button" => widgets::build_color_button(wu, ctx)?.upcast(),
        "literal" => widgets::build_literal(wu, ctx)?.upcast(),
        "launcher" => launcher::build_launcher(wu, ctx)?,
        #[cfg(feature = "systray")]
        "systray" => widgets::build_systray(wu, ctx)?.upcast(),
        #[cfg(feature = "shader")]
        "shader" => widgets::build_shader(wu, ctx)?.upcast(),
        "rhai-widget" => {
            #[cfg(feature = "rhai")]
            {
                rhai_widget::build_rhai_widget(wu, ctx)?
            }
            #[cfg(not(feature = "rhai"))]
            {
                bail!("`rhai-widget` requires the `rhai` feature")
            }
        }
        unknown => {
            #[cfg(feature = "rhai")]
            if let Some(def) = meh_rhai_engine::get_widget_def(unknown) {
                return rhai_widget::build_rhai_defwidget(wu, def, ctx);
            }
            bail!(
                "Unknown widget: `{}`. Is it defined with `defwidget`?",
                unknown
            )
        }
    };

    // Each build_* already called apply_common_props internally; no second call here.
    Ok(w)
}

pub(crate) fn expand_defwidget(
    def: &WidgetDefinition,
    wu: &BasicWidgetUse,
    ctx: &EvalCtx,
) -> Result<gtk4::Widget> {
    let mut new_scope: HashMap<VarName, DynVal> = HashMap::new();

    for arg_spec in &def.expected_args {
        let key = &arg_spec.name;
        let value = if let Some(entry) = wu.attrs.attrs.get(key) {
            entry
                .value
                .as_simplexpr()
                .ok()
                .and_then(|expr| ctx.eval_expr(&expr).ok())
                .unwrap_or_else(|| DynVal::from_string(String::new()))
        } else if arg_spec.optional {
            DynVal::from_string(String::new())
        } else {
            bail!("Missing required arg `{}` for widget `{}`", key, def.name)
        };
        new_scope.insert(VarName(key.0.clone()), value);
    }

    // Handle `children` by building them in the outer context and storing somehow.
    // For now build children in caller context and pass as a GTK widget in a
    // special scope entry (not ideal but functional for simple cases).
    let children_widgets: Vec<gtk4::Widget> = wu
        .children
        .iter()
        .filter_map(|c| build_widget(c, ctx).ok())
        .collect();

    // Create the child context. We inject a magic `__children__` variable that
    // `(children)` in the defwidget body can reference via our override.
    let child_ctx = ctx.child_scope(new_scope);

    // Build the defwidget body
    build_widget_with_children(&def.widget, &child_ctx, &children_widgets)
}

pub(crate) fn build_widget_with_children(
    wu: &WidgetUse,
    ctx: &EvalCtx,
    outer_children: &[gtk4::Widget],
) -> Result<gtk4::Widget> {
    match wu {
        WidgetUse::Children(_) => {
            let container = gtk4::Box::new(gtk4::Orientation::Horizontal, 0);
            for ch in outer_children {
                container.append(ch);
            }
            Ok(container.upcast())
        }
        WidgetUse::Basic(basic) => {
            // For box widgets, pass orientation to direct loop children.
            let is_box = basic.name.as_str() == "box";
            let box_orientation = if is_box {
                ctx.eval_attr_str(&basic.attrs, "orientation")
                    .as_deref()
                    .map(parse_orientation)
                    .unwrap_or(gtk4::Orientation::Horizontal)
            } else {
                gtk4::Orientation::Vertical
            };

            let mut built_children = Vec::new();
            for child in &basic.children {
                let w = match child {
                    WidgetUse::Loop(lp) if is_box => build_loop_oriented(lp, ctx, box_orientation)?,
                    _ => build_widget_with_children(child, ctx, outer_children)?,
                };
                built_children.push(w);
            }
            let w = build_basic_with_prebuilt(basic, ctx, &built_children)?;
            Ok(w)
        }
        WidgetUse::Loop(lp) => build_loop(lp, ctx),
    }
}

pub(crate) fn build_basic_with_prebuilt(
    wu: &BasicWidgetUse,
    ctx: &EvalCtx,
    prebuilt_children: &[gtk4::Widget],
) -> Result<gtk4::Widget> {
    // Build via normal path if no prebuilt children needed
    if prebuilt_children.is_empty() {
        return build_basic(wu, ctx);
    }

    // For widgets that take children (box, centerbox, eventbox, etc.)
    let w: gtk4::Widget = match wu.name.as_str() {
        "box" => {
            let b = make_box_widget(wu, ctx)?;
            for ch in prebuilt_children {
                b.append(ch);
            }
            apply_common_props(&b, wu, ctx);
            b.upcast()
        }
        "eventbox" => {
            let b = gtk4::Box::new(gtk4::Orientation::Horizontal, 0);
            for ch in prebuilt_children {
                b.append(ch);
            }
            apply_common_props(&b, wu, ctx);
            b.upcast()
        }
        _ => {
            // fall back: build normally, children are ignored in prebuilt mode
            build_basic(wu, ctx)?
        }
    };
    Ok(w)
}

// ── apply_common_props ────────────────────────────────────────────────────────

#[allow(deprecated)]
pub(crate) fn apply_common_props(widget: &impl IsA<gtk4::Widget>, wu: &BasicWidgetUse, ctx: &EvalCtx) {
    let attrs = &wu.attrs;

    if let Some(class) = ctx.eval_attr_str(attrs, "class") {
        for c in class.split_whitespace() {
            widget.add_css_class(c);
        }
    }
    if let Some(style) = ctx.eval_attr_str(attrs, "style") {
        // Inline styles via gtk4 CssProvider per widget
        let provider = gtk4::CssProvider::new();
        provider.load_from_string(&format!("* {{ {} }}", style));
        widget
            .style_context()
            .add_provider(&provider, gtk4::STYLE_PROVIDER_PRIORITY_USER);
    }
    if let Some(name) = ctx.eval_attr_str(attrs, "name") {
        widget.set_widget_name(&name);
    }
    if let Some(align) = ctx.eval_attr_str(attrs, "halign") {
        widget.set_halign(parse_align(&align));
    }
    if let Some(align) = ctx.eval_attr_str(attrs, "valign") {
        widget.set_valign(parse_align(&align));
    }
    if let Some(v) = ctx.eval_attr_bool(attrs, "hexpand") {
        widget.set_hexpand(v);
    }
    if let Some(v) = ctx.eval_attr_bool(attrs, "vexpand") {
        widget.set_vexpand(v);
    }
    if let Some(v) = ctx.eval_attr_f64(attrs, "width") {
        widget.set_size_request(v as i32, -1);
    }
    if let Some(v) = ctx.eval_attr_f64(attrs, "height") {
        // set_size_request needs both; keep width
        let (w, _) = widget.size_request();
        widget.set_size_request(w, v as i32);
    }
    if let Some(v) = ctx.eval_attr_bool(attrs, "visible") {
        widget.set_visible(v);
    }
    if let Some(v) = ctx.eval_attr_bool(attrs, "sensitive") {
        widget.set_sensitive(v);
    }
    let initial_opacity = ctx.eval_attr_f64(attrs, "opacity");
    if let Some(v) = initial_opacity {
        widget.set_opacity(v);
    }
    // Reactive opacity binding + optional animation
    {
        let initial_str = initial_opacity.unwrap_or(1.0).to_string();
        #[cfg(feature = "animations")]
        {
            let duration = ctx.eval_attr_i64(attrs, "animate-duration").unwrap_or(0) as u32;
            let easing = parse_adw_easing(ctx.eval_attr_str(attrs, "animate-easing").as_deref());
            if duration > 0 {
                ensure_adw_init();
                let anim_holder: std::rc::Rc<std::cell::RefCell<Option<libadwaita::TimedAnimation>>> =
                    std::rc::Rc::new(std::cell::RefCell::new(None));
                let ah = anim_holder.clone();
                let wc = widget.upcast_ref::<gtk4::Widget>().clone();
                maybe_bind(attrs, "opacity", &ctx.scope, initial_str, move |v| {
                    if let Ok(f) = v.parse::<f64>() {
                        let to = f.clamp(0.0, 1.0);
                        if let Some(p) = ah.borrow().as_ref() {
                            p.pause();
                        }
                        let from = wc.opacity();
                        let wc2 = wc.clone();
                        let target = libadwaita::CallbackAnimationTarget::new(move |val| {
                            wc2.set_opacity(val);
                        });
                        let a = libadwaita::TimedAnimation::new(&wc, from, to, duration, target);
                        a.set_easing(easing);
                        a.set_follow_enable_animations_setting(true);
                        a.play();
                        *ah.borrow_mut() = Some(a);
                    }
                });
            } else {
                let wc = widget.upcast_ref::<gtk4::Widget>().clone();
                maybe_bind(attrs, "opacity", &ctx.scope, initial_str, move |v| {
                    if let Ok(f) = v.parse::<f64>() {
                        wc.set_opacity(f.clamp(0.0, 1.0));
                    }
                });
            }
        }
        #[cfg(not(feature = "animations"))]
        {
            let wc = widget.upcast_ref::<gtk4::Widget>().clone();
            maybe_bind(attrs, "opacity", &ctx.scope, initial_str, move |v| {
                if let Ok(f) = v.parse::<f64>() {
                    wc.set_opacity(f.clamp(0.0, 1.0));
                }
            });
        }
    }
    {
        // Use unwrap_or_default() so the binding is always registered even when the
        // var is not yet in scope at build time (defpoll initial fetch still running).
        // GTK4 treats set_tooltip_text("") like None — don't call it with empty string.
        let tooltip = ctx.eval_attr_str(attrs, "tooltip").unwrap_or_default();
        if !tooltip.is_empty() {
            widget.set_tooltip_text(Some(&tooltip));
        }
        let wc = widget.upcast_ref::<gtk4::Widget>().clone();
        maybe_bind(attrs, "tooltip", &ctx.scope, tooltip, move |v| {
            wc.set_tooltip_text(if v.is_empty() { None } else { Some(&v) });
        });
    }

    // Reactive bindings for properties that commonly reference variables.
    let wc = widget.upcast_ref::<gtk4::Widget>().clone();
    maybe_bind(
        attrs,
        "visible",
        &ctx.scope,
        widget.is_visible().to_string(),
        move |v| wc.set_visible(v == "true"),
    );

    {
        let wc = widget.upcast_ref::<gtk4::Widget>().clone();
        let initial_class = ctx.eval_attr_str(attrs, "class").unwrap_or_default();
        let mut last: Vec<String> = initial_class.split_whitespace().map(String::from).collect();
        for c in &last {
            widget.add_css_class(c);
        }
        maybe_bind(attrs, "class", &ctx.scope, initial_class, move |new| {
            for c in &last {
                wc.remove_css_class(c);
            }
            last = new.split_whitespace().map(String::from).collect();
            for c in &last {
                wc.add_css_class(c);
            }
        });
    }

    apply_event_handlers(widget, wu, ctx);
}

fn apply_event_handlers(widget: &impl IsA<gtk4::Widget>, wu: &BasicWidgetUse, ctx: &EvalCtx) {
    let attrs = &wu.attrs;

    // Use EventControllerLegacy for click events — fires unconditionally on raw
    // GDK ButtonPress, unlike GestureClick which can be denied on passive/transparent
    // widgets (e.g. click-catcher). Matches GTK3 EventBox button-press-event semantics.
    let onclick = ctx.eval_attr_str(attrs, "onclick");
    let onright = ctx.eval_attr_str(attrs, "onrightclick");
    let onmiddle = ctx.eval_attr_str(attrs, "onmiddleclick");
    if onclick.is_some() || onright.is_some() || onmiddle.is_some() {
        let ctrl = gtk4::EventControllerLegacy::new();
        ctrl.connect_event(move |ctrl, event| {
            use gtk4::gdk::EventType;
            if event.event_type() == EventType::ButtonPress {
                if ctrl.widget().map(|w| !w.is_sensitive()).unwrap_or(false) {
                    return glib::Propagation::Proceed;
                }
                let btn = event
                    .downcast_ref::<gtk4::gdk::ButtonEvent>()
                    .map(|e| e.button())
                    .unwrap_or(1);
                match btn {
                    1 => {
                        if let Some(cmd) = &onclick {
                            spawn_cmd(cmd);
                        }
                    }
                    3 => {
                        if let Some(cmd) = &onright {
                            spawn_cmd(cmd);
                        }
                    }
                    2 => {
                        if let Some(cmd) = &onmiddle {
                            spawn_cmd(cmd);
                        }
                    }
                    _ => {}
                }
            }
            glib::Propagation::Proceed
        });
        widget.add_controller(ctrl);
    }
    if let Some(cmd) = ctx.eval_attr_str(attrs, "onscroll") {
        let cmd = cmd.clone();
        let ctrl = gtk4::EventControllerScroll::new(
            gtk4::EventControllerScrollFlags::BOTH_AXES
                | gtk4::EventControllerScrollFlags::DISCRETE,
        );
        ctrl.connect_scroll(move |_, dx, dy| {
            let dir = if dy < 0.0 || dx < 0.0 { "up" } else { "down" };
            spawn_cmd(&cmd.replace("{}", dir));
            glib::Propagation::Proceed
        });
        widget.add_controller(ctrl);
    }
    if let Some(cmd) = ctx.eval_attr_str(attrs, "onhover") {
        let ctrl = gtk4::EventControllerMotion::new();
        ctrl.connect_enter(move |_, _, _| {
            spawn_cmd(&cmd);
        });
        widget.add_controller(ctrl);
    }
    if let Some(cmd) = ctx.eval_attr_str(attrs, "onhoverlost") {
        let ctrl = gtk4::EventControllerMotion::new();
        ctrl.connect_leave(move |_| {
            spawn_cmd(&cmd);
        });
        widget.add_controller(ctrl);
    }
    if let Some(cmd) = ctx.eval_attr_str(attrs, "onkeypress") {
        let ctrl = gtk4::EventControllerKey::new();
        ctrl.connect_key_pressed(move |_, key, _, _| {
            spawn_cmd(&cmd.replace("{}", &key.name().unwrap_or_default()));
            glib::Propagation::Proceed
        });
        widget.add_controller(ctrl);
    }
}
