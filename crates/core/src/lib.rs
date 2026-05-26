// GPL-3.0-or-later
//! Paths, variable state, IPC types, and yuck config loading for meh.

use std::{
    collections::HashMap,
    hash::{DefaultHasher, Hash, Hasher},
    path::{Path, PathBuf},
};

use anyhow::{bail, Context, Result};
use eww_shared_util::{AttrName, Span, VarName};
use serde::{Deserialize, Serialize};
use simplexpr::{dynval::DynVal, SimplExpr};
use yuck::{
    config::{
        attributes::Attributes,
        file_provider::{FilesError, YuckFileProvider},
        toplevel::Config,
        widget_definition::WidgetDefinition,
    },
    error::DiagError,
    parser::ast::Ast,
};

// ── Paths ─────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct MehPaths {
    pub config_dir: PathBuf,
    pub socket_file: PathBuf,
    pub pid_file:   PathBuf,
    pub log_dir:    PathBuf,
    pub cache_dir:  PathBuf,
}

impl MehPaths {
    pub fn from_config_dir(config_dir: impl AsRef<Path>) -> Result<Self> {
        let config_dir = config_dir.as_ref();
        if config_dir.is_file() {
            bail!("Config path must be a directory, not a file");
        }
        if !config_dir.exists() {
            bail!("Config directory {} does not exist", config_dir.display());
        }
        let config_dir = config_dir.canonicalize()?;

        let mut h = DefaultHasher::new();
        config_dir.display().to_string().hash(&mut h);
        let id = format!("{:x}", h.finish());

        let runtime_dir = std::env::var("XDG_RUNTIME_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from("/tmp"));
        let socket_file = runtime_dir.join(format!("meh2-server_{}", id));
        let pid_file    = runtime_dir.join(format!("meh2-daemon_{}.pid", id));

        let cache_base = std::env::var("XDG_CACHE_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|_| {
                PathBuf::from(std::env::var("HOME").unwrap_or_default()).join(".cache")
            });
        let log_dir   = cache_base.join("meh2");
        let cache_dir = cache_base.join("meh2");

        if !log_dir.exists() {
            std::fs::create_dir_all(&log_dir)?;
        }

        Ok(Self { config_dir, socket_file, pid_file, log_dir, cache_dir })
    }

    pub fn default_paths() -> Result<Self> {
        let cfg_base = std::env::var("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|_| {
                PathBuf::from(std::env::var("HOME").unwrap_or_default()).join(".config")
            });
        let meh_dir = cfg_base.join("meh2");
        let eww_dir = cfg_base.join("eww");
        let dir = if meh_dir.exists() { meh_dir } else { eww_dir };
        Self::from_config_dir(dir)
    }

    pub fn main_yuck_file(&self) -> PathBuf {
        for name in &["meh2.yuck", "meh.yuck", "eww.yuck"] {
            let p = self.config_dir.join(name);
            if p.exists() { return p; }
        }
        self.config_dir.join("meh.yuck")
    }

    pub fn scss_file(&self) -> Option<PathBuf> {
        for name in &["meh.scss", "eww.scss", "style.scss"] {
            let p = self.config_dir.join(name);
            if p.exists() { return Some(p); }
        }
        None
    }
}

// ── Variable state ────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Default)]
pub struct VarState {
    pub vars: HashMap<VarName, DynVal>,
}

impl VarState {
    pub fn new() -> Self { Self::default() }
    pub fn get(&self, name: &VarName) -> Option<&DynVal> { self.vars.get(name) }
    pub fn set(&mut self, name: VarName, value: DynVal) { self.vars.insert(name, value); }
}

// ── EvalCtx ───────────────────────────────────────────────────────────────────

pub struct EvalCtx<'a> {
    pub scope:       HashMap<VarName, DynVal>,
    pub global_vars: &'a HashMap<VarName, DynVal>,
    pub widget_defs: &'a HashMap<String, WidgetDefinition>,
}

impl<'a> EvalCtx<'a> {
    pub fn new(
        global_vars: &'a HashMap<VarName, DynVal>,
        widget_defs: &'a HashMap<String, WidgetDefinition>,
    ) -> Self {
        Self { scope: HashMap::new(), global_vars, widget_defs }
    }

    pub fn all_vars(&self) -> HashMap<VarName, DynVal> {
        let mut vars = self.global_vars.clone();
        vars.extend(self.scope.clone());
        vars
    }

    pub fn eval_expr(&self, expr: &SimplExpr) -> Result<DynVal> {
        expr.eval(&self.all_vars()).map_err(|e| anyhow::anyhow!("{}", e))
    }

    pub fn eval_attr(&self, attrs: &Attributes, key: &str) -> Option<DynVal> {
        attrs.attrs.get(&AttrName(key.to_string())).and_then(|entry| {
            entry.value.as_simplexpr().ok().and_then(|expr| self.eval_expr(&expr).ok())
        })
    }

    pub fn eval_attr_str(&self, attrs: &Attributes, key: &str) -> Option<String> {
        self.eval_attr(attrs, key).map(|v| v.0)
    }

    pub fn eval_attr_bool(&self, attrs: &Attributes, key: &str) -> Option<bool> {
        self.eval_attr(attrs, key).and_then(|v| v.as_bool().ok())
    }

    pub fn eval_attr_f64(&self, attrs: &Attributes, key: &str) -> Option<f64> {
        self.eval_attr(attrs, key).and_then(|v| v.as_f64().ok())
    }

    pub fn eval_attr_i64(&self, attrs: &Attributes, key: &str) -> Option<i64> {
        self.eval_attr(attrs, key).and_then(|v| v.as_i64().ok())
    }

