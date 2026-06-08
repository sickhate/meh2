// GPL-3.0-or-later

use std::{
    collections::{HashMap, VecDeque},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use anyhow::Result;
use once_cell::sync::OnceCell;
use rhai::{AST, Dynamic, Engine, Scope};

use crate::sandbox::{self, ScriptSandbox};

static GLOBAL: OnceCell<Arc<RhaiEngine>> = OnceCell::new();

/// Return the global engine, or `None` if `init()` has not been called yet.
pub fn global() -> Option<Arc<RhaiEngine>> {
    GLOBAL.get().cloned()
}

/// Create and register the global engine. Idempotent: if already initialised,
/// returns the existing instance.
pub fn init() -> Arc<RhaiEngine> {
    if let Some(existing) = GLOBAL.get() {
        return existing.clone();
    }
    let engine = RhaiEngine::new();
    let _ = GLOBAL.set(engine.clone());
    engine
}

// ── Engine ────────────────────────────────────────────────────────────────────

/// Sandboxed Rhai engine with AST cache.
///
/// `rhai/sync` feature makes `Engine: Send + Sync`, so this type is safe to
/// share across threads via `Arc`.
/// Maximum compiled scripts kept in the AST cache (FIFO eviction).
const MAX_AST_CACHE: usize = 64;

type AstCache = (HashMap<PathBuf, Arc<AST>>, VecDeque<PathBuf>);

pub struct RhaiEngine {
    engine: Engine,
    cache: Mutex<AstCache>,
}

// Safety: `Engine` is Send+Sync when compiled with the `sync` feature (which
// we require in the workspace dep). The `Mutex<HashMap>` is also Send+Sync.
unsafe impl Send for RhaiEngine {}
unsafe impl Sync for RhaiEngine {}

impl RhaiEngine {
    fn new() -> Arc<Self> {
        let mut engine = Engine::new();

        // ── Sandbox ───────────────────────────────────────────────────────────

        // Disable module loading — scripts cannot `import` arbitrary files.
        engine.set_module_resolver(rhai::module_resolvers::DummyModuleResolver::new());

        // Resource limits to prevent runaway scripts.
        engine.set_max_operations(500_000); // ~500ms on modern hardware
        engine.set_max_call_levels(32);
        engine.set_max_string_size(1024 * 1024);
        engine.set_max_array_size(10_000);
        engine.set_max_map_size(1_000);

        // ── API surface ───────────────────────────────────────────────────────

        // read_file(path) → string — reads and trims a file. Returns "" if not found.
        engine.register_fn("read_file", |path: &str| -> String {
            if !sandbox::read_allowed(path) {
                tracing::warn!("rhai read_file({path}): denied by plugin sandbox");
                return String::new();
            }
            match std::fs::read_to_string(path) {
                Ok(s) => s.trim_end().to_string(),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
                Err(e) => {
                    tracing::warn!("rhai read_file({path}): {e}");
                    String::new()
                }
            }
        });

        // run_shell(cmd) → string — stdout of `sh -c cmd`.
        // Explicit opt-in; logged so users can see when scripts shell out.
        engine.register_fn("run_shell", |cmd: &str| -> String {
            if !sandbox::shell_allowed() {
                tracing::warn!("rhai run_shell({cmd}): denied by plugin sandbox");
                return String::new();
            }
            tracing::debug!("rhai run_shell: {cmd}");
            std::process::Command::new("sh")
                .arg("-c")
                .arg(cmd)
                .output()
                .map(|o| String::from_utf8_lossy(&o.stdout).trim_end().to_string())
                .unwrap_or_default()
        });

        // parse_int / parse_float helpers
        engine.register_fn("parse_int", |s: &str| -> i64 {
            s.trim().parse::<i64>().unwrap_or(0)
        });
        engine.register_fn("parse_float", |s: &str| -> f64 {
            s.trim().parse::<f64>().unwrap_or(0.0)
        });

        // env_var(name) → string — reads an environment variable, "" if unset.
        engine.register_fn("env_var", |name: &str| -> String {
            std::env::var(name).unwrap_or_default()
        });

        // path_exists(path) → bool — true if the path exists on disk.
        engine.register_fn("path_exists", |path: &str| -> bool {
            std::path::Path::new(path).exists()
        });

        // read_or(path, default) → string — like read_file but returns `default`
        // when the file is missing or empty. Useful for reading state flag files.
        engine.register_fn("read_or", |path: &str, default: &str| -> String {
            if !sandbox::read_allowed(path) {
                tracing::warn!("rhai read_or({path}): denied by plugin sandbox");
                return default.to_string();
            }
            let content = std::fs::read_to_string(path)
                .map(|s| s.trim_end().to_string())
                .unwrap_or_default();
            if content.is_empty() {
                default.to_string()
            } else {
                content
            }
        });

        // json_decode(json_str) → Dynamic — parse a JSON string into a Rhai value.
        // Objects become maps (#{...}), arrays become arrays, primitives their
        // Rhai equivalents. Returns an empty map on parse error (error is logged).
        // json_decode supports both objects AND arrays by going through serde_json.
        engine.register_fn("json_decode", |json: &str| -> rhai::Dynamic {
            match serde_json::from_str::<serde_json::Value>(json) {
                Ok(val) => serde_json_to_rhai(&val),
                Err(e) => {
                    tracing::warn!("rhai json_decode: {e}");
                    rhai::Dynamic::from_map(rhai::Map::new())
                }
            }
        });

        // json_encode(value) → string — serialise a Rhai value to a JSON string.
        // Useful for caching complex objects via write_cache().
        // Returns "{}" on error.
        engine.register_fn("json_encode", |val: rhai::Dynamic| -> String {
            let json_val = rhai_dynamic_to_json(&val);
            match serde_json::to_string(&json_val) {
                Ok(s) => s,
                Err(e) => {
                    tracing::warn!("rhai json_encode: {e}");
                    "{}".to_string()
                }
            }
        });

        // read_cache(key) → string — reads from ~/.cache/meh2/<key>. Symmetric with write_cache.
        // Returns "" if the key doesn't exist or HOME is unset.
        engine.register_fn("read_cache", |key: &str| -> String {
            if !key
                .chars()
                .all(|c| c.is_alphanumeric() || c == '_' || c == '-')
            {
                tracing::warn!("rhai read_cache: invalid key {:?}", key);
                return String::new();
            }
            let path = match std::env::var("HOME") {
                Ok(h) => std::path::PathBuf::from(h).join(".cache/meh2").join(key),
                Err(_) => return String::new(),
            };
            match std::fs::read_to_string(&path) {
                Ok(s) => s.trim_end().to_string(),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
                Err(e) => {
                    tracing::warn!("rhai read_cache({key}): {e}");
                    String::new()
                }
            }
        });

        // write_cache(key, value) → bool — writes value to ~/.cache/meh2/<key>.
        // Key is restricted to [a-zA-Z0-9_-] to prevent path traversal.
        // Returns true on success, false on error (error is logged).
        engine.register_fn("write_cache", |key: &str, value: &str| -> bool {
            if !key
                .chars()
                .all(|c| c.is_alphanumeric() || c == '_' || c == '-')
            {
                tracing::warn!(
                    "rhai write_cache: invalid key {:?}, must match [a-zA-Z0-9_-]",
                    key
                );
                return false;
            }
            let dir = match std::env::var("HOME") {
                Ok(h) => std::path::PathBuf::from(h).join(".cache/meh2"),
                Err(_) => {
                    tracing::warn!("rhai write_cache: HOME not set");
                    return false;
                }
            };
            if let Err(e) = std::fs::create_dir_all(&dir) {
                tracing::warn!("rhai write_cache: mkdir {}: {}", dir.display(), e);
                return false;
            }
            match std::fs::write(dir.join(key), value) {
                Ok(()) => true,
                Err(e) => {
                    tracing::warn!("rhai write_cache({key}): {e}");
                    false
                }
            }
        });

        Arc::new(Self {
            engine,
            cache: Mutex::new((HashMap::new(), VecDeque::new())),
        })
    }

    // ── Public API ─────────────────────────────────────────────────────────────

    /// Execute a Rhai script file and return the final value as a string.
    ///
    /// `path` is resolved relative to `config_dir` if not absolute.
    /// The compiled AST is cached; repeated calls cost only `eval_ast_with_scope`.
    pub fn eval_file(&self, path: &Path, config_dir: &Path) -> Result<String> {
        let abs = if path.is_absolute() {
            path.to_path_buf()
        } else {
            config_dir.join(path)
        };

        let ast = self.get_or_compile(&abs)?;
        let mut scope = Scope::new();

        let result: Dynamic = self
            .engine
            .eval_ast_with_scope(&mut scope, &ast)
            .map_err(|e| anyhow::anyhow!("rhai `{}`: {}", abs.display(), e))?;

        Ok(dynamic_to_string(result))
    }

    /// Execute an inline Rhai snippet and return the final value as a string.
    pub fn eval_inline(&self, script: &str) -> Result<String> {
        let mut scope = Scope::new();
        let result: Dynamic = self
            .engine
            .eval_with_scope(&mut scope, script)
            .map_err(|e| anyhow::anyhow!("rhai inline: {}", e))?;
        Ok(dynamic_to_string(result))
    }

    /// Call a named function in a Rhai script file and return the result as a string.
    ///
    /// The compiled AST is cached. Per-call cost is a `Scope` allocation + `call_fn`.
    pub fn call_fn(&self, path: &Path, config_dir: &Path, fn_name: &str) -> Result<String> {
        self.call_fn_sandboxed(path, config_dir, fn_name, None)
    }

    /// Like [`call_fn`] but applies a plugin sandbox for the duration of the call.
    pub fn call_fn_sandboxed(
        &self,
        path: &Path,
        config_dir: &Path,
        fn_name: &str,
        sandbox: Option<ScriptSandbox>,
    ) -> Result<String> {
        sandbox::with_sandbox(sandbox, || self.call_fn_inner(path, config_dir, fn_name))
    }

    fn call_fn_inner(&self, path: &Path, config_dir: &Path, fn_name: &str) -> Result<String> {
        let abs = if path.is_absolute() {
            path.to_path_buf()
        } else {
            config_dir.join(path)
        };

        let ast = self.get_or_compile(&abs)?;
        let mut scope = Scope::new();

        let result: Dynamic = self
            .engine
            .call_fn::<Dynamic>(&mut scope, &ast, fn_name, ())
            .map_err(|e| anyhow::anyhow!("rhai `{}::{}`: {}", abs.display(), fn_name, e))?;

        Ok(dynamic_to_string(result))
    }

    /// Call a named function and interpret the result as a widget tree map.
    ///
    /// The Rhai function must return a `Map` (#{...}) with at least a `"type"` key.
    /// Children are expressed as an `"children"` array of nested maps.
    ///
    /// `vars` is injected into the Rhai scope before the call so scripts can
    /// reference watched var values directly (e.g. `CPU`, `MEM`) without reading
    /// cache files.  Variable names must be valid Rhai identifiers.
    pub fn call_fn_as_widget_data(
        &self,
        path: &Path,
        config_dir: &Path,
        fn_name: &str,
        vars: &std::collections::HashMap<String, String>,
    ) -> Result<crate::RhaiWidgetData> {
        self.call_fn_as_widget_data_sandboxed(path, config_dir, fn_name, vars, None)
    }

    pub fn call_fn_as_widget_data_sandboxed(
        &self,
        path: &Path,
        config_dir: &Path,
        fn_name: &str,
        vars: &std::collections::HashMap<String, String>,
        sandbox: Option<ScriptSandbox>,
    ) -> Result<crate::RhaiWidgetData> {
        sandbox::with_sandbox(sandbox, || {
            self.call_fn_as_widget_data_inner(path, config_dir, fn_name, vars)
        })
    }

    fn call_fn_as_widget_data_inner(
        &self,
        path: &Path,
        config_dir: &Path,
        fn_name: &str,
        vars: &std::collections::HashMap<String, String>,
    ) -> Result<crate::RhaiWidgetData> {
        let abs = if path.is_absolute() {
            path.to_path_buf()
        } else {
            config_dir.join(path)
        };

        let ast = self.get_or_compile(&abs)?;
        let mut scope = Scope::new();

        for (k, v) in vars {
            scope.push(k.clone(), rhai::Dynamic::from(v.clone()));
        }

        let result: Dynamic = self
            .engine
            .call_fn::<Dynamic>(&mut scope, &ast, fn_name, ())
            .map_err(|e| anyhow::anyhow!("rhai `{}::{}`: {}", abs.display(), fn_name, e))?;

        dynamic_to_widget_data(result)
            .map_err(|e| anyhow::anyhow!("rhai widget `{}::{}`: {}", abs.display(), fn_name, e))
    }

    /// Remove a file's compiled AST from the cache (call on hot-reload).
    pub fn invalidate(&self, path: &Path) {
        if let Ok(mut guard) = self.cache.lock() {
            let (cache, order) = &mut *guard;
            cache.remove(path);
            order.retain(|p| p != path);
        }
    }

    /// Clear the entire AST cache so all scripts are recompiled on the next call.
    /// Call this on `meh2 reload` to pick up changes to any Rhai script.
    pub fn invalidate_all(&self) {
        if let Ok(mut guard) = self.cache.lock() {
            let (cache, order) = &mut *guard;
            let n = cache.len();
            cache.clear();
            order.clear();
            tracing::debug!("rhai-engine: cleared {} cached AST(s)", n);
        }
    }

    // ── Internals ─────────────────────────────────────────────────────────────

    fn get_or_compile(&self, path: &PathBuf) -> Result<Arc<AST>> {
        {
            let guard = self
                .cache
                .lock()
                .map_err(|_| anyhow::anyhow!("rhai cache mutex poisoned"))?;
            let (cache, _order) = &*guard;
            if let Some(ast) = cache.get(path) {
                return Ok(Arc::clone(ast));
            }
        }

        let ast = Arc::new(
            self.engine
                .compile_file(path.clone())
                .map_err(|e| anyhow::anyhow!("rhai compile `{}`: {}", path.display(), e))?,
        );

        let mut guard = self
            .cache
            .lock()
            .map_err(|_| anyhow::anyhow!("rhai cache mutex poisoned"))?;
        let (cache, order) = &mut *guard;
        if !cache.contains_key(path) {
            while cache.len() >= MAX_AST_CACHE {
                if let Some(old) = order.pop_front() {
                    cache.remove(&old);
                } else {
                    break;
                }
            }
            order.push_back(path.clone());
        }
        cache.insert(path.clone(), Arc::clone(&ast));
        Ok(ast)
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn dynamic_to_widget_data(d: Dynamic) -> std::result::Result<crate::RhaiWidgetData, String> {
    if d.type_name() != "map" {
        return Err(format!(
            "rhai-widget function must return a map (#{{...}}), got {}",
            d.type_name()
        ));
    }
    let map = d.cast::<rhai::Map>();

    let widget_type = map
        .get("type")
        .ok_or_else(|| "rhai-widget map missing required \"type\" key".to_string())?
        .clone();
    let widget_type = if widget_type.type_name() == "string" {
        widget_type.cast::<String>()
    } else {
        return Err("rhai-widget \"type\" must be a string".to_string());
    };

    let children = if let Some(kids) = map.get("children") {
        if kids.type_name() == "array" {
            kids.clone()
                .cast::<rhai::Array>()
                .into_iter()
                .map(dynamic_to_widget_data)
                .collect::<std::result::Result<Vec<_>, _>>()?
        } else {
            vec![]
        }
    } else {
        vec![]
    };

    let mut attrs = Vec::new();
    for (key, val) in &map {
        let k = key.as_str();
        if k == "type" || k == "children" {
            continue;
        }
        let v = match val.type_name() {
            "string" => val.clone().cast::<String>(),
            "i64" => val.clone().cast::<i64>().to_string(),
            "f64" => {
                let f = val.clone().cast::<f64>();
                if f.fract() == 0.0 {
                    format!("{}", f as i64)
                } else {
                    format!("{f:.2}")
                }
            }
            "bool" => val.clone().cast::<bool>().to_string(),
            _ => val.to_string(),
        };
        attrs.push((k.to_string(), v));
    }

    Ok(crate::RhaiWidgetData {
        widget_type,
        attrs,
        children,
    })
}

fn dynamic_to_string(v: Dynamic) -> String {
    match v.type_name() {
        "string" => v.cast::<String>(),
        "i64" => v.cast::<i64>().to_string(),
        "f64" => {
            let f = v.cast::<f64>();
            // Trim unnecessary decimal places (e.g. "42.0" → "42")
            if f.fract() == 0.0 && f.abs() < 1e15 {
                format!("{}", f as i64)
            } else {
                format!("{:.2}", f)
            }
        }
        "bool" => v.cast::<bool>().to_string(),
        "()" => String::new(),
        _ => v.to_string(),
    }
}

/// Recursively convert a `serde_json::Value` to a Rhai `Dynamic` for `json_decode`.
fn serde_json_to_rhai(val: &serde_json::Value) -> rhai::Dynamic {
    use serde_json::Value;
    match val {
        Value::Object(map) => {
            let rhai_map: rhai::Map = map
                .iter()
                .map(|(k, v)| (k.as_str().into(), serde_json_to_rhai(v)))
                .collect();
            rhai::Dynamic::from_map(rhai_map)
        }
        Value::Array(arr) => {
            let rhai_arr: rhai::Array = arr.iter().map(serde_json_to_rhai).collect();
            rhai::Dynamic::from_array(rhai_arr)
        }
        Value::String(s) => rhai::Dynamic::from(s.clone()),
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                rhai::Dynamic::from(i)
            } else if let Some(f) = n.as_f64() {
                rhai::Dynamic::from(f)
            } else {
                rhai::Dynamic::from(n.to_string())
            }
        }
        Value::Bool(b) => rhai::Dynamic::from(*b),
        Value::Null => rhai::Dynamic::UNIT,
    }
}

/// Recursively convert a Rhai `Dynamic` to a `serde_json::Value` for `json_encode`.
fn rhai_dynamic_to_json(d: &Dynamic) -> serde_json::Value {
    if d.is::<rhai::Map>() {
        let map = d.clone().cast::<rhai::Map>();
        let obj: serde_json::Map<String, serde_json::Value> = map
            .into_iter()
            .map(|(k, v)| (k.to_string(), rhai_dynamic_to_json(&v)))
            .collect();
        serde_json::Value::Object(obj)
    } else if d.is::<rhai::Array>() {
        let arr = d.clone().cast::<rhai::Array>();
        serde_json::Value::Array(arr.iter().map(rhai_dynamic_to_json).collect())
    } else if d.is::<String>() {
        serde_json::Value::String(d.clone().cast::<String>())
    } else if d.is::<i64>() {
        serde_json::Value::Number(d.clone().cast::<i64>().into())
    } else if d.is::<f64>() {
        let f = d.clone().cast::<f64>();
        serde_json::Number::from_f64(f)
            .map(serde_json::Value::Number)
            .unwrap_or(serde_json::Value::Null)
    } else if d.is::<bool>() {
        serde_json::Value::Bool(d.clone().cast::<bool>())
    } else {
        serde_json::Value::Null
    }
}
