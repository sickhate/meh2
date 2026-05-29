// GPL-3.0-or-later
//! GTK4 widget builder and window management for meh.

use std::collections::HashMap;

use anyhow::{Result, bail};
use eww_shared_util::{AttrName, VarName};
use gtk4::{gdk, prelude::*};
use meh_core::EvalCtx;
use simplexpr::{SimplExpr, dynval::DynVal};
use std::cell::RefCell;
use yuck::config::{
    widget_definition::WidgetDefinition,
    widget_use::{BasicWidgetUse, LoopWidgetUse, WidgetUse},
};

// ── Reactive binding system ───────────────────────────────────────────────────

/// A live reactive attribute binding.
/// Holds the unevaluated expression, the local scope it was captured in, and a setter closure.
/// Call `update` when global vars change; the setter fires only if the evaluated value changed.
pub struct Binding {
    expr: SimplExpr,
    var_refs: Vec<VarName>,
    is_constant: bool,
    scope: HashMap<VarName, DynVal>,
    setter: Box<dyn FnMut(String) + 'static>,
    last_val: String,
}

impl Binding {
    pub fn update(&mut self, global_vars: &HashMap<VarName, DynVal>) -> bool {
        let new_val = if self.is_constant {
            self.expr.eval(&HashMap::new()).map(|v| v.0).unwrap_or_default()
        } else {
            let refs = &self.var_refs;
            let mut vars = HashMap::with_capacity(refs.len());
            for var in refs {
                if !vars.contains_key(var) {
                    if let Some(val) = self.scope.get(var).or_else(|| global_vars.get(var)) {
                        vars.insert(var.clone(), val.clone());
                    }
                }
            }
            self.expr.eval(&vars).map(|v| v.0).unwrap_or_default()
        };
        if new_val != self.last_val {
            (self.setter)(new_val.clone());
            self.last_val = new_val;
            true
        } else {
            false
        }
    }

    pub fn intersects(&self, changed: &std::collections::HashSet<VarName>) -> bool {
        if self.is_constant {
            return false;
        }
        self.var_refs.iter().any(|v| changed.contains(v))
    }
}

/// A live reactive `for` loop binding.
/// When the elements expression changes, clears and rebuilds the container children.
pub struct LoopBinding {
    expr: SimplExpr,
    var_refs: Vec<VarName>,
    is_constant: bool,
    scope: HashMap<VarName, DynVal>,
    lp: LoopWidgetUse,
    widget_defs: HashMap<String, WidgetDefinition>,
    container: gtk4::Box,
    last_val: String,
}

impl LoopBinding {
    pub fn update(&mut self, global_vars: &HashMap<VarName, DynVal>) -> bool {
        let new_val = if self.is_constant {
            self.expr.eval(&HashMap::new()).map(|v| v.0).unwrap_or_default()
        } else {
            let refs = &self.var_refs;
            let mut vars = HashMap::with_capacity(refs.len());
            for var in refs {
                if !vars.contains_key(var) {
                    if let Some(val) = self.scope.get(var).or_else(|| global_vars.get(var)) {
                        vars.insert(var.clone(), val.clone());
                    }
                }
            }
            self.expr.eval(&vars).map(|v| v.0).unwrap_or_default()
        };
        if new_val == self.last_val {
            return false;
        }
        self.last_val = new_val.clone();

        while let Some(child) = self.container.first_child() {
            self.container.remove(&child);
        }
        let ctx = EvalCtx {
            scope: self.scope.clone(),
            global_vars,
            widget_defs: &self.widget_defs,
        };
        populate_loop_container(&self.container, &self.lp, &ctx);
        true
    }

    pub fn intersects(&self, changed: &std::collections::HashSet<VarName>) -> bool {
        if self.is_constant {
            return false;
        }
        self.var_refs.iter().any(|v| changed.contains(v))
    }
}

/// Either an attribute binding, a loop binding, or a Rhai widget binding.
pub enum AnyBinding {
    Attr(Binding),
    Loop(LoopBinding),
    #[cfg(feature = "rhai")]
    RhaiWidget(rhai_widget::RhaiWidgetBinding),
}

impl AnyBinding {
    pub fn update(&mut self, global_vars: &HashMap<VarName, DynVal>) -> bool {
        match self {
            AnyBinding::Attr(b) => b.update(global_vars),
            AnyBinding::Loop(b) => b.update(global_vars),
            #[cfg(feature = "rhai")]
            AnyBinding::RhaiWidget(b) => b.update(global_vars),
        }
    }

    pub fn intersects(&self, changed: &std::collections::HashSet<VarName>) -> bool {
        match self {
            AnyBinding::Attr(b) => b.intersects(changed),
            AnyBinding::Loop(b) => b.intersects(changed),
            #[cfg(feature = "rhai")]
            AnyBinding::RhaiWidget(b) => b.intersects(changed),
        }
    }
}

// Active only during `collect_bindings()`; None otherwise.
thread_local! {
    static BINDING_COLLECTOR: RefCell<Option<Vec<AnyBinding>>> = const { RefCell::new(None) };
}

/// Run `f` while collecting any bindings registered via `maybe_bind` / `register_loop_binding`.
/// Returns the result of `f` plus all collected bindings.
pub fn collect_bindings<T>(f: impl FnOnce() -> T) -> (T, Vec<AnyBinding>) {
    BINDING_COLLECTOR.with(|col| *col.borrow_mut() = Some(Vec::new()));
    let result = f();
    let bindings = BINDING_COLLECTOR.with(|col| col.borrow_mut().take().unwrap_or_default());
    (result, bindings)
}

/// Register a reactive binding for `attr_name` in `attrs` — but only if the attribute's
/// expression references at least one variable (literals never need reactive updates).
fn maybe_bind<F>(
    attrs: &yuck::config::attributes::Attributes,
    attr_name: &str,
    scope: &HashMap<VarName, DynVal>,
    initial: String,
    setter: F,
) where
    F: FnMut(String) + 'static,
{
    if let Some(entry) = attrs.attrs.get(&AttrName(attr_name.to_string()))
        && let Ok(expr) = entry.value.as_simplexpr()
        && !expr.collect_var_refs().is_empty()
    {
        BINDING_COLLECTOR.with(|col| {
            if let Some(bindings) = col.borrow_mut().as_mut() {
                let var_refs = expr.collect_var_refs();
                let is_constant = var_refs.is_empty();
                bindings.push(AnyBinding::Attr(Binding {
                    expr,
                    var_refs,
                    is_constant,
                    scope: scope.clone(),
                    setter: Box::new(setter),
                    last_val: initial,
                }));
            }
        });
    }
}

/// Register a reactive loop binding — fires when `lp.elements_expr` references any variable.
fn register_loop_binding(lp: &LoopWidgetUse, ctx: &EvalCtx, container: gtk4::Box, initial: String) {
    if lp.elements_expr.collect_var_refs().is_empty() {
        return;
    }
    BINDING_COLLECTOR.with(|col| {
        if let Some(bindings) = col.borrow_mut().as_mut() {
            let var_refs = lp.elements_expr.collect_var_refs();
            let is_constant = var_refs.is_empty();
            bindings.push(AnyBinding::Loop(LoopBinding {
                expr: lp.elements_expr.clone(),
                var_refs,
                is_constant,
                scope: ctx.scope.clone(),
                lp: lp.clone(),
                widget_defs: ctx.widget_defs.clone(),
                container,
                last_val: initial,
            }));
        }
    });
}

pub mod app;
pub mod css;
mod launcher;
#[cfg(feature = "rhai")]
mod rhai_widget;
pub mod window;

pub use app::{App, Cmd, connect_color_scheme, init_platform};

// ── Tokio handle (needed by systray button click handlers) ────────────────────