    pub fn child_scope(&self, extra: HashMap<VarName, DynVal>) -> EvalCtx<'a> {
        let mut scope = self.scope.clone();
        scope.extend(extra);
        EvalCtx { scope, global_vars: self.global_vars, widget_defs: self.widget_defs }
    }
}

// ── Yuck config loading ───────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct MehConfig {
    pub yuck:      Config,
    pub var_state: VarState,
}

impl Default for MehConfig {
    fn default() -> Self {
        Self {
            yuck: Config {
                widget_definitions: Default::default(),
                window_definitions: Default::default(),
                var_definitions:    Default::default(),
                script_vars:        Default::default(),
            },
            var_state: VarState::new(),
        }
    }
}

impl MehConfig {
    pub fn load(paths: &MehPaths) -> Result<Self> {
        let main_file = paths.main_yuck_file();
        if !main_file.exists() {
            bail!("Config file not found: {}", main_file.display());
        }

        let mut db = FileDb::new(paths.config_dir.clone());
        let yuck = Config::generate_from_main_file(&mut db, &main_file)
            .map_err(|e| anyhow::anyhow!("yuck parse error:\n{:#?}", e))?;

        let mut var_state = VarState::new();
        for (name, def) in &yuck.var_definitions {
            var_state.set(name.clone(), def.initial_value.clone());
        }
        // Pre-populate script var (deflisten/defpoll/defsubscribe) initial values so
        // any popup opened before the async var-forwarder runs its first batch
        // can evaluate expressions containing these vars without "Unknown variable" errors.
        for (name, def) in &yuck.script_vars {
            var_state.set(name.clone(), def.initial_value());
        }

        Ok(Self { yuck, var_state })
    }
}

/// Minimal file provider that resolves includes relative to config_dir.
struct FileDb {
    config_dir: PathBuf,
    next_id:    usize,
    files:      HashMap<usize, (String, String)>,
}

impl FileDb {
    fn new(config_dir: PathBuf) -> Self {
        Self { config_dir, next_id: 0, files: HashMap::new() }
    }

    fn alloc_id(&mut self) -> usize {
        let id = self.next_id;
        self.next_id += 1;
        id
    }
}

impl YuckFileProvider for FileDb {
    fn load_yuck_file(&mut self, path: PathBuf) -> Result<(Span, Vec<Ast>), FilesError> {
        let abs = if path.is_absolute() { path } else { self.config_dir.join(path) };
        let src = std::fs::read_to_string(&abs)?;
        let file_id = self.alloc_id();
        self.files.insert(file_id, (abs.display().to_string(), src.clone()));
        yuck::parser::parse_toplevel(file_id, src).map_err(FilesError::DiagError)
    }

    fn load_yuck_str(&mut self, name: String, content: String) -> Result<(Span, Vec<Ast>), DiagError> {
        let file_id = self.alloc_id();
        self.files.insert(file_id, (name, content.clone()));
        yuck::parser::parse_toplevel(file_id, content)
    }

    fn unload(&mut self, id: usize) {
        self.files.remove(&id);
    }
}

// ── CSS loading ───────────────────────────────────────────────────────────────

pub fn compile_css(paths: &MehPaths) -> Option<String> {
    let scss = paths.scss_file()?;
    let src = std::fs::read_to_string(&scss).ok()?;
    let opts = grass::Options::default().load_path(&paths.config_dir);
    match grass::from_string(src, &opts) {
        Ok(css) => Some(css),
        Err(e) => {
            tracing::warn!("SCSS compile error: {}", e);
            None
        }
    }
}

// ── IPC types ─────────────────────────────────────────────────────────────────

#[derive(Debug, Serialize, Deserialize)]
pub enum IpcCmd {
    Ping,
    Open    { window: String, toggle: bool, monitor: Option<i32> },
    Close   { windows: Vec<String> },
    CloseAll,
    Reload,
    Update  { vars: HashMap<String, String> },
    State,
    Get     { var: String },
    ListWindows,
    Kill,
}

#[derive(Debug, Serialize, Deserialize)]
pub enum IpcResponse {
    Ok(String),
    Err(String),
}

impl IpcResponse {
    pub fn ok(s: impl Into<String>) -> Self  { Self::Ok(s.into()) }
    pub fn err(s: impl Into<String>) -> Self { Self::Err(s.into()) }
    pub fn ok_empty() -> Self { Self::Ok(String::new()) }
}

pub async fn ipc_write<T: Serialize>(
    stream: &mut (impl tokio::io::AsyncWriteExt + Unpin),
    msg: &T,
) -> Result<()> {
    let bytes = bincode::serialize(msg)?;
    let len   = (bytes.len() as u32).to_be_bytes();
    stream.write_all(&len).await?;
    stream.write_all(&bytes).await?;
    Ok(())
}

pub async fn ipc_read<T: for<'de> Deserialize<'de>>(
    stream: &mut (impl tokio::io::AsyncReadExt + Unpin),
) -> Result<T> {
    let mut len_buf = [0u8; 4];
    stream.read_exact(&mut len_buf).await?;
    let len = u32::from_be_bytes(len_buf) as usize;
    let mut buf = vec![0u8; len];
    stream.read_exact(&mut buf).await?;
    Ok(bincode::deserialize(&buf)?)
}

pub async fn send_ipc_cmd(socket: &Path, cmd: &IpcCmd) -> Result<IpcResponse> {
    use tokio::net::UnixStream;
    let stream = UnixStream::connect(socket)
        .await
        .with_context(|| format!("Cannot connect to meh socket {}", socket.display()))?;
    let (mut reader, mut writer) = tokio::io::split(stream);
    ipc_write(&mut writer, cmd).await?;
    let resp = ipc_read::<IpcResponse>(&mut reader).await?;
    Ok(resp)
}
