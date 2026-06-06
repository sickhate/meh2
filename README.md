# meh2

> *meh with a Rhai engine, a plugin system, and Python-free polling.*

**meh2** is a GTK4 Wayland widget system and status bar — a fork of [meh](https://github.com/sickhate/meh) that adds in-process Rhai scripting, a Rhai plugin system, and inotify-backed file subscriptions. Your existing yuck/meh configs work without modification; Rhai is purely additive.

---

## Features

- **Yuck configuration** — same S-expression language as eww/meh. Existing configs are compatible.
- **GTK4 + Wayland-native** — `gtk4-layer-shell`, fractional scaling, hardware-accelerated rendering. No X11.
- **Rhai scripting engine** — `.rhai` files and `rhai:` inline expressions as `defpoll`/`deflisten` sources. Runs in-process, no fork; AST compiled once and cached (< 1 ms per tick).
- **Rhai plugin system** — drop a directory into `~/.config/meh2/plugins/`; plugins contribute vars and custom `defwidget`s declared in `plugin.toml`.
- **Rhai widgets** — `(rhai-widget)` builds live widget trees from a Rhai function, rebuilt only when its watched vars change.
- **Python-free polling** — `json_decode`, string ops, and `run_shell` cover every use case; no Python interpreter in the poll path.
- **Reactive bindings** — variable updates push only to affected widgets. No full tree rebuilds.
- **Granular hot reload** — `meh2 reload` rebuilds only windows whose definition changed; unchanged windows keep their state.
- **Poll gating** — `defpoll` and `deflisten` subprocesses pause when no windows are open. Idle daemon sits at ~0.17% CPU.
- **`defsubscribe`** — inotify file watchers (with tilde expansion + atomic-write support) and DBus property watchers. Zero-cost reactive sources — no polling, no subprocess.
- **Deflisten process groups** — `deflisten` subprocesses run while windows are open (gated like defpoll when the bar is hidden); SIGTERM on shutdown reaches grandchildren (inotifywait, playerctl --follow, nmcli monitor, etc.).
- **Declarative animations** — `AdwTimedAnimation` on `progress` values and opacity. Interruptible, respects system reduced-motion.
- **Native app launcher** — `(launcher)` widget: instant `gio::AppInfo` search, PATH executable autocomplete, keyboard nav, click-to-launch.
- **System tray** — StatusNotifierItem/StatusNotifierHost. Opt-in via `systray` Cargo feature.
- **`(shader)` widget** — GLSL fragment shaders via `GtkGLArea`. Full profile only.
- **Heap trimming** — popups are destroyed with `gtk_window_destroy()` and `malloc_trim(0)` returns freed image memory to the OS, keeping resident memory flat over long uptimes.
- **Plugin sandbox** — `plugin.toml` permissions enforce file read allowlists and opt-in `run_shell()`.
- **Versioned IPC** — CLI/daemon protocol v1; mismatched versions fail with a clear error.
- **Three build profiles**: `minimal`, `default` (systray + dbus + animations + rhai + plugins), `full` (+ shader).

---

## Performance

| Metric | meh | meh2 |
|---|---|---|
| Idle CPU (static bar) | ~0.17% | ~0.17% |
| Idle CPU (1s clock) | ~0.35% | ~0.35% |
| Poll latency (shell) | ~1.3–1.8 ms | ~1.3–1.8 ms |
| Poll latency (Rhai) | N/A | < 1 ms (no fork) |
| Python in poll path | Required | **None** |
| Persistent subprocess RSS | ~5–6 MB each | None when using Rhai |
| Rhai engine overhead | N/A | ~2–4 MB RSS |
| Resident memory (bar, cairo renderer) | — | ~28 MB, flat |
| Poll gating | Windows-closed pause | `defpoll` + `deflisten` pause |
| Listen subprocess RSS | ~5–6 MB each (always on) | Gated when bar hidden |
| Plugin permissions | N/A | Enforced sandbox in `plugin.toml` |

> **Memory.** Popups are torn down with `gtk_window_destroy()` (not `close()`,
> which only hides them and leaks the widget tree), and a `malloc_trim(0)` after
> each popup closes returns the freed image memory to the OS — so resident memory
> stays flat no matter how many menus you open. Run the bar with the cairo GSK
> renderer (`GSK_RENDERER=cairo`) for the lowest footprint (~28 MB); the default
> Vulkan renderer holds ~50 MB of GPU buffers for otherwise-static bar content.

---

## What meh2 adds over meh

| Feature | meh | meh2 |
|---|---|---|
| Poll sources | Shell scripts only | `.rhai` files + `rhai:` inline + shell |
| Event handlers | Shell commands | `.rhai` files + `rhai:` inline + shell |
| Plugin system | None | Rhai plugins in `~/.config/meh2/plugins/` (sandboxed) |
| Widget builders | `defwidget` in yuck only | `(rhai-widget)` + plugin-registered widgets |
| File watching | `defsubscribe :inotify` | `defsubscribe :file` with tilde expansion + atomic-write support |
| Python in poll path | Required for complex scripts | **Zero** — json_decode, string ops, run_shell cover every use case |
| Poll latency (Rhai) | N/A | < 1 ms (no fork, AST cached) |
| Poll latency (shell) | ~1.3–1.8 ms | Same — shell path unchanged |

---

## Rhai API

Scripts have access to a sandboxed API. **Config scripts** (`defpoll`, event
handlers, user `(rhai-widget)`) are unrestricted. **Plugin scripts** are limited
by `[permissions]` in `plugin.toml` (see [docs/plugins.md](docs/plugins.md)).

```
read_file(path)          → string    silent "" on NotFound
read_or(path, default)   → string    returns default if missing/empty
write_cache(key, value)  → bool      writes to ~/.cache/meh2/<key>
read_cache(key)          → string    reads from ~/.cache/meh2/<key>
run_shell(cmd)           → string    stdout of sh -c cmd (logged)
parse_int(s)             → i64
parse_float(s)           → f64
env_var(name)            → string
path_exists(path)        → bool
json_decode(json_str)    → Dynamic   JSON object/array/value → Rhai
json_encode(value)       → string    any Rhai value → JSON string
```

### Gotchas

- `string.trim()` and `string.replace()` are **in-place** — they return `()`, not the new value.
- Template strings `` `${var}` `` work in `.rhai` files but not inside yuck strings (use `+` instead).
- No built-in regex — use `split()`, `contains()`, `index_of()`, `sub_string()`.

---

## Installation

### Arch Linux

```bash
git clone https://github.com/sickhate/meh2
cd meh2
makepkg -si
```

### From source

```bash
git clone https://github.com/sickhate/meh2
cd meh2
cargo build --release
sudo install -Dm755 target/release/meh2 /usr/bin/meh2
```

**Runtime dependencies:** `gtk4`, `gtk4-layer-shell`, `libadwaita`, `cairo`, `glib2`, `pango`

**Build dependencies:** `rust` (stable), `cargo`

### Build profiles

```bash
# Minimal — no Rhai, no tray, no animations
cargo build --release --no-default-features --features minimal

# Default — everything most users want (includes Rhai + plugins)
cargo build --release

# Full — default + GLSL shaders
cargo build --release --features full
```

---

## Quick start

```bash
meh2 daemon
meh2 open bar
meh2 reload   # hot reload — only changed windows restart
meh2 close bar
meh2 update MY_VAR=hello
```

Config lives in `~/.config/meh2/`. Main file: `meh2.yuck` (or `eww.yuck`). CSS: `style.scss`.

---

## Example — Rhai poll source

```yuck
(defpoll CPU :interval "2s" "scripts/getCpu.rhai")

(defwidget cpu-bar []
  (label :text "${CPU.pct}% ${CPU.temp}°C"))
```

```rhai
// scripts/getCpu.rhai
let stat = read_file("/proc/stat");
let nums = [];
for p in stat.split("\n")[0].split(" ") {
    if p != "" && p != "cpu" { nums += p; }
}
let idle  = parse_int(nums[3]);
let total = parse_int(nums[0]) + parse_int(nums[1]) + parse_int(nums[2]) + idle + parse_int(nums[4]);

let prev = read_cache("cpu_prev").split(",");
let pct  = 0;
if prev.len() >= 2 {
    let dt = total - parse_int(prev[0]);
    let di = idle  - parse_int(prev[1]);
    if dt > 0 { pct = ((dt - di) * 100) / dt; }
}
write_cache("cpu_prev", total + "," + idle);

let temp = parse_int(read_file("/sys/class/thermal/thermal_zone0/temp")) / 1000;
`{"pct":${pct},"temp":${temp}}`
```

---

## Example — defsubscribe :file

```yuck
; Instant inotify response — no polling, zero overhead when file doesn't change
(defsubscribe THEME :file "~/.local/share/meh2/theme" :initial "dark")
```

The watcher handles missing files (watches parent dir), atomic writes (rm+recreate), and tilde paths.

---

## Example — Rhai plugin

```
~/.config/meh2/plugins/sysinfo/
├── plugin.toml
└── main.rhai
```

```toml
# plugin.toml
name    = "sysinfo"
version = "0.1.0"

[permissions]
read_files  = ["/proc/stat", "/proc/meminfo"]
allow_shell = false

[[vars]]
name     = "PLUGIN_CPU"
interval = "3s"

[[widgets]]
name          = "sysinfo-pill"
fn_name       = "render_sysinfo_pill"
default_watch = ["PLUGIN_CPU"]
```

```rhai
// main.rhai
fn get_PLUGIN_CPU() {
    let pct = parse_int(read_file("/sys/class/thermal/thermal_zone0/temp")) / 1000;
    `{"pct":${pct}}`
}

fn render_sysinfo_pill() #{
    type: "label",
    text: "CPU " + PLUGIN_CPU.pct + "%"
}
```

Use in yuck: `(sysinfo-pill)` — no `:src` or `:fn` needed.

---

## Project layout

```
crates/
├── cli/           meh2 binary
├── daemon/        lifecycle + IPC server
├── core/          config, paths, IPC protocol (v1)
├── gtk4-impl/     GTK widgets — bindings.rs, builder.rs, widgets.rs, runtime.rs
├── script-vars/   defpoll / deflisten / defsubscribe
├── rhai-engine/   Rhai runtime + plugin sandbox
├── plugin-host/   plugin discovery + poll tasks
├── notifier-host/ system tray (systray feature)
├── yuck/          config language (MIT/eww)
└── simplexpr/     expression evaluator (MIT/eww)
docs/plugins.md    plugin authoring + permissions
```

Run tests: `cargo test --release --locked`

Arch package build artifacts (`pkg/`, `*.pkg.tar.zst`) are gitignored.

---

## Phase status

| Phase | Description | Status |
|---|---|---|
| 0 | Fork baseline — meh2 binary, config dir, socket prefix | Complete |
| 1 | Rhai poll/listen sources | Complete |
| 2 | Rhai event handlers | Complete |
| 3 | Rhai plugin system | Complete |
| 4 | `(rhai-widget)` — Rhai-defined widget trees | Complete |
| 4.5 | Plugin-registered defwidgets | Complete |
| 5 | Hybrid yuck+Rhai (inline Rhai in attr expressions) | Planned |

---

## Acknowledgements

- **[meh](https://github.com/sickhate/meh)** — the direct parent project this forks from
- **[elkowar/eww](https://github.com/elkowar/eww)** — the original yuck language and concept. MIT licensed.
- **[Ewwii-sh/ewwii](https://github.com/Ewwii-sh/ewwii)** — completed the GTK4 port. GPL-3.0.

---

## License

GPL-3.0-or-later. See [LICENSE](LICENSE).

Code copied or adapted from elkowar/eww retains its MIT header.
