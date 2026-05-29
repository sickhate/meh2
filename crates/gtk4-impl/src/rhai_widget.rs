// GPL-3.0-or-later
//! `(rhai-widget :src "path.rhai" :fn "build" :watch "VAR1 VAR2")`
//!
//! Calls a Rhai function that returns a widget-tree map, converts it to
//! WidgetUse, and renders it. When `:watch` vars change, the subtree is
//! transparently rebuilt in-place.

use std::{collections::HashMap, path::PathBuf};

use anyhow::Result;
use eww_shared_util::{AttrName, Span, VarName};
use gtk4::prelude::*;
use meh_core::EvalCtx;
use simplexpr::{SimplExpr, dynval::DynVal};
use yuck::config::{
    attributes::{AttrEntry, Attributes},
    widget_definition::WidgetDefinition,
    widget_use::{BasicWidgetUse, WidgetUse},
};
use yuck::parser::ast::Ast;

use crate::{AnyBinding, BINDING_COLLECTOR, CONFIG_DIR, build_widget};

// ── Reactive binding ──────────────────────────────────────────────────────────

pub struct RhaiWidgetBinding {
    pub(crate) watched_vars: Vec<VarName>,
    /// Call-site attribute values evaluated at build time; merged with
    /// watched-var values on every rebuild so plugins see both.
    pub(crate) static_attrs: HashMap<String, String>,
    pub(crate) scope: HashMap<VarName, DynVal>,
    pub(crate) widget_defs: HashMap<String, WidgetDefinition>,
    pub(crate) script_path: PathBuf,
    pub(crate) fn_name: String,
    pub(crate) container: gtk4::Box,
    pub(crate) last_watched_vals: HashMap<VarName, String>,
}

impl RhaiWidgetBinding {
    pub fn update(&mut self, global_vars: &HashMap<VarName, DynVal>) -> bool {
        let mut changed = false;
        for var in &self.watched_vars {
            let new_val = global_vars
                .get(var)
                .map(|v| v.0.clone())
                .unwrap_or_default();
            let old_val = self.last_watched_vals.get(var).cloned().unwrap_or_default();
            if new_val != old_val {
                changed = true;
                self.last_watched_vals.insert(var.clone(), new_val);
            }
        }
        if !changed {
            return false;
        }
        self.rebuild(global_vars);
        true
    }

    pub fn intersects(&self, changed: &std::collections::HashSet<VarName>) -> bool {
        self.watched_vars.iter().any(|v| changed.contains(v))
    }

    fn rebuild(&self, global_vars: &HashMap<VarName, DynVal>) {
        let Some(engine) = meh_rhai_engine::global() else {
            tracing::warn!("rhai-widget: engine unavailable on rebuild");
            return;
        };
        let config_dir = CONFIG_DIR
            .get()
            .map(|p| p.as_path())
            .unwrap_or(std::path::Path::new("."));

        // Merge static call-site attrs with current watched-var values;
        // watched vars take precedence so the latest value always wins.
        let mut vars = self.static_attrs.clone();
        for v in &self.watched_vars {
            let val = global_vars
                .get(v)
                .map(|dv| dv.0.clone())
                .unwrap_or_default();
            vars.insert(v.0.clone(), val);
        }

        let data = match engine.call_fn_as_widget_data(
            &self.script_path,
            config_dir,
            &self.fn_name,
            &vars,
        ) {
            Ok(d) => d,
            Err(e) => {
                tracing::error!("rhai-widget {}: {}", self.fn_name, e);
                return;
            }
        };

        let wu = rhai_data_to_widget_use(data, Span::DUMMY);
        let ctx = EvalCtx {
            scope: self.scope.clone(),
            global_vars,
            widget_defs: &self.widget_defs,
        };

        match build_widget(&wu, &ctx) {
            Ok(new_child) => {
                while let Some(child) = self.container.first_child() {
                    self.container.remove(&child);
                }
                self.container.append(&new_child);
            }
            Err(e) => tracing::error!("rhai-widget rebuild {}: {}", self.fn_name, e),
        }
    }
}

// ── Builder ───────────────────────────────────────────────────────────────────

pub fn build_rhai_widget(wu: &BasicWidgetUse, ctx: &EvalCtx) -> Result<gtk4::Widget> {
    let get_str = |key: &str| -> String {
        wu.attrs
            .attrs
            .get(&AttrName(key.to_string()))
            .and_then(|e| e.value.as_simplexpr().ok())
            .and_then(|expr| ctx.eval_expr(&expr).ok())
            .map(|v| v.0)
            .unwrap_or_default()
    };

    let src = get_str("src");
    let fn_name = get_str("fn");
    let watch = get_str("watch");

    if src.is_empty() || fn_name.is_empty() {
        anyhow::bail!("rhai-widget requires :src and :fn attributes");
    }

    let config_dir = CONFIG_DIR
        .get()
        .map(|p| p.as_path())
        .unwrap_or(std::path::Path::new("."));

    let script_path = if std::path::Path::new(&src).is_absolute() {
        PathBuf::from(&src)
    } else {
        config_dir.join(&src)
    };

    let Some(engine) = meh_rhai_engine::global() else {
        anyhow::bail!("rhai-widget: Rhai engine not initialised");
    };

    let watched_vars: Vec<VarName> = watch
        .split_whitespace()
        .filter(|s| !s.is_empty())
        .map(|s| VarName(s.to_string()))
        .collect();

    let initial_vars: std::collections::HashMap<String, String> = watched_vars
        .iter()
        .map(|v| {
            let val = ctx
                .global_vars
                .get(v)
                .map(|dv| dv.0.clone())
                .unwrap_or_default();
            (v.0.clone(), val)
        })
        .collect();

    let data = engine.call_fn_as_widget_data(&script_path, config_dir, &fn_name, &initial_vars)?;
    let inner_wu = rhai_data_to_widget_use(data, wu.span);
    let inner = build_widget(&inner_wu, ctx)?;

    let container = gtk4::Box::new(gtk4::Orientation::Horizontal, 0);
    container.append(&inner);

    if !watched_vars.is_empty() {
        let last_watched_vals = watched_vars
            .iter()
            .map(|v| {
                let val = ctx
                    .global_vars
                    .get(v)
                    .map(|dv| dv.0.clone())
                    .unwrap_or_default();
                (v.clone(), val)
            })
            .collect();

        BINDING_COLLECTOR.with(|col| {
            if let Some(bindings) = col.borrow_mut().as_mut() {
                bindings.push(AnyBinding::RhaiWidget(RhaiWidgetBinding {
                    watched_vars,
                    static_attrs: HashMap::new(),
                    scope: ctx.scope.clone(),
                    widget_defs: ctx.widget_defs.clone(),
                    script_path,
                    fn_name,
                    container: container.clone(),
                    last_watched_vals,
                }));
            }
        });
    }

    Ok(container.upcast())
}

