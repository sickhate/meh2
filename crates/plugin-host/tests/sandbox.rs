//! Plugin sandbox enforcement.

use std::path::Path;

use meh_rhai_engine::ScriptSandbox;

#[test]
fn plugin_read_is_limited_to_plugin_dir_and_allowlist() {
    let dir = std::env::temp_dir().join("meh2-sandbox-test");
    let _ = std::fs::create_dir_all(&dir);
    let allowed = dir.join("allowed.txt");
    std::fs::write(&allowed, "ok").unwrap();

    let sandbox = ScriptSandbox::for_plugin(&dir, false, &[]);
    assert!(sandbox.allows_read(allowed.to_str().unwrap()));
    assert!(!sandbox.allows_read("/etc/passwd"));
}

#[test]
fn unrestricted_allows_any_read() {
    let sandbox = ScriptSandbox::unrestricted();
    assert!(sandbox.allows_read("/etc/passwd"));
}

#[test]
fn shell_disabled_by_default_for_plugins() {
    let dir = Path::new("/tmp");
    let sandbox = ScriptSandbox::for_plugin(dir, false, &[]);
    assert!(!sandbox.allows_shell());
}