static TOKIO_HANDLE: once_cell::sync::OnceCell<tokio::runtime::Handle> =
    once_cell::sync::OnceCell::new();

/// Called by the daemon after creating the tokio runtime so that widget code
/// can spawn tasks without owning the runtime.
pub fn set_tokio_handle(handle: tokio::runtime::Handle) {
    let _ = TOKIO_HANDLE.set(handle);
}

static CONFIG_DIR: once_cell::sync::OnceCell<std::path::PathBuf> = once_cell::sync::OnceCell::new();

/// Called by the daemon so event handlers can resolve relative `.rhai` paths.
pub fn set_config_dir(dir: std::path::PathBuf) {
    let _ = CONFIG_DIR.set(dir);
}

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

fn populate_loop_container(container: &gtk4::Box, lp: &LoopWidgetUse, ctx: &EvalCtx) {
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

fn build_loop_oriented(
    lp: &LoopWidgetUse,
    ctx: &EvalCtx,
    orientation: gtk4::Orientation,
) -> Result<gtk4::Widget> {
    let items_val = ctx.eval_expr(&lp.elements_expr)?;
    let container = gtk4::Box::new(orientation, 0);
    populate_loop_container(&container, lp, ctx);
    register_loop_binding(lp, ctx, container.clone(), items_val.0);
    Ok(container.upcast())
}

fn build_basic(wu: &BasicWidgetUse, ctx: &EvalCtx) -> Result<gtk4::Widget> {
    // Check if it's a user-defined widget
    if let Some(def) = ctx.widget_defs.get(&wu.name).cloned() {
        return expand_defwidget(&def, wu, ctx);
    }

    let w: gtk4::Widget = match wu.name.as_str() {
        "box" => build_box(wu, ctx)?.upcast(),
        "centerbox" => build_centerbox(wu, ctx)?.upcast(),
        "eventbox" => build_eventbox(wu, ctx)?.upcast(),
        "label" => build_label(wu, ctx)?.upcast(),
        "button" => build_button(wu, ctx)?.upcast(),
        "image" => build_image(wu, ctx)?,
        "scale" => build_scale(wu, ctx)?.upcast(),
        "progress" => build_progress(wu, ctx)?.upcast(),
        "circular-progress" => build_circular_progress(wu, ctx)?,
        "scroll" => build_scroll(wu, ctx)?.upcast(),
        "overlay" => build_overlay(wu, ctx)?.upcast(),
        "revealer" => build_revealer(wu, ctx)?.upcast(),
        "stack" => build_stack(wu, ctx)?.upcast(),
        "expander" => build_expander(wu, ctx)?.upcast(),
        "checkbox" => build_checkbox(wu, ctx)?.upcast(),
        "input" => build_input(wu, ctx)?.upcast(),
        "calendar" => build_calendar(wu, ctx)?.upcast(),
        "combo-box-text" => build_combo_box_text(wu, ctx)?.upcast(),
        "color-button" => build_color_button(wu, ctx)?.upcast(),
        "literal" => build_literal(wu, ctx)?.upcast(),
        "launcher" => launcher::build_launcher(wu, ctx)?,
        #[cfg(feature = "systray")]
        "systray" => build_systray(wu, ctx)?.upcast(),
        #[cfg(feature = "shader")]
        "shader" => build_shader(wu, ctx)?.upcast(),
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

fn expand_defwidget(
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

fn build_widget_with_children(
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

fn build_basic_with_prebuilt(
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
fn apply_common_props(widget: &impl IsA<gtk4::Widget>, wu: &BasicWidgetUse, ctx: &EvalCtx) {
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
                let anim_holder: std::rc::Rc<RefCell<Option<libadwaita::TimedAnimation>>> =
                    std::rc::Rc::new(RefCell::new(None));
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

pub(crate) fn spawn_cmd(cmd: &str) {
    let s = cmd.trim();

    // Route Rhai event handlers to the engine; fall back to shell for everything else.
    let is_rhai = s.ends_with(".rhai") || s.starts_with("rhai:");

    if is_rhai {
        #[cfg(feature = "rhai")]
        {
            if let Some(engine) = meh_rhai_engine::global() {
                let cmd = cmd.to_owned();
                let cdir = CONFIG_DIR
                    .get()
                    .cloned()
                    .unwrap_or_else(|| std::path::PathBuf::from("."));

                // Run the Rhai script on the tokio blocking thread pool with a
                // 500ms timeout enforced by the engine's operation limit.
                // Any returned string is executed as a shell command so scripts
                // can emit `"meh2 update VAR=value"` as their return value.
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
                                // Non-empty return value → run as shell command.
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
        }
        // No engine (feature disabled or not yet init) — fall through to shell.
        tracing::warn!("rhai event handler ignored (engine unavailable): {s}");
        return;
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

fn parse_align(s: &str) -> gtk4::Align {
    match s.to_lowercase().as_str() {
        "start" | "left" | "top" => gtk4::Align::Start,
        "end" | "right" | "bottom" => gtk4::Align::End,
        "center" => gtk4::Align::Center,
        "fill" | "baseline" => gtk4::Align::Fill,
        _ => gtk4::Align::Fill,
    }
}

fn parse_orientation(s: &str) -> gtk4::Orientation {
    match s.to_lowercase().as_str() {
        "h" | "horizontal" => gtk4::Orientation::Horizontal,
        _ => gtk4::Orientation::Vertical,
    }
}

// ── Box ───────────────────────────────────────────────────────────────────────

fn make_box_widget(wu: &BasicWidgetUse, ctx: &EvalCtx) -> Result<gtk4::Box> {
    let attrs = &wu.attrs;
    let orientation = ctx
        .eval_attr_str(attrs, "orientation")
        .as_deref()
        .map(parse_orientation)
        .unwrap_or(gtk4::Orientation::Horizontal);
    let spacing = ctx.eval_attr_i64(attrs, "spacing").unwrap_or(0) as i32;

    let b = gtk4::Box::new(orientation, spacing);

    if let Some(v) = ctx.eval_attr_bool(attrs, "space-evenly") {
        b.set_homogeneous(v);
    }
    Ok(b)
}

fn build_box(wu: &BasicWidgetUse, ctx: &EvalCtx) -> Result<gtk4::Box> {
    let b = make_box_widget(wu, ctx)?;
    let orientation = b.orientation();
    for child in &wu.children {
        let result = match child {
            WidgetUse::Loop(lp) => build_loop_oriented(lp, ctx, orientation),
            _ => build_widget(child, ctx),
        };
        match result {
            Ok(w) => b.append(&w),
            Err(e) => tracing::warn!("box child error: {}", e),
        }
    }
    apply_common_props(&b, wu, ctx);
    Ok(b)
}

// ── CenterBox ─────────────────────────────────────────────────────────────────

fn build_centerbox(wu: &BasicWidgetUse, ctx: &EvalCtx) -> Result<gtk4::CenterBox> {
    let b = gtk4::CenterBox::new();
    let attrs = &wu.attrs;
    let orientation = ctx
        .eval_attr_str(attrs, "orientation")
        .as_deref()
        .map(parse_orientation)
        .unwrap_or(gtk4::Orientation::Horizontal);
    b.set_orientation(orientation);

    let mut children = wu.children.iter().filter_map(|c| build_widget(c, ctx).ok());

    if let Some(w) = children.next() {
        b.set_start_widget(Some(&w));
    }
    if let Some(w) = children.next() {
        b.set_center_widget(Some(&w));
    }
    if let Some(w) = children.next() {
        b.set_end_widget(Some(&w));
    }

    apply_common_props(&b, wu, ctx);
    Ok(b)
}

// ── EventBox ─────────────────────────────────────────────────────────────────

fn build_eventbox(wu: &BasicWidgetUse, ctx: &EvalCtx) -> Result<gtk4::Box> {
    let b = gtk4::Box::new(gtk4::Orientation::Horizontal, 0);
    for child in &wu.children {
        match build_widget(child, ctx) {
            Ok(w) => b.append(&w),
            Err(e) => tracing::warn!("eventbox child error: {}", e),
        }
    }
    apply_common_props(&b, wu, ctx);
    Ok(b)
}

// ── Label ─────────────────────────────────────────────────────────────────────

#[allow(deprecated)]
fn build_label(wu: &BasicWidgetUse, ctx: &EvalCtx) -> Result<gtk4::Label> {
    let attrs = &wu.attrs;
    let text = ctx.eval_attr_str(attrs, "text").unwrap_or_default();
    let label = gtk4::Label::new(None);

    let markup = ctx.eval_attr_bool(attrs, "markup").unwrap_or(false);
    let use_markup = ctx.eval_attr_bool(attrs, "use-markup").unwrap_or(markup);

    if use_markup {
        label.set_markup(&text);
    } else {
        label.set_text(&text);
    }

    // Reactive binding: re-evaluate text expression when vars change.
    {
        let lc = label.clone();
        maybe_bind(attrs, "text", &ctx.scope, text.clone(), move |v| {
            if use_markup {
                lc.set_markup(&v);
            } else {
                lc.set_text(&v);
            }
        });
    }

    if let Some(w) = ctx.eval_attr_i64(attrs, "limit-width") {
        label.set_max_width_chars(w as i32);
        label.set_ellipsize(gtk4::pango::EllipsizeMode::End);
    }
    if let Some(w) = ctx.eval_attr_i64(attrs, "width-chars") {
        label.set_width_chars(w as i32);
    }
    if let Some(justify) = ctx.eval_attr_str(attrs, "justify") {
        label.set_justify(match justify.as_str() {
            "left" => gtk4::Justification::Left,
            "right" => gtk4::Justification::Right,
            "center" => gtk4::Justification::Center,
            "fill" => gtk4::Justification::Fill,
            _ => gtk4::Justification::Left,
        });
    }
    if ctx.eval_attr_bool(attrs, "wrap").unwrap_or(false) {
        label.set_wrap(true);
        label.set_wrap_mode(gtk4::pango::WrapMode::Word);
    }
    // Note: gtk4::Label removed set_angle; use a rotation transform via CSS instead
    if let Some(deg) = ctx.eval_attr_f64(attrs, "angle") {
        let style = format!("transform: rotate({}deg);", deg);
        let provider = gtk4::CssProvider::new();
        provider.load_from_string(&format!("label {{ {} }}", style));
        label
            .style_context()
            .add_provider(&provider, gtk4::STYLE_PROVIDER_PRIORITY_USER);
    }

    apply_common_props(&label, wu, ctx);
    Ok(label)
}

// ── Button ────────────────────────────────────────────────────────────────────

fn build_button(wu: &BasicWidgetUse, ctx: &EvalCtx) -> Result<gtk4::Button> {
    let button = gtk4::Button::new();
    // Build content from children or label attribute
    if wu.children.is_empty() {
        if let Some(text) = ctx.eval_attr_str(&wu.attrs, "label") {
            button.set_label(&text);
            let bc = button.clone();
            maybe_bind(&wu.attrs, "label", &ctx.scope, text, move |v| {
                bc.set_label(&v);
            });
        }
    } else {
        let container = gtk4::Box::new(gtk4::Orientation::Horizontal, 0);
        for child in &wu.children {
            match build_widget(child, ctx) {
                Ok(w) => {
                    container.append(&w);
                }
                Err(e) => tracing::warn!("button child error: {}", e),
            }
        }
        button.set_child(Some(&container));
    }
    apply_common_props(&button, wu, ctx);
    Ok(button)
}

// ── Image ─────────────────────────────────────────────────────────────────────

fn build_image(wu: &BasicWidgetUse, ctx: &EvalCtx) -> Result<gtk4::Widget> {
    let attrs = &wu.attrs;

    if let Some(path) = ctx.eval_attr_str(attrs, "path") {
        // Use DrawingArea + Cairo for file images. gtk4::Image and gtk4::Picture
        // both mis-size on fractional-scale displays (1.5x etc.) because the texture's
        // reported logical size is physical/scale, not the requested logical dimensions.
        // DrawingArea's draw-func receives the correctly-allocated logical (w, h) and
        // Cairo scales correctly to device pixels — no HiDPI arithmetic needed here.
        let area = gtk4::DrawingArea::new();

        // Shared mutable path + cached pixbuf for the draw func.
        let shared: std::rc::Rc<std::cell::RefCell<(String, Option<gdk::gdk_pixbuf::Pixbuf>)>> =
            std::rc::Rc::new(std::cell::RefCell::new((path.clone(), None)));

        let shared_draw = shared.clone();
        area.set_draw_func(move |_, cr, w, h| {
            if w <= 0 || h <= 0 {
                return;
            }
            let mut state = shared_draw.borrow_mut();
            // Load (or reload) pixbuf at the actual logical dimensions.
            let needs_load = state
                .1
                .as_ref()
                .map(|pb| pb.width() != w || pb.height() != h)
                .unwrap_or(true);
            if needs_load {
                #[allow(deprecated)]
                let pb = gdk::gdk_pixbuf::Pixbuf::from_file_at_scale(&state.0, w, h, false).ok();
                state.1 = pb;
            }
            if let Some(pb) = &state.1 {
                cr.set_source_pixbuf(pb, 0.0, 0.0);
                let _ = cr.paint();
            }
        });

        // Reactive path — invalidate cache and queue redraw.
        let shared_bind = shared.clone();
        let area_bind = area.clone();
        maybe_bind(attrs, "path", &ctx.scope, path, move |new_path| {
            shared_bind.borrow_mut().0 = new_path;
            shared_bind.borrow_mut().1 = None; // invalidate
            area_bind.queue_draw();
        });

        apply_common_props(&area, wu, ctx);
        return Ok(area.upcast());
    }

    // Icon-name path — keep gtk4::Image for themed icon rendering.
    #[allow(deprecated)]
    let image = gtk4::Image::new();
    if let Some(icon) = ctx.eval_attr_str(attrs, "icon-name") {
        let size = ctx
            .eval_attr_i64(attrs, "icon-size")
            .map(|s| s as i32)
            .unwrap_or(16);
        image.set_from_gicon(&gtk4::gio::ThemedIcon::new(&icon).upcast::<gtk4::gio::Icon>());
        image.set_pixel_size(size);
    }
    apply_common_props(&image, wu, ctx);
    Ok(image.upcast())
}

// ── Scale ─────────────────────────────────────────────────────────────────────

fn build_scale(wu: &BasicWidgetUse, ctx: &EvalCtx) -> Result<gtk4::Scale> {
    let attrs = &wu.attrs;
    let orientation = ctx
        .eval_attr_str(attrs, "orientation")
        .as_deref()
        .map(parse_orientation)
        .unwrap_or(gtk4::Orientation::Horizontal);
    let min = ctx.eval_attr_f64(attrs, "min").unwrap_or(0.0);
    let max = ctx.eval_attr_f64(attrs, "max").unwrap_or(100.0);
    let value = ctx.eval_attr_f64(attrs, "value").unwrap_or(0.0);

    let adj = gtk4::Adjustment::new(value, min, max, 1.0, 10.0, 0.0);
    let scale = gtk4::Scale::new(orientation, Some(&adj));
    scale.set_draw_value(false);

    if let Some(inverted) = ctx.eval_attr_bool(attrs, "flipped") {
        scale.set_inverted(inverted);
    }
    if let Some(inverted) = ctx.eval_attr_bool(attrs, "inverted") {
        scale.set_inverted(inverted);
    }

    if let Some(cmd) = ctx.eval_attr_str(attrs, "onchange") {
        scale.connect_value_changed(move |s| {
            spawn_cmd(&cmd.replace("{}", &s.value().to_string()));
        });
    }

    {
        let sc = scale.clone();
        maybe_bind(attrs, "value", &ctx.scope, value.to_string(), move |v| {
            if let Ok(f) = v.parse::<f64>() {
                sc.set_value(f);
            }
        });
    }

    apply_common_props(&scale, wu, ctx);
    Ok(scale)
}

// ── Animations helpers (feature-gated) ────────────────────────────────────────

#[cfg(feature = "animations")]
fn ensure_adw_init() {
    static ADW_INIT: std::sync::OnceLock<()> = std::sync::OnceLock::new();
    ADW_INIT.get_or_init(|| {
        libadwaita::init().expect("libadwaita init failed");
    });
}

#[cfg(feature = "animations")]
fn parse_adw_easing(s: Option<&str>) -> libadwaita::Easing {
    match s
        .unwrap_or("ease-in-out-cubic")
        .to_lowercase()
        .replace('_', "-")
        .as_str()
    {
        "linear" => libadwaita::Easing::Linear,
        "ease-in" | "ease-in-quad" => libadwaita::Easing::EaseInQuad,
        "ease-out" | "ease-out-quad" => libadwaita::Easing::EaseOutQuad,
        "ease-in-out" | "ease-in-out-quad" => libadwaita::Easing::EaseInOutQuad,
        "ease-in-cubic" => libadwaita::Easing::EaseInCubic,
        "ease-out-cubic" => libadwaita::Easing::EaseOutCubic,
        "ease-in-out-cubic" => libadwaita::Easing::EaseInOutCubic,
        "ease-in-sine" => libadwaita::Easing::EaseInSine,
        "ease-out-sine" => libadwaita::Easing::EaseOutSine,
        "ease-in-out-sine" => libadwaita::Easing::EaseInOutSine,
        "ease-in-expo" => libadwaita::Easing::EaseInExpo,
        "ease-out-expo" => libadwaita::Easing::EaseOutExpo,
        "ease-in-out-expo" => libadwaita::Easing::EaseInOutExpo,
        "ease-in-back" => libadwaita::Easing::EaseInBack,
        "ease-out-back" => libadwaita::Easing::EaseOutBack,
        "ease-in-out-back" => libadwaita::Easing::EaseInOutBack,
        "ease-in-bounce" => libadwaita::Easing::EaseInBounce,
        "ease-out-bounce" => libadwaita::Easing::EaseOutBounce,
        "ease-in-out-bounce" => libadwaita::Easing::EaseInOutBounce,
        _ => libadwaita::Easing::EaseInOutCubic,
    }
}

// ── Progress ──────────────────────────────────────────────────────────────────

fn build_progress(wu: &BasicWidgetUse, ctx: &EvalCtx) -> Result<gtk4::ProgressBar> {
    let attrs = &wu.attrs;
    let bar = gtk4::ProgressBar::new();

    let value = ctx.eval_attr_f64(attrs, "value").unwrap_or(0.0);
    bar.set_fraction((value / 100.0).clamp(0.0, 1.0));

    #[cfg(feature = "animations")]
    {
        let duration = ctx.eval_attr_i64(attrs, "animate-duration").unwrap_or(0) as u32;
        let easing = parse_adw_easing(ctx.eval_attr_str(attrs, "animate-easing").as_deref());
        if duration > 0 {
            ensure_adw_init();
            let anim_holder: std::rc::Rc<RefCell<Option<libadwaita::TimedAnimation>>> =
                std::rc::Rc::new(RefCell::new(None));
            let ah = anim_holder.clone();
            let bc = bar.clone();
            maybe_bind(attrs, "value", &ctx.scope, value.to_string(), move |v| {
                if let Ok(f) = v.parse::<f64>() {
                    let to = (f / 100.0).clamp(0.0, 1.0);
                    if let Some(p) = ah.borrow().as_ref() {
                        p.pause();
                    }
                    let from = bc.fraction();
                    let bc2 = bc.clone();
                    let target = libadwaita::CallbackAnimationTarget::new(move |val| {
                        bc2.set_fraction(val);
                    });
                    let a = libadwaita::TimedAnimation::new(&bc, from, to, duration, target);
                    a.set_easing(easing);
                    a.set_follow_enable_animations_setting(true);
                    a.play();
                    *ah.borrow_mut() = Some(a);
                }
            });
        } else {
            let bc = bar.clone();
            maybe_bind(attrs, "value", &ctx.scope, value.to_string(), move |v| {
                if let Ok(f) = v.parse::<f64>() {
                    bc.set_fraction((f / 100.0).clamp(0.0, 1.0));
                }
            });
        }
    }
    #[cfg(not(feature = "animations"))]
    {
        let bc = bar.clone();
        maybe_bind(attrs, "value", &ctx.scope, value.to_string(), move |v| {
            if let Ok(f) = v.parse::<f64>() {
                bc.set_fraction((f / 100.0).clamp(0.0, 1.0));
            }
        });
    }

    let orientation = ctx
        .eval_attr_str(attrs, "orientation")
        .as_deref()
        .map(parse_orientation)
        .unwrap_or(gtk4::Orientation::Horizontal);
    bar.set_orientation(orientation);

    if let Some(inverted) = ctx.eval_attr_bool(attrs, "flipped") {
        bar.set_inverted(inverted);
    }

    apply_common_props(&bar, wu, ctx);
    Ok(bar)
}

// ── Circular Progress (custom widget from Ewwii) ──────────────────────────────

static RING_OVERLAY_CTR: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

fn build_circular_progress(wu: &BasicWidgetUse, ctx: &EvalCtx) -> Result<gtk4::Widget> {
    use std::{cell::Cell, f64::consts::PI, rc::Rc};
    let attrs = &wu.attrs;

    let value = ctx.eval_attr_f64(attrs, "value").unwrap_or(0.0);
    let thickness = ctx.eval_attr_f64(attrs, "thickness").unwrap_or(4.0);
    let clockwise = ctx.eval_attr_bool(attrs, "clockwise").unwrap_or(true);
    // `:start-at` (eww %) and `:start-angle` (degrees) both supported.
    // eww's start-at: 0=top, 25=right, 50=bottom, 75=left (percentage of circle).
    let start_angle = if let Some(pct) = ctx.eval_attr_f64(attrs, "start-at") {
        pct / 100.0 * 360.0 - 90.0
    } else {
        ctx.eval_attr_f64(attrs, "start-angle").unwrap_or(-90.0)
    };
    let explicit_fg = ctx.eval_attr_str(attrs, "foreground");
    let explicit_bg = ctx.eval_attr_str(attrs, "background");

    let size = ctx
        .eval_attr_i64(attrs, "width")
        .or_else(|| ctx.eval_attr_i64(attrs, "height"))
        .unwrap_or(24) as i32;

    let area = gtk4::DrawingArea::new();
    area.set_content_width(size);
    area.set_content_height(size);
    // No CSS provider needed: draw_func clears to transparent (Source operator)
    // before painting, so the GTK theme background is always overwritten.

    // Shared reactive value — updated by maybe_bind, read by draw_func.
    let value_cell = Rc::new(Cell::new(value));
    {
        let vc = value_cell.clone();
        let area_ref = area.clone();
        maybe_bind(attrs, "value", &ctx.scope, value.to_string(), move |v| {
            if let Ok(f) = v.parse::<f64>() {
                vc.set(f);
                area_ref.queue_draw();
            }
        });
    }

    area.set_draw_func(move |widget, cr, w, h| {
        // Erase whatever GTK painted as CSS background — we control all drawing.
        cr.set_operator(gtk4::cairo::Operator::Source);
        cr.set_source_rgba(0.0, 0.0, 0.0, 0.0);
        cr.paint().ok();
        cr.set_operator(gtk4::cairo::Operator::Over);

        let val = value_cell.get();
        let w = w as f64;
        let h = h as f64;
        let cx = w / 2.0;
        let cy = h / 2.0;
        let r = (w.min(h) / 2.0) - thickness / 2.0;
        let start = start_angle * PI / 180.0;
        let fraction = (val / 100.0).clamp(0.0, 1.0);
        let sweep = 2.0 * PI * fraction;

        // Background arc (full circle) — use explicit attr or transparent
        let bg_rgba = if let Some(ref c) = explicit_bg {
            parse_rgba_css(c)
        } else {
            (0.0, 0.0, 0.0, 0.0)
        };
        if bg_rgba.3 > 0.0 {
            cr.set_source_rgba(bg_rgba.0, bg_rgba.1, bg_rgba.2, bg_rgba.3);
            cr.set_line_width(thickness);
            cr.arc(cx, cy, r, 0.0, 2.0 * PI);
            let _ = cr.stroke();
        }

        // Foreground arc — use explicit attr, else read CSS `color` property
        let fg_rgba = if let Some(ref c) = explicit_fg {
            parse_rgba_css(c)
        } else {
            let c = widget.color();
            (
                c.red() as f64,
                c.green() as f64,
                c.blue() as f64,
                c.alpha() as f64,
            )
        };
        // Only draw if there's something to draw — cairo arc with equal angles draws a full circle.
        if sweep > 1e-6 {
            cr.set_source_rgba(fg_rgba.0, fg_rgba.1, fg_rgba.2, fg_rgba.3);
            cr.set_line_width(thickness);
            if clockwise {
                cr.arc(cx, cy, r, start, start + sweep);
            } else {
                cr.arc_negative(cx, cy, r, start, start - sweep);
            }
            let _ = cr.stroke();
        }
    });

    // If children present, wrap in an Overlay so icons/labels render on top.
    // CSS class goes on the Overlay when children are present:
    //   - `color` IS inherited → DrawingArea.widget.color() picks up the user's
    //     color rule (used for the foreground arc in draw_func).
    //   - `background-color` is NOT inherited → the user's `.stat-ring { background-color }
    //     rule on the Overlay never paints a grey rectangle behind the Cairo arcs.
    let widget: gtk4::Widget = if wu.children.is_empty() {
        apply_common_props(&area, wu, ctx);
        area.upcast()
    } else {
        let overlay = gtk4::Overlay::new();
        overlay.set_child(Some(&area));
        for child in &wu.children {
            if let Ok(w) = build_widget(child, ctx) {
                w.set_halign(gtk4::Align::Center);
                w.set_valign(gtk4::Align::Center);
                overlay.add_overlay(&w);
            }
        }
        overlay.set_size_request(size, size);
        // Force overlay background transparent at priority 901 (> user stylesheet 800).
        // GTK4 4.10+: style_context().add_provider() is deprecated; use a unique class
        // scoped to this widget instance and a display-level provider instead.
        let n = RING_OVERLAY_CTR.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let ov_class = format!("meh-ring-ov-{n}");
        overlay.add_css_class(&ov_class);
        let ov_provider = gtk4::CssProvider::new();
        ov_provider.load_from_string(&format!(".{ov_class} {{ background: transparent; }}"));
        if let Some(display) = gdk::Display::default() {
            gtk4::style_context_add_provider_for_display(&display, &ov_provider, 901);
            overlay.connect_destroy(move |_| {
                if let Some(d) = gdk::Display::default() {
                    gtk4::style_context_remove_provider_for_display(&d, &ov_provider);
                }
            });
        }
        // CSS class (e.g. .stat-ring) and onclick/events go on the overlay wrapper.
        apply_common_props(&overlay, wu, ctx);
        overlay.upcast()
    };

    Ok(widget)
}

fn parse_rgba_css(s: &str) -> (f64, f64, f64, f64) {
    let s = s.trim();
    // Try GTK's built-in parser first — handles #hex, rgb(), rgba(), named colors.
    if let Ok(rgba) = s.parse::<gtk4::gdk::RGBA>() {
        return (
            rgba.red() as f64,
            rgba.green() as f64,
            rgba.blue() as f64,
            rgba.alpha() as f64,
        );
    }
    // Manual hex fallback
    if s.starts_with('#') {
        let hex = s.trim_start_matches('#');
        let (r, g, b, a) = match hex.len() {
            6 => {
                let r = u8::from_str_radix(&hex[0..2], 16).unwrap_or(255) as f64 / 255.0;
                let g = u8::from_str_radix(&hex[2..4], 16).unwrap_or(255) as f64 / 255.0;
                let b = u8::from_str_radix(&hex[4..6], 16).unwrap_or(255) as f64 / 255.0;
                (r, g, b, 1.0)
            }
            8 => {
                let r = u8::from_str_radix(&hex[0..2], 16).unwrap_or(255) as f64 / 255.0;
                let g = u8::from_str_radix(&hex[2..4], 16).unwrap_or(255) as f64 / 255.0;
                let b = u8::from_str_radix(&hex[4..6], 16).unwrap_or(255) as f64 / 255.0;
                let a = u8::from_str_radix(&hex[6..8], 16).unwrap_or(255) as f64 / 255.0;
                (r, g, b, a)
            }
            _ => (1.0, 1.0, 1.0, 1.0),
        };
        return (r, g, b, a);
    }
    // transparent fallback
    (0.0, 0.0, 0.0, 0.0)
}

// ── Scroll ────────────────────────────────────────────────────────────────────

fn build_scroll(wu: &BasicWidgetUse, ctx: &EvalCtx) -> Result<gtk4::ScrolledWindow> {
    let sw = gtk4::ScrolledWindow::new();
    let attrs = &wu.attrs;

    let hscroll = ctx.eval_attr_bool(attrs, "hscroll").unwrap_or(true);
    let vscroll = ctx.eval_attr_bool(attrs, "vscroll").unwrap_or(true);
    sw.set_policy(
        if hscroll {
            gtk4::PolicyType::Automatic
        } else {
            gtk4::PolicyType::Never
        },
        if vscroll {
            gtk4::PolicyType::Automatic
        } else {
            gtk4::PolicyType::Never
        },
    );

    if let Some(child) = wu.children.first()
        && let Ok(w) = build_widget(child, ctx)
    {
        sw.set_child(Some(&w));
    }

    apply_common_props(&sw, wu, ctx);
    Ok(sw)
}

// ── Overlay ───────────────────────────────────────────────────────────────────

fn build_overlay(wu: &BasicWidgetUse, ctx: &EvalCtx) -> Result<gtk4::Overlay> {
    let overlay = gtk4::Overlay::new();
    let mut iter = wu.children.iter();
    if let Some(main) = iter.next()
        && let Ok(w) = build_widget(main, ctx)
    {
        overlay.set_child(Some(&w));
    }
    for extra in iter {
        if let Ok(w) = build_widget(extra, ctx) {
            overlay.add_overlay(&w);
        }
    }
    apply_common_props(&overlay, wu, ctx);
    Ok(overlay)
}

// ── Revealer ──────────────────────────────────────────────────────────────────

fn build_revealer(wu: &BasicWidgetUse, ctx: &EvalCtx) -> Result<gtk4::Revealer> {
    let revealer = gtk4::Revealer::new();
    let attrs = &wu.attrs;

    let transition = ctx
        .eval_attr_str(attrs, "transition")
        .as_deref()
        .map(parse_revealer_transition)
        .unwrap_or(gtk4::RevealerTransitionType::SlideDown);
    revealer.set_transition_type(transition);

    if let Some(dur) = ctx.eval_attr_i64(attrs, "duration") {
        revealer.set_transition_duration(dur as u32);
    }

    let reveal = ctx.eval_attr_bool(attrs, "reveal").unwrap_or(true);
    revealer.set_reveal_child(reveal);

    {
        let rc = revealer.clone();
        maybe_bind(attrs, "reveal", &ctx.scope, reveal.to_string(), move |v| {
            rc.set_reveal_child(v == "true");
        });
    }

    if let Some(child) = wu.children.first()
        && let Ok(w) = build_widget(child, ctx)
    {
        revealer.set_child(Some(&w));
    }

    apply_common_props(&revealer, wu, ctx);
    Ok(revealer)
}

fn parse_revealer_transition(s: &str) -> gtk4::RevealerTransitionType {
    match s.to_lowercase().as_str() {
        "slideright" | "slide_right" => gtk4::RevealerTransitionType::SlideRight,
        "slideleft" | "slide_left" => gtk4::RevealerTransitionType::SlideLeft,
        "slideup" | "slide_up" => gtk4::RevealerTransitionType::SlideUp,
        "crossfade" | "cross_fade" => gtk4::RevealerTransitionType::Crossfade,
        "none" => gtk4::RevealerTransitionType::None,
        _ => gtk4::RevealerTransitionType::SlideDown,
    }
}

// ── Stack ─────────────────────────────────────────────────────────────────────

fn build_stack(wu: &BasicWidgetUse, ctx: &EvalCtx) -> Result<gtk4::Stack> {
    let stack = gtk4::Stack::new();
    let attrs = &wu.attrs;

    if let Some(transition) = ctx.eval_attr_str(attrs, "transition") {
        let t = match transition.as_str() {
            "slide-right" | "slideright" => gtk4::StackTransitionType::SlideRight,
            "slide-left" | "slideleft" => gtk4::StackTransitionType::SlideLeft,
            "slide-up" | "slideup" => gtk4::StackTransitionType::SlideUp,
            "slide-down" | "slidedown" => gtk4::StackTransitionType::SlideDown,
            "crossfade" => gtk4::StackTransitionType::Crossfade,
            _ => gtk4::StackTransitionType::None,
        };
        stack.set_transition_type(t);
    }

    if let Some(shown) = ctx.eval_attr_str(attrs, "shown") {
        for child in &wu.children {
            if let WidgetUse::Basic(b) = child
                && let Some(name) = ctx
                    .eval_attr_str(&b.attrs, "name")
                    .or_else(|| Some(b.name.clone()))
                && let Ok(w) = build_widget(child, ctx)
            {
                stack.add_named(&w, Some(&name));
            }
        }
        stack.set_visible_child_name(&shown);
    } else {
        for child in &wu.children {
            if let Ok(w) = build_widget(child, ctx) {
                stack.add_child(&w);
            }
        }
    }

    apply_common_props(&stack, wu, ctx);
    Ok(stack)
}

// ── Expander ──────────────────────────────────────────────────────────────────

fn build_expander(wu: &BasicWidgetUse, ctx: &EvalCtx) -> Result<gtk4::Expander> {
    let attrs = &wu.attrs;
    let label = ctx.eval_attr_str(attrs, "label").unwrap_or_default();
    let exp = gtk4::Expander::new(Some(&label));
    exp.set_expanded(ctx.eval_attr_bool(attrs, "expanded").unwrap_or(false));

    if let Some(child) = wu.children.first()
        && let Ok(w) = build_widget(child, ctx)
    {
        exp.set_child(Some(&w));
    }

    apply_common_props(&exp, wu, ctx);
    Ok(exp)
}

// ── Checkbox ─────────────────────────────────────────────────────────────────

fn build_checkbox(wu: &BasicWidgetUse, ctx: &EvalCtx) -> Result<gtk4::CheckButton> {
    let attrs = &wu.attrs;
    let cb = gtk4::CheckButton::new();
    if let Some(v) = ctx.eval_attr_bool(attrs, "checked") {
        cb.set_active(v);
    }
    if let Some(label) = ctx.eval_attr_str(attrs, "label") {
        cb.set_label(Some(&label));
    }

    if let Some(cmd) = ctx.eval_attr_str(attrs, "ontoggle") {
        cb.connect_toggled(move |b| {
            spawn_cmd(&cmd.replace("{}", &b.is_active().to_string()));
        });
    }

    apply_common_props(&cb, wu, ctx);
    Ok(cb)
}

// ── Input ─────────────────────────────────────────────────────────────────────

fn build_input(wu: &BasicWidgetUse, ctx: &EvalCtx) -> Result<gtk4::Entry> {
    let attrs = &wu.attrs;
    let entry = gtk4::Entry::new();

    // :value — reactive; bind a var to clear/reset the field (e.g. launcher pattern)
    let initial_value = ctx.eval_attr_str(attrs, "value").unwrap_or_default();
    entry.set_text(&initial_value);
    {
        let e = entry.clone();
        maybe_bind(attrs, "value", &ctx.scope, initial_value, move |v| {
            e.set_text(&v);
        });
    }

    if let Some(v) = ctx.eval_attr_str(attrs, "placeholder") {
        entry.set_placeholder_text(Some(&v));
    }

    if let Some(cmd) = ctx.eval_attr_str(attrs, "onchange") {
        entry.connect_changed(move |e| {
            spawn_cmd(&cmd.replace("{}", &e.text()));
        });
    }
    if let Some(cmd) = ctx.eval_attr_str(attrs, "onaccept") {
        entry.connect_activate(move |e| {
            spawn_cmd(&cmd.replace("{}", &e.text()));
        });
    }

    // :focus — grab keyboard focus on map (static "true") or when var flips to "true"
    // Useful for launchers: open the window, then `meh update FOCUS=true`
    let initial_focus = ctx.eval_attr_str(attrs, "focus").unwrap_or_default();
    if initial_focus == "true" {
        let e = entry.clone();
        entry.connect_map(move |_| {
            e.grab_focus();
        });
    }
    {
        let e = entry.clone();
        maybe_bind(attrs, "focus", &ctx.scope, initial_focus, move |v| {
            if v == "true" {
                e.grab_focus();
            }
        });
    }

    apply_common_props(&entry, wu, ctx);
    Ok(entry)
}

// ── Calendar ──────────────────────────────────────────────────────────────────

fn build_calendar(wu: &BasicWidgetUse, ctx: &EvalCtx) -> Result<gtk4::Calendar> {
    let cal = gtk4::Calendar::new();
    if let Some(cmd) = ctx.eval_attr_str(&wu.attrs, "onclick") {
        cal.connect_day_selected(move |c| {
            let date = c.date();
            spawn_cmd(&cmd.replace(
                "{}",
                &format!(
                    "{}-{:02}-{:02}",
                    date.year(),
                    date.month() as u32,
                    date.day_of_month()
                ),
            ));
        });
    }
    apply_common_props(&cal, wu, ctx);
    Ok(cal)
}

// ── ComboBoxText ──────────────────────────────────────────────────────────────

#[allow(deprecated)]
fn build_combo_box_text(wu: &BasicWidgetUse, ctx: &EvalCtx) -> Result<gtk4::ComboBoxText> {
    let attrs = &wu.attrs;
    let combo = gtk4::ComboBoxText::new();

    if let Some(items) = ctx.eval_attr(attrs, "items")
        && let Ok(arr) = items.as_vec()
    {
        for item in &arr {
            combo.append_text(item);
        }
    }
    if let Some(active) = ctx.eval_attr_i64(attrs, "active") {
        combo.set_active(Some(active as u32));
    }

    if let Some(cmd) = ctx.eval_attr_str(attrs, "onchange") {
        combo.connect_changed(move |c| {
            let v = c.active_text().map(|s| s.to_string()).unwrap_or_default();
            spawn_cmd(&cmd.replace("{}", &v));
        });
    }

    apply_common_props(&combo, wu, ctx);
    Ok(combo)
}

// ── ColorButton ───────────────────────────────────────────────────────────────

#[allow(deprecated)]
fn build_color_button(wu: &BasicWidgetUse, ctx: &EvalCtx) -> Result<gtk4::ColorButton> {
    let cb = gtk4::ColorButton::new();
    if let Some(cmd) = ctx.eval_attr_str(&wu.attrs, "onchange") {
        cb.connect_color_set(move |b| {
            let c = b.rgba();
            spawn_cmd(&cmd.replace(
                "{}",
                &format!(
                    "rgba({},{},{},{})",
                    (c.red() * 255.0) as u8,
                    (c.green() * 255.0) as u8,
                    (c.blue() * 255.0) as u8,
                    c.alpha()
                ),
            ));
        });
    }
    apply_common_props(&cb, wu, ctx);
    Ok(cb)
}

// ── Literal ───────────────────────────────────────────────────────────────────

fn build_literal(wu: &BasicWidgetUse, ctx: &EvalCtx) -> Result<gtk4::Label> {
    // `literal` is just a raw label that can contain markup
    let text = ctx.eval_attr_str(&wu.attrs, "content").unwrap_or_default();
    let label = gtk4::Label::new(None);
    label.set_markup(&text);
    {
        let lc = label.clone();
        maybe_bind(&wu.attrs, "content", &ctx.scope, text, move |v| {
            lc.set_markup(&v);
        });
    }
    apply_common_props(&label, wu, ctx);
    Ok(label)
}

// ── Systray ───────────────────────────────────────────────────────────────────

#[cfg(feature = "systray")]
fn build_systray(wu: &BasicWidgetUse, ctx: &EvalCtx) -> Result<gtk4::Box> {
    use std::{cell::RefCell, collections::HashMap, rc::Rc};

    let container = gtk4::Box::new(gtk4::Orientation::Horizontal, 4);
    apply_common_props(&container.clone().upcast::<gtk4::Widget>(), wu, ctx);

    // mpsc channel: tx is Send (moved to tokio task), rx polled on GTK thread.
    let (tx, rx) = std::sync::mpsc::channel::<SystrayEvent>();
    let rx = Rc::new(RefCell::new(rx));

    let items: Rc<RefCell<HashMap<String, gtk4::Button>>> = Default::default();
    let items2 = items.clone();
    let container2 = container.clone();

    // Poll for tray events every 50 ms on the GTK thread.
    gtk4::glib::timeout_add_local(std::time::Duration::from_millis(50), move || {
        loop {
            match rx.borrow().try_recv() {
                Ok(ev) => systray_handle_event(ev, &container2, &items2),
                Err(std::sync::mpsc::TryRecvError::Empty) => break,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    return gtk4::glib::ControlFlow::Break;
                }
            }
        }
        gtk4::glib::ControlFlow::Continue
    });

    let Some(handle) = TOKIO_HANDLE.get() else {
        tracing::warn!("systray: no tokio handle set; tray will be empty");
        return Ok(container);
    };

    handle.spawn(async move {
        if let Err(e) = run_notifier_host_task(tx).await {
            tracing::error!("systray host error: {}", e);
        }
    });

    Ok(container)
}

#[cfg(feature = "systray")]
enum SystrayEvent {
    Add {
        id: String,
        icon: meh_notifier_host::IconResult,
        tooltip: Option<String>,
        item: meh_notifier_host::Item,
    },
    Remove {
        id: String,
    },
}

#[cfg(feature = "systray")]
fn systray_handle_event(
    ev: SystrayEvent,
    container: &gtk4::Box,
    items: &std::rc::Rc<std::cell::RefCell<std::collections::HashMap<String, gtk4::Button>>>,
) {
    use std::sync::Arc;
    match ev {
        SystrayEvent::Add {
            id,
            icon,
            tooltip,
            item,
        } => {
            let img = systray_icon_to_image(icon);
            img.set_pixel_size(24);

            let btn = gtk4::Button::new();
            btn.set_child(Some(&img));
            if let Some(tip) = tooltip {
                btn.set_tooltip_text(Some(&tip));
            }

            let item = Arc::new(item);
            btn.connect_clicked(move |_| {
                let item = item.clone();
                if let Some(handle) = TOKIO_HANDLE.get() {
                    handle.spawn(async move {
                        let _ = item.sni.activate(0, 0).await;
                    });
                }
            });

            container.append(&btn);
            items.borrow_mut().insert(id, btn);
        }
        SystrayEvent::Remove { id } => {
            if let Some(btn) = items.borrow_mut().remove(&id) {
                container.remove(&btn);
            }
        }
    }
}

#[cfg(feature = "systray")]
fn systray_icon_to_image(icon: meh_notifier_host::IconResult) -> gtk4::Image {
    use meh_notifier_host::IconResult;
    match icon {
        IconResult::Named {
            name,
            theme_path: None,
        } => {
            if std::path::Path::new(&name).is_absolute() {
                let img = gtk4::Image::new();
                if let Ok(texture) = gtk4::gdk::Texture::from_filename(&name) {
                    img.set_paintable(Some(&texture));
                }
                img
            } else {
                gtk4::Image::from_icon_name(&name)
            }
        }
        IconResult::Named {
            name,
            theme_path: Some(tp),
        } => {
            let theme = gtk4::IconTheme::new();
            theme.add_search_path(&tp);
            let paintable = theme.lookup_icon(
                &name,
                &[],
                24,
                1,
                gtk4::TextDirection::None,
                gtk4::IconLookupFlags::empty(),
            );
            gtk4::Image::from_paintable(Some(&paintable))
        }
        IconResult::Pixmap {
            width,
            height,
            rgba,
        } => {
            let bytes = gtk4::glib::Bytes::from_owned(rgba);
            let texture = gtk4::gdk::MemoryTexture::new(
                width,
                height,
                gtk4::gdk::MemoryFormat::R8g8b8a8,
                &bytes,
                (width * 4) as usize,
            );
            gtk4::Image::from_paintable(Some(&texture))
        }
        IconResult::Missing => gtk4::Image::from_icon_name("image-missing"),
    }
}

#[cfg(feature = "systray")]
async fn run_notifier_host_task(tx: std::sync::mpsc::Sender<SystrayEvent>) -> anyhow::Result<()> {
    use meh_notifier_host::{Host, Item, Watcher, register_as_host, run_host};

    struct MehHost {
        tx: std::sync::mpsc::Sender<SystrayEvent>,
    }

    impl Host for MehHost {
        fn add_item(&mut self, id: &str, item: Item) {
            let id = id.to_owned();
            let tx = self.tx.clone();
            tokio::spawn(async move {
                let icon = item.load_icon_result(24).await;
                let tooltip = item.sni.title().await.ok().filter(|s| !s.is_empty());
                let _ = tx.send(SystrayEvent::Add {
                    id,
                    icon,
                    tooltip,
                    item,
                });
            });
        }

        fn remove_item(&mut self, id: &str) {
            let _ = self.tx.send(SystrayEvent::Remove { id: id.to_owned() });
        }
    }

    let con = zbus::Connection::session().await?;
    Watcher::new().attach_to(&con).await?;
    let (_name, snw) = register_as_host(&con).await?;
    let mut host = MehHost { tx };
    let err = run_host(&mut host, &snw).await;
    Err(anyhow::anyhow!("notifier host: {}", err))
}

// ── Shader widget (GtkGLArea + glow) ─────────────────────────────────────────

#[cfg(feature = "shader")]
const VERT_SRC: &str = concat!(
    "#version 330 core\n",
    "const vec2 VERTS[3] = vec2[3](vec2(-1,-1), vec2(3,-1), vec2(-1,3));\n",
    "void main() { gl_Position = vec4(VERTS[gl_VertexID], 0.0, 1.0); }\n",
);

#[cfg(feature = "shader")]
struct ShaderGlState {
    gl: glow::Context,
    program: glow::NativeProgram,
    vao: glow::NativeVertexArray,
    loc_time: Option<glow::NativeUniformLocation>,
    loc_resolution: Option<glow::NativeUniformLocation>,
    start: std::time::Instant,
    w: f32,
    h: f32,
}

/// Load GL function pointers via EGL (the GTK4 GL backend on Wayland).
/// Must be called only when an EGL context is current (inside `connect_realize`
/// after `area.make_current()`).
#[cfg(feature = "shader")]
fn egl_loader(name: &std::ffi::CStr) -> *const std::ffi::c_void {
    unsafe extern "C" {
        fn eglGetProcAddress(procname: *const std::ffi::c_char) -> *const std::ffi::c_void;
    }
    unsafe { eglGetProcAddress(name.as_ptr()) }
}

#[cfg(feature = "shader")]
fn compile_program(gl: &glow::Context, vert: &str, frag: &str) -> Result<glow::NativeProgram> {
    use glow::HasContext;
    unsafe {
        let vs = gl
            .create_shader(glow::VERTEX_SHADER)
            .map_err(|e| anyhow::anyhow!("create vertex shader: {}", e))?;
        gl.shader_source(vs, vert);
        gl.compile_shader(vs);
        if !gl.get_shader_compile_status(vs) {
            let log = gl.get_shader_info_log(vs);
            gl.delete_shader(vs);
            anyhow::bail!("vertex shader: {}", log);
        }

        let fs = gl
            .create_shader(glow::FRAGMENT_SHADER)
            .map_err(|e| anyhow::anyhow!("create fragment shader: {}", e))?;
        gl.shader_source(fs, frag);
        gl.compile_shader(fs);
        if !gl.get_shader_compile_status(fs) {
            let log = gl.get_shader_info_log(fs);
            gl.delete_shader(fs);
            gl.delete_shader(vs);
            anyhow::bail!("fragment shader: {}", log);
        }

        let prog = gl
            .create_program()
            .map_err(|e| anyhow::anyhow!("create program: {}", e))?;
        gl.attach_shader(prog, vs);
        gl.attach_shader(prog, fs);
        gl.link_program(prog);
        gl.detach_shader(prog, vs);
        gl.delete_shader(vs);
        gl.detach_shader(prog, fs);
        gl.delete_shader(fs);

        if !gl.get_program_link_status(prog) {
            let log = gl.get_program_info_log(prog);
            gl.delete_program(prog);
            anyhow::bail!("program link: {}", log);
        }
        Ok(prog)
    }
}

/// `(shader :frag "path/to/shader.glsl" :width 300 :height 200)`
///
/// Renders a GLSL fragment shader via GtkGLArea (OpenGL 3.3 core).
/// Available uniforms in the fragment shader:
/// - `uniform float iTime`      — seconds since the widget was realized
/// - `uniform vec2  iResolution` — widget dimensions in pixels
///
/// Only available in the `full` build profile (`--features shader`).
#[cfg(feature = "shader")]
fn build_shader(wu: &BasicWidgetUse, ctx: &EvalCtx) -> Result<gtk4::GLArea> {
    use glow::HasContext;
    use gtk4::prelude::*;
    use std::{cell::RefCell, rc::Rc};

    let frag_path = ctx
        .eval_attr_str(&wu.attrs, "frag")
        .ok_or_else(|| anyhow::anyhow!("`shader` widget requires `:frag` attribute"))?;

    // Solid magenta fallback — visually obvious indicator of a missing shader file.
    let frag_src = std::fs::read_to_string(&frag_path).unwrap_or_else(|e| {
        tracing::warn!("shader: cannot read `{}`: {}", frag_path, e);
        "#version 330 core\nout vec4 fragColor;\nvoid main(){fragColor=vec4(1,0,1,1);}".to_string()
    });

    let area = gtk4::GLArea::new();
    area.set_required_version(3, 3);
    area.set_use_es(false);
    area.set_has_depth_buffer(false);
    area.set_has_stencil_buffer(false);
    area.set_auto_render(true);

    if let Some(w) = ctx.eval_attr_i64(&wu.attrs, "width") {
        area.set_width_request(w as i32);
    }
    if let Some(h) = ctx.eval_attr_i64(&wu.attrs, "height") {
        area.set_height_request(h as i32);
    }
    apply_common_props(&area.clone().upcast::<gtk4::Widget>(), wu, ctx);

    let state: Rc<RefCell<Option<ShaderGlState>>> = Rc::new(RefCell::new(None));

    // realize: compile shaders, create VAO
    {
        let state = state.clone();
        area.connect_realize(move |area| {
            area.make_current();
            if area.error().is_some() {
                return;
            }
            let gl = unsafe { glow::Context::from_loader_function_cstr(egl_loader) };
            match compile_program(&gl, VERT_SRC, &frag_src) {
                Ok(program) => {
                    let vao = unsafe { gl.create_vertex_array().expect("create vao") };
                    let loc_time = unsafe { gl.get_uniform_location(program, "iTime") };
                    let loc_resolution = unsafe { gl.get_uniform_location(program, "iResolution") };
                    *state.borrow_mut() = Some(ShaderGlState {
                        gl,
                        program,
                        vao,
                        loc_time,
                        loc_resolution,
                        start: std::time::Instant::now(),
                        w: 1.0,
                        h: 1.0,
                    });
                }
                Err(e) => tracing::warn!("shader compile failed for `{}`: {}", frag_path, e),
            }
        });
    }

    // unrealize: free GL objects before the context is destroyed
    {
        let state = state.clone();
        area.connect_unrealize(move |area| {
            area.make_current();
            if let Some(s) = state.borrow_mut().take() {
                unsafe {
                    s.gl.delete_vertex_array(s.vao);
                    s.gl.delete_program(s.program);
                }
            }
        });
    }

    // resize: update iResolution
    {
        let state = state.clone();
        area.connect_resize(move |_area, w, h| {
            if let Some(s) = state.borrow_mut().as_mut() {
                s.w = w as f32;
                s.h = h as f32;
            }
        });
    }

    // render: draw one fullscreen triangle per frame
    {
        let state = state.clone();
        area.connect_render(move |_area, _gl_ctx| {
            let borrowed = state.borrow();
            if let Some(s) = borrowed.as_ref() {
                unsafe {
                    s.gl.clear_color(0.0, 0.0, 0.0, 0.0);
                    s.gl.clear(glow::COLOR_BUFFER_BIT);
                    s.gl.use_program(Some(s.program));
                    if let Some(loc) = s.loc_time.as_ref() {
                        s.gl.uniform_1_f32(Some(loc), s.start.elapsed().as_secs_f32());
                    }
                    if let Some(loc) = s.loc_resolution.as_ref() {
                        s.gl.uniform_2_f32(Some(loc), s.w, s.h);
                    }
                    s.gl.bind_vertex_array(Some(s.vao));
                    s.gl.draw_arrays(glow::TRIANGLES, 0, 3);
                }
            }
            gtk4::glib::Propagation::Proceed
        });
    }

    Ok(area)
}
