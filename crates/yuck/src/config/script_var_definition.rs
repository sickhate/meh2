use simplexpr::{dynval::DynVal, SimplExpr};

use crate::{
    error::{DiagError, DiagResult, DiagResultExt},
    format_diagnostic::ToDiagnostic,
    gen_diagnostic,
    parser::{ast::Ast, ast_iterator::AstIterator, from_ast::FromAstElementContent},
};
use eww_shared_util::{Span, VarName};

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub enum ScriptVarDefinition {
    Poll(PollScriptVar),
    Listen(ListenScriptVar),
    Subscribe(SubscribeScriptVar),
}

impl ScriptVarDefinition {
    pub fn name_span(&self) -> Span {
        match self {
            ScriptVarDefinition::Poll(x) => x.name_span,
            ScriptVarDefinition::Listen(x) => x.name_span,
            ScriptVarDefinition::Subscribe(x) => x.name_span,
        }
    }

    pub fn name(&self) -> &VarName {
        match self {
            ScriptVarDefinition::Poll(x) => &x.name,
            ScriptVarDefinition::Listen(x) => &x.name,
            ScriptVarDefinition::Subscribe(x) => &x.name,
        }
    }

    pub fn command_span(&self) -> Option<Span> {
        match self {
            ScriptVarDefinition::Poll(x) => match x.command {
                VarSource::Shell(span, ..) => Some(span),
                VarSource::Function(_) => None,
            },
            ScriptVarDefinition::Listen(x) => Some(x.command_span),
            ScriptVarDefinition::Subscribe(_) => None,
        }
    }

