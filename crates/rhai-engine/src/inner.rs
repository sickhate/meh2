// GPL-3.0-or-later

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use anyhow::Result;
use once_cell::sync::OnceCell;
use rhai::{Dynamic, Engine, Scope, AST};

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
pub struct RhaiEngine {
    engine: Engine,
    cache:  Mutex<HashMap<PathBuf, AST>>,
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
        engine.set_max_operations(500_000);  // ~500ms on modern hardware
        engine.set_max_call_levels(32);
        engine.set_max_string_size(1024 * 1024);
        engine.set_max_array_size(10_000);
        engine.set_max_map_size(1_000);

        // ── API surface ───────────────────────────────────────────────────────

        // read_file(path) → string — reads and trims a file. Returns "" if not found.
        engine.register_fn("read_file", |path: &str| -> String {
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

        Arc::new(Self {
            engine,
            cache: Mutex::new(HashMap::new()),
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

        let result: Dynamic = self.engine
            .eval_ast_with_scope(&mut scope, &ast)
            .map_err(|e| anyhow::anyhow!("rhai `{}`: {}", abs.display(), e))?;

        Ok(dynamic_to_string(result))
    }

    /// Execute an inline Rhai snippet and return the final value as a string.
    pub fn eval_inline(&self, script: &str) -> Result<String> {
        let mut scope = Scope::new();
        let result: Dynamic = self.engine
            .eval_with_scope(&mut scope, script)
            .map_err(|e| anyhow::anyhow!("rhai inline: {}", e))?;
        Ok(dynamic_to_string(result))
    }

    /// Call a named function in a Rhai script file and return the result as a string.
    ///
    /// The compiled AST is cached. Per-call cost is a `Scope` allocation + `call_fn`.
    pub fn call_fn(&self, path: &Path, config_dir: &Path, fn_name: &str) -> Result<String> {
        let abs = if path.is_absolute() {
            path.to_path_buf()
        } else {
            config_dir.join(path)
        };

        let ast = self.get_or_compile(&abs)?;
        let mut scope = Scope::new();

        let result: Dynamic = self.engine
            .call_fn::<Dynamic>(&mut scope, &ast, fn_name, ())
            .map_err(|e| anyhow::anyhow!("rhai `{}::{}`: {}", abs.display(), fn_name, e))?;

        Ok(dynamic_to_string(result))
    }

    /// Remove a file's compiled AST from the cache (call on hot-reload).
    pub fn invalidate(&self, path: &Path) {
        if let Ok(mut c) = self.cache.lock() {
            c.remove(path);
        }
    }

    // ── Internals ─────────────────────────────────────────────────────────────

    fn get_or_compile(&self, path: &PathBuf) -> Result<AST> {
        {
            let cache = self.cache.lock()
                .map_err(|_| anyhow::anyhow!("rhai cache mutex poisoned"))?;
            if let Some(ast) = cache.get(path) {
                return Ok(ast.clone());
            }
        }

        let ast = self.engine
            .compile_file(path.clone())
            .map_err(|e| anyhow::anyhow!("rhai compile `{}`: {}", path.display(), e))?;

        let mut cache = self.cache.lock()
            .map_err(|_| anyhow::anyhow!("rhai cache mutex poisoned"))?;
        cache.insert(path.clone(), ast.clone());
        Ok(ast)
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn dynamic_to_string(v: Dynamic) -> String {
    match v.type_name() {
        "string"  => v.cast::<String>(),
        "i64"     => v.cast::<i64>().to_string(),
        "f64"     => {
            let f = v.cast::<f64>();
            // Trim unnecessary decimal places (e.g. "42.0" → "42")
            if f.fract() == 0.0 && f.abs() < 1e15 {
                format!("{}", f as i64)
            } else {
                format!("{:.2}", f)
            }
        }
        "bool"    => v.cast::<bool>().to_string(),
        "()"      => String::new(),
        _         => v.to_string(),
    }
}
