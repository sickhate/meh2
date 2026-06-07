// GPL-3.0-or-later
//! Reactive attribute and loop bindings.

use std::collections::HashMap;
use std::sync::Arc;

use eww_shared_util::{AttrName, VarName};
use gtk4::prelude::*;
use meh_core::EvalCtx;
use simplexpr::{SimplExpr, dynval::DynVal};
use std::cell::RefCell;
use yuck::config::{
    widget_definition::WidgetDefinition,
    widget_use::LoopWidgetUse,
};

use crate::builder::populate_loop_container;

// ── Reactive binding system ───────────────────────────────────────────────────

/// Evaluate a binding expression using only the vars it references.
fn eval_binding_expr(
    expr: &SimplExpr,
    var_refs: &[VarName],
    is_constant: bool,
    scope: &HashMap<VarName, DynVal>,
    global_vars: &HashMap<VarName, DynVal>,
) -> String {
    if is_constant {
        return expr.eval(&HashMap::new()).map(|v| v.0).unwrap_or_default();
    }
    let mut vars = HashMap::with_capacity(var_refs.len());
    for var in var_refs {
        if vars.contains_key(var) {
            continue;
        }
        if let Some(val) = scope.get(var).or_else(|| global_vars.get(var)) {
            vars.insert(var.clone(), val.clone());
        }
    }
    expr.eval(&vars).map(|v| v.0).unwrap_or_default()
}

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
        let new_val = eval_binding_expr(
            &self.expr,
            &self.var_refs,
            self.is_constant,
            &self.scope,
            global_vars,
        );
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
    widget_defs: Arc<HashMap<String, WidgetDefinition>>,
    container: gtk4::Box,
    last_val: String,
    /// Bindings registered inside loop body items; replaced on each rebuild.
    child_bindings: Vec<AnyBinding>,
}

impl LoopBinding {
    fn rebuild_children(&mut self, global_vars: &HashMap<VarName, DynVal>) {
        self.child_bindings.clear();
        while let Some(child) = self.container.first_child() {
            self.container.remove(&child);
        }
        let ctx = EvalCtx {
            scope: self.scope.clone(),
            global_vars,
            widget_defs: self.widget_defs.clone(),
        };
        let (_, collected) =
            collect_bindings(|| populate_loop_container(&self.container, &self.lp, &ctx));
        self.child_bindings = collected;
    }

    pub fn update(&mut self, global_vars: &HashMap<VarName, DynVal>) -> bool {
        let new_val = eval_binding_expr(
            &self.expr,
            &self.var_refs,
            self.is_constant,
            &self.scope,
            global_vars,
        );
        if new_val == self.last_val {
            return false;
        }
        self.last_val = new_val;
        self.rebuild_children(global_vars);
        true
    }

    pub fn intersects(&self, changed: &std::collections::HashSet<VarName>) -> bool {
        if !self.is_constant && self.var_refs.iter().any(|v| changed.contains(v)) {
            return true;
        }
        self.child_bindings
            .iter()
            .any(|b| b.intersects(changed))
    }

    pub fn intersects_own(&self, changed: &std::collections::HashSet<VarName>) -> bool {
        !self.is_constant && self.var_refs.iter().any(|v| changed.contains(v))
    }

    pub fn update_children(&mut self, changed: &std::collections::HashSet<VarName>, global_vars: &HashMap<VarName, DynVal>) {
        for child in &mut self.child_bindings {
            child.update_matching(changed, global_vars);
        }
    }
}

/// Either an attribute binding, a loop binding, or a Rhai widget binding.
pub enum AnyBinding {
    Attr(Binding),
    Loop(LoopBinding),
    #[cfg(feature = "rhai")]
    RhaiWidget(crate::rhai_widget::RhaiWidgetBinding),
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

    /// Update this binding and any nested child bindings when `changed` intersects them.
    pub fn update_matching(
        &mut self,
        changed: &std::collections::HashSet<VarName>,
        global_vars: &HashMap<VarName, DynVal>,
    ) {
        match self {
            AnyBinding::Attr(b) => {
                if b.intersects(changed) {
                    b.update(global_vars);
                }
            }
            AnyBinding::Loop(b) => {
                b.update_children(changed, global_vars);
                if b.intersects_own(changed) {
                    b.update(global_vars);
                }
            }
            #[cfg(feature = "rhai")]
            AnyBinding::RhaiWidget(b) => {
                b.update_children(changed, global_vars);
                if b.intersects(changed) {
                    b.update(global_vars);
                }
            }
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
    pub(crate) static BINDING_COLLECTOR: RefCell<Option<Vec<AnyBinding>>> = const { RefCell::new(None) };
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
pub(crate) fn maybe_bind<F>(
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
pub(crate) fn register_loop_binding(
    lp: &LoopWidgetUse,
    ctx: &EvalCtx,
    container: gtk4::Box,
    initial: String,
    child_bindings: Vec<AnyBinding>,
) {
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
                child_bindings,
            }));
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    use simplexpr::parser;

    fn expr(src: &str) -> SimplExpr {
        parser::parse_string(0, 0, src).expect("parse expr")
    }

    #[test]
    fn intersects_only_referenced_vars() {
        let binding = Binding {
            expr: expr("greeting"),
            var_refs: vec![VarName("greeting".into())],
            is_constant: false,
            scope: HashMap::new(),
            setter: Box::new(|_| {}),
            last_val: String::new(),
        };
        let changed: HashSet<VarName> = [VarName("other".into())].into_iter().collect();
        assert!(!binding.intersects(&changed));
        let changed: HashSet<VarName> = [VarName("greeting".into())].into_iter().collect();
        assert!(binding.intersects(&changed));
    }

    #[test]
    fn eval_binding_expr_uses_scope_over_global() {
        let expr = expr("name");
        let mut scope = HashMap::new();
        scope.insert(VarName("name".into()), DynVal::from_string("local".into()));
        let mut global = HashMap::new();
        global.insert(VarName("name".into()), DynVal::from_string("global".into()));
        let out = eval_binding_expr(
            &expr,
            &[VarName("name".into())],
            false,
            &scope,
            &global,
        );
        assert_eq!(out, "local");
    }
}