    pub fn initial_value(&self) -> DynVal {
        match self {
            ScriptVarDefinition::Poll(x) => x
                .initial_value
                .clone()
                .unwrap_or_else(|| DynVal::from_string(String::new())),
            ScriptVarDefinition::Listen(x) => x.initial_value.clone(),
            ScriptVarDefinition::Subscribe(x) => x.initial_value.clone(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
#[allow(unpredictable_function_pointer_comparisons)]
pub enum VarSource {
    // TODO allow for other executors? (python, etc)
    Shell(Span, String),
    #[serde(skip)]
    Function(fn() -> Result<DynVal, Box<dyn std::error::Error + Sync + Send + 'static>>),
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct PollScriptVar {
    pub name: VarName,
    pub run_while_expr: SimplExpr,
    pub command: VarSource,
    pub initial_value: Option<DynVal>,
    pub interval: std::time::Duration,
    pub name_span: Span,
}

impl FromAstElementContent for PollScriptVar {
    const ELEMENT_NAME: &'static str = "defpoll";

    fn from_tail<I: Iterator<Item = Ast>>(
        _span: Span,
        mut iter: AstIterator<I>,
    ) -> DiagResult<Self> {
        let result: DiagResult<_> = (move || {
            let (name_span, name) = iter.expect_symbol()?;
            let mut attrs = iter.expect_key_values()?;
            let initial_value = Some(
                attrs
                    .primitive_optional("initial")?
                    .unwrap_or_else(|| DynVal::from_string(String::new())),
            );
            let interval = attrs
                .primitive_required::<DynVal, _>("interval")?
                .as_duration()
                .map_err(|e| DiagError(e.to_diagnostic()))?;
            let (script_span, script) = iter.expect_literal()?;

            let run_while_expr = attrs
                .ast_optional::<SimplExpr>("run-while")?
                .unwrap_or_else(|| SimplExpr::Literal(DynVal::from(true)));

            iter.expect_done()?;
            Ok(Self {
                name_span,
                name: VarName(name),
                run_while_expr,
                command: VarSource::Shell(script_span, script.to_string()),
                initial_value,
                interval,
            })
        })();
        result.note(r#"Expected format: `(defpoll name :interval "10s" "echo 'a shell script'")`"#)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct ListenScriptVar {
    pub name: VarName,
    pub command: String,
    pub initial_value: DynVal,
    pub command_span: Span,
    pub name_span: Span,
}
impl FromAstElementContent for ListenScriptVar {
    const ELEMENT_NAME: &'static str = "deflisten";

    fn from_tail<I: Iterator<Item = Ast>>(
        _span: Span,
        mut iter: AstIterator<I>,
    ) -> DiagResult<Self> {
        let result: DiagResult<_> = (move || {
            let (name_span, name) = iter.expect_symbol()?;
            let mut attrs = iter.expect_key_values()?;
            let initial_value = attrs
                .primitive_optional("initial")?
                .unwrap_or_else(|| DynVal::from_string(String::new()));
            let (command_span, script) = iter.expect_literal()?;
            iter.expect_done()?;
            Ok(Self {
                name_span,
                name: VarName(name),
                command: script.to_string(),
                initial_value,
                command_span,
            })
        })();
        result.note(r#"Expected format: `(deflisten name :initial "0" "tail -f /tmp/example")`"#)
    }
}

// ── Subscribe vars ────────────────────────────────────────────────────────────

/// Which DBus bus to connect to.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub enum DbusKind {
    Session,
    System,
}

/// Source for a `defsubscribe` var.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub enum SubscribeSource {
    /// Watch a file with inotify; var = trimmed file contents on each change.
    File { path: String },
    /// Watch a DBus property via `org.freedesktop.DBus.Properties.PropertiesChanged`.
    Dbus {
        bus: DbusKind,
        service: String,
        object: String,
        interface: String,
        property: String,
    },
}

/// A `defsubscribe` var that reacts to inotify or DBus events.
///
/// Syntax:
/// ```yuck
/// (defsubscribe battery :file "/sys/class/power_supply/BAT0/capacity" :initial "0")
///
/// (defsubscribe nm-state
///   :dbus-bus "system"
///   :dbus-service "org.freedesktop.NetworkManager"
///   :dbus-object  "/org/freedesktop/NetworkManager"
///   :dbus-iface   "org.freedesktop.NetworkManager"
///   :dbus-prop    "State"
///   :initial "0")
/// ```
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct SubscribeScriptVar {
    pub name: VarName,
    pub name_span: Span,
    pub initial_value: DynVal,
    pub source: SubscribeSource,
}

impl FromAstElementContent for SubscribeScriptVar {
    const ELEMENT_NAME: &'static str = "defsubscribe";

    fn from_tail<I: Iterator<Item = Ast>>(
        _span: Span,
        mut iter: AstIterator<I>,
    ) -> DiagResult<Self> {
        let result: DiagResult<_> = (move || {
            let (name_span, name) = iter.expect_symbol()?;
            let mut attrs = iter.expect_key_values()?;
            let initial_value = attrs
                .primitive_optional("initial")?
                .unwrap_or_else(|| DynVal::from_string(String::new()));

            let source = if let Some(path) = attrs.primitive_optional::<String, _>("file")? {
                SubscribeSource::File { path }
            } else if let Some(service) = attrs.primitive_optional::<String, _>("dbus-service")? {
                let object = attrs.primitive_required::<String, _>("dbus-object")?;
                let interface = attrs.primitive_required::<String, _>("dbus-iface")?;
                let property = attrs.primitive_required::<String, _>("dbus-prop")?;
                let bus_str = attrs
                    .primitive_optional::<String, _>("dbus-bus")?
                    .unwrap_or_else(|| "session".to_string());
                let bus = match bus_str.as_str() {
                    "system" => DbusKind::System,
                    "session" => DbusKind::Session,
                    other => {
                        return Err(DiagError(gen_diagnostic! {
                            msg = format!("Unknown :dbus-bus `{other}`, expected \"session\" or \"system\""),
                            label = name_span,
                        }))
                    }
                };
                SubscribeSource::Dbus {
                    bus,
                    service,
                    object,
                    interface,
                    property,
                }
            } else {
                return Err(DiagError(gen_diagnostic! {
                    msg = "defsubscribe requires either :file or :dbus-service attribute",
                    label = name_span,
                }));
            };

            iter.expect_done()?;
            Ok(Self {
                name: VarName(name),
                name_span,
                initial_value,
                source,
            })
        })();
        result.note(concat!(
            "Expected format:\n",
            "  `(defsubscribe name :file \"/path/to/file\")`\n",
            "  `(defsubscribe name :dbus-service \"…\" :dbus-object \"…\" ",
            ":dbus-iface \"…\" :dbus-prop \"…\")`",
        ))
    }
}
