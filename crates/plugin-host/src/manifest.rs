// GPL-3.0-or-later

#[derive(serde::Deserialize, Debug, Clone)]
pub struct PluginManifest {
    pub name: String,
    pub version: String,
    pub author: Option<String>,
    pub description: Option<String>,
    #[serde(default)]
    pub vars: Vec<VarDecl>,
    #[serde(default)]
    pub permissions: Permissions,
}

#[derive(serde::Deserialize, Debug, Clone)]
pub struct VarDecl {
    pub name: String,
    #[serde(rename = "type")]
    pub kind: VarKind,
    /// Poll interval in seconds. Defaults to 60.
    pub interval: Option<u64>,
    /// Initial value emitted before the first poll tick. Defaults to "".
    pub initial: Option<String>,
}

#[derive(serde::Deserialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum VarKind {
    Poll,
}

#[derive(serde::Deserialize, Debug, Clone, Default)]
pub struct Permissions {
    /// File paths the plugin may read via `read_file()`. Not yet enforced —
    /// reserved for future path-allowlist sandboxing.
    #[serde(default)]
    pub read_files: Vec<String>,
    /// Whether the plugin may call `run_shell()`. Not yet enforced.
    #[serde(default)]
    pub allow_shell: bool,
    /// Whether the plugin may call `write_cache()`. Informational only —
    /// write_cache() is always sandboxed to ~/.cache/meh2/.
    #[serde(default)]
    pub write_cache: bool,
}
