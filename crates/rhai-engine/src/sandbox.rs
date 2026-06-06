// GPL-3.0-or-later
//! Per-script sandbox policy for plugin Rhai code.

use std::{
    cell::RefCell,
    path::{Path, PathBuf},
};

thread_local! {
    static ACTIVE: RefCell<Option<ScriptSandbox>> = const { RefCell::new(None) };
}

/// File/shell policy applied while a plugin script runs.
#[derive(Debug, Clone, Default)]
pub struct ScriptSandbox {
    /// When `false`, `run_shell()` is a no-op for this script.
    pub allow_shell: bool,
    /// Paths that `read_file()` / `read_or()` may access. Empty means unrestricted reads.
    pub read_prefixes: Vec<PathBuf>,
}

impl ScriptSandbox {
    /// Config scripts (`defpoll`, event handlers, user `rhai-widget`) — no extra restrictions.
    pub fn unrestricted() -> Self {
        Self {
            allow_shell: true,
            read_prefixes: Vec::new(),
        }
    }

    pub fn for_plugin(plugin_dir: &Path, allow_shell: bool, read_files: &[String]) -> Self {
        let mut read_prefixes = vec![plugin_dir.to_path_buf()];
        for entry in read_files {
            read_prefixes.push(if Path::new(entry).is_absolute() {
                PathBuf::from(entry)
            } else {
                plugin_dir.join(entry)
            });
        }
        Self {
            allow_shell,
            read_prefixes,
        }
    }

    pub fn allows_read(&self, path: &str) -> bool {
        if self.read_prefixes.is_empty() {
            return true;
        }
        let p = Path::new(path);
        self.read_prefixes.iter().any(|prefix| path_starts_with(p, prefix))
    }

    pub fn allows_shell(&self) -> bool {
        self.allow_shell
    }
}

fn path_starts_with(path: &Path, prefix: &Path) -> bool {
    if path.starts_with(prefix) {
        return true;
    }
    match (path.canonicalize(), prefix.canonicalize()) {
        (Ok(p), Ok(pref)) => p.starts_with(pref),
        _ => false,
    }
}

pub(crate) fn with_sandbox<R>(sandbox: Option<ScriptSandbox>, f: impl FnOnce() -> R) -> R {
    ACTIVE.with(|cell| *cell.borrow_mut() = sandbox);
    let result = f();
    ACTIVE.with(|cell| *cell.borrow_mut() = None);
    result
}

pub(crate) fn read_allowed(path: &str) -> bool {
    ACTIVE.with(|cell| {
        cell.borrow()
            .as_ref()
            .map(|sb| sb.allows_read(path))
            .unwrap_or(true)
    })
}

pub(crate) fn shell_allowed() -> bool {
    ACTIVE.with(|cell| cell.borrow().as_ref().map(|sb| sb.allows_shell()).unwrap_or(true))
}