/// Build a plugin-registered widget (`(my-widget :attr "val" :watch "VARS")`).
///
/// Attrs are evaluated at build time and injected into Rhai scope alongside
/// the current values of all watched vars. On rebuild (watched var change),
/// static attr values are re-merged so the Rhai function always sees both.
pub fn build_rhai_defwidget(
    wu: &BasicWidgetUse,
    def: &meh_rhai_engine::RhaiWidgetDef,
    ctx: &EvalCtx,
) -> Result<gtk4::Widget> {
    let config_dir = CONFIG_DIR
        .get()
        .map(|p| p.as_path())
        .unwrap_or(std::path::Path::new("."));

    let Some(engine) = meh_rhai_engine::global() else {
        anyhow::bail!("rhai-defwidget `{}`: Rhai engine not initialised", wu.name);
    };

    // Resolve :watch — call-site overrides the plugin's default_watch.
    let watch_override = wu
        .attrs
        .attrs
        .get(&AttrName("watch".to_string()))
        .and_then(|e| e.value.as_simplexpr().ok())
        .and_then(|expr| ctx.eval_expr(&expr).ok())
        .map(|v| v.0);

    let watched_vars: Vec<VarName> = match &watch_override {
        Some(s) => s
            .split_whitespace()
            .filter(|s| !s.is_empty())
            .map(|s| VarName(s.to_string()))
            .collect(),
        None => def
            .default_watch
            .iter()
            .map(|s| VarName(s.clone()))
            .collect(),
    };

    // Evaluate all call-site attrs (excluding meta-attr :watch) as static values.
    let mut static_attrs: HashMap<String, String> = HashMap::new();
    for (attr_name, entry) in &wu.attrs.attrs {
        if attr_name.0 == "watch" {
            continue;
        }
        let val = entry
            .value
            .as_simplexpr()
            .ok()
            .and_then(|expr| ctx.eval_expr(&expr).ok())
            .map(|v| v.0)
            .unwrap_or_default();
        static_attrs.insert(attr_name.0.clone(), val);
    }

    // Build initial scope: static attrs + current watched-var values.
    let mut scope_vars = static_attrs.clone();
    for var in &watched_vars {
        let val = ctx
            .global_vars
            .get(var)
            .map(|dv| dv.0.clone())
            .unwrap_or_default();
        scope_vars.insert(var.0.clone(), val);
    }

    let data =
        engine.call_fn_as_widget_data(&def.script_path, config_dir, &def.fn_name, &scope_vars)?;
    let inner_wu = rhai_data_to_widget_use(data, wu.span);
    let inner = build_widget(&inner_wu, ctx)?;

    let container = gtk4::Box::new(gtk4::Orientation::Horizontal, 0);
    container.append(&inner);

    if !watched_vars.is_empty() {
        let last_watched_vals = watched_vars
            .iter()
            .map(|v| {
                let val = ctx
                    .global_vars
                    .get(v)
                    .map(|dv| dv.0.clone())
                    .unwrap_or_default();
                (v.clone(), val)
            })
            .collect();

        BINDING_COLLECTOR.with(|col| {
            if let Some(bindings) = col.borrow_mut().as_mut() {
                bindings.push(AnyBinding::RhaiWidget(RhaiWidgetBinding {
                    watched_vars,
                    static_attrs,
                    scope: ctx.scope.clone(),
                    widget_defs: ctx.widget_defs.clone(),
                    script_path: def.script_path.clone(),
                    fn_name: def.fn_name.clone(),
                    container: container.clone(),
                    last_watched_vals,
                }));
            }
        });
    }

    Ok(container.upcast())
}

// ── Map → WidgetUse conversion ────────────────────────────────────────────────

pub fn rhai_data_to_widget_use(data: meh_rhai_engine::RhaiWidgetData, span: Span) -> WidgetUse {
    let attrs_map = data
        .attrs
        .into_iter()
        .map(|(k, v)| {
            let expr = SimplExpr::Literal(DynVal::from_string(v));
            let entry = AttrEntry::new(span, Ast::SimplExpr(span, expr));
            (AttrName(k), entry)
        })
        .collect();

    let children = data
        .children
        .into_iter()
        .map(|c| rhai_data_to_widget_use(c, span))
        .collect();

    WidgetUse::Basic(BasicWidgetUse {
        name: data.widget_type,
        attrs: Attributes::new(span, attrs_map),
        children,
        span,
        name_span: span,
    })
}
