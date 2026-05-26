# meh2 Plugin System

Plugins let you add new data sources (vars) to your bar without writing Rust or
recompiling meh2. A plugin is a directory containing two files:

- `plugin.toml` — declares the plugin's name, version, and the vars it provides.
- `main.rhai` — Rhai script that implements the vars.

## Installing a plugin

Drop the plugin directory into `~/.config/meh2/plugins/`:

```
~/.config/meh2/plugins/
└── sysinfo/
    ├── plugin.toml
    └── main.rhai
```

Restart the meh2 daemon to pick up the new plugin:

```bash
meh2 kill && meh2 daemon &
```

Then reference the plugin's vars in your yuck config like any other var:

```yuck
(label :text {"CPU: " + PLUGIN_CPU})
```

## Plugin manifest (`plugin.toml`)

```toml
name        = "sysinfo"
version     = "0.1.0"
author      = "you"
description = "CPU and RAM from /proc"

[[vars]]
name     = "PLUGIN_CPU"    # var name used in yuck
type     = "poll"          # only "poll" is supported in Phase 3
interval = 2               # poll every 2 seconds
initial  = "0%"            # value before the first tick

[[vars]]
name     = "PLUGIN_RAM"
type     = "poll"
interval = 5
initial  = "0 MB"

[permissions]
read_files  = ["/proc/stat", "/proc/meminfo"]  # informational; not yet enforced
allow_shell = false
```

### Fields

| Field | Required | Description |
|---|---|---|
| `name` | yes | Plugin name (display only) |
| `version` | yes | Semver string |
| `author` | no | Author display name |
| `description` | no | One-line description |
| `vars[].name` | yes | Var name as used in yuck. Convention: `PLUGIN_<NAME>` |
| `vars[].type` | yes | `"poll"` (only option in Phase 3) |
| `vars[].interval` | no | Poll interval in seconds. Default: 60 |
| `vars[].initial` | no | Initial value before first tick. Default: `""` |
| `permissions.read_files` | no | File paths the script reads (informational) |
| `permissions.allow_shell` | no | Whether the script calls `run_shell()` (informational) |

## Writing the Rhai script (`main.rhai`)

For each var declared in `plugin.toml`, export a function named `get_<VARNAME>`:

```rhai
fn get_PLUGIN_CPU() {
    // return a string
    "42%"
}

fn get_PLUGIN_RAM() {
    let raw = read_file("/proc/meminfo");
    // ... parse and return
    "1234 MB"
}
```

### Available API

The full meh2 Rhai API is available in plugins:

| Function | Returns | Description |
|---|---|---|
| `read_file(path)` | string | Read and trim a file. `""` if not found. |
| `run_shell(cmd)` | string | Run `sh -c cmd`, return stdout. |
| `parse_int(s)` | i64 | Parse string to integer, 0 on failure. |
| `parse_float(s)` | f64 | Parse string to float, 0.0 on failure. |
| `env_var(name)` | string | Read env var, `""` if unset. |
| `path_exists(path)` | bool | True if path exists on disk. |

### Rhai gotchas

- `string.trim()` and `string.replace(from, to)` modify **in-place** and return
  `()`, not the new value. Never do `let x = s.trim()`.
- `read_file()` already trims trailing whitespace.
- String concatenation: use `+`. Template strings `` `${var}` `` work in `.rhai`
  files but **not** inside yuck strings.
- The per-call timeout is 500 ms. Scripts that take longer are interrupted.

### Persisting state between ticks

Rhai functions have no persistent state between calls. To track a delta (e.g.
CPU usage), write to a cache file and read it back on the next tick:

```rhai
fn get_PLUGIN_CPU() {
    let cache = env_var("HOME") + "/.cache/meh2/my_plugin_state";
    let prev  = read_file(cache);
    // ... compute new value from prev ...
    run_shell("printf '" + new_state + "' > " + cache);
    result
}
```

## Hot reload

`meh2 reload` invalidates the AST cache for all plugin scripts. The next poll
tick recompiles `main.rhai` from disk, so you can edit a plugin's logic and
run `meh2 reload` to pick up the change without restarting the daemon.

Adding or removing plugins (new directories, deleted directories) requires a
full daemon restart — `meh2 reload` only recompiles existing plugins' scripts.

## Plugin discovery paths

meh2 searches two locations (in order):

1. `~/.config/meh2/plugins/` — user plugins (takes priority)
2. `~/.local/share/meh2/plugins/` — system-wide plugins

Both are searched; all valid plugins from both locations are loaded.

## Example plugin

See `examples/plugin-demo/plugins/sysinfo/` for a complete working example
that provides `PLUGIN_CPU` and `PLUGIN_RAM` from `/proc/stat` and `/proc/meminfo`.

Run it with:

```bash
meh2 --config examples/plugin-demo daemon &
meh2 --config examples/plugin-demo open bar
```
