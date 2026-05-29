# meh2

> *meh with a Rhai engine, a plugin system, and Python-free polling.*

**meh2** is a GTK4 Wayland widget system and status bar — a fork of [meh](https://github.com/sickhate/meh) that adds in-process Rhai scripting, a Rhai plugin system, and inotify-backed file subscriptions. Your existing yuck/meh configs work without modification; Rhai is purely additive.

---

## Performance

| Metric | meh | meh2 |
|---|---|---|
| Idle CPU (static bar) | ~0.17% | ~0.17% |
| Idle CPU (1s clock) | ~0.35% | ~0.35% |
| Poll latency (shell) | ~1.3–1.8 ms | ~1.3–1.8 ms |
| Poll latency (Rhai) | N/A | < 1 ms (no fork) |
| Python in poll path | Required | **None* |
| Persistent subprocess RSS | ~5–6 MB each | None when using Rhai |
| Rhai engine overhead | N/A | ~2–4 MB RSS |
| Poll gating | Windows-closed pause | Same |

---

## What meh2 adds over meh

| Feature | meh | meh2 |
|---|---|---|
| Poll sources | Shell scripts only | `.rhai` files + `rhai:` inline + shell |
| Event handlers | Shell commands | `.rhai` files + `rhai:` inline + shell |
| Plugin system | None | Rhai plugins in `~/.config/meh2/plugins/` |
| Widget builders | `defwidget` in yuck only | `(rhai-widget)` + plugin-registered widgets |
| File watching | `defsubscribe :inotify` | `defsubscribe :file` with tilde expansion + atomic-write support |
| Python in poll path | Required for complex scripts | **Zero** — json_decode, string ops, run_shell cover every use case |
| Poll latency (Rhai) | N/A | < 1 ms (no fork, AST cached) |
| Poll latency (shell) | ~1.3–1.8 ms | Same — shell path unchanged |

---

## Rhai API

Scripts have access to a sandboxed API (no filesystem/network unless explicitly called):

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
