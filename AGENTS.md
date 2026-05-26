# meh2

> **meh2** = meh + Rhai + plugin system. A fork of meh (the GTK4 eww-successor)
> that adds an in-process Rhai scripting engine, a Rhai-based plugin system, and
> eventually full Rhai widget configuration alongside yuck.
>
> Binary: `meh2`. Config dir: `~/.config/meh2/`. Parent project: `~/Projects/meh`.

-----

## Current state (last updated 2026-05-26)

**Phase 0 (fork baseline) complete.**
**Phase 1 complete.** Rhai engine wired into `defpoll`/`deflisten`. `.rhai` files and `rhai:` inline sources work.
**Phase 2 complete.** Rhai event handlers: `:onclick`/`:onscroll`/`:onhover` etc. accept `.rhai` files and `rhai:` inline.
**Phase 3 complete.** Rhai plugin system. Drop a directory into `~/.config/meh2/plugins/`, daemon picks it up at start.
**Phase 4 complete.** `(rhai-widget :src "f.rhai" :fn "fn" :watch "VARS")` — Rhai functions return map-based widget trees, rendered live. See ADR-M007.
**Real config migrated.** `~/.config/meh2/` is a full migration of the user's meh bar with Rhai replacements for high-frequency polls.
**meh2 is the active daily bar.** Running as default via `~/.local/share/bar_choice = meh2`. Selectable via bar-switch scripts.

### Rhai API surface (crates/rhai-engine/src/inner.rs)
- `read_file(path)` → string (silent empty on NotFound, warn on other errors)
- `run_shell(cmd)` → string (stdout, logged)
- `parse_int(s)` → i64
- `parse_float(s)` → f64
- `env_var(name)` → string
- `path_exists(path)` → bool

### Known Rhai gotchas (IMPORTANT — read before writing scripts)
- `string.trim()` and `string.replace(from, to)` are **in-place** in Rhai — they modify the string and return `()`, NOT the trimmed/replaced value. Never do `let x = str.trim()`.
- `read_file()` already trims trailing whitespace — no `.trim()` needed on its result.
- To parse `/proc` files: use `split(" ")` + `parse_int(tok)` to find the first positive integer. Do NOT use `replace()`.
- String concatenation: use `+` operator. Template strings `` `${var}` `` work in `.rhai` files but NOT inside yuck strings (yuck parser intercepts `${}`).
- Inline `rhai:` in yuck: use `+` for path building (`h + "/.local/share/..."`) not template strings.

Feature flag: `rhai` (in `default` and `full` profiles; excluded from `minimal`).
New crate: `crates/rhai-engine/`. Example: `examples/rhai-bar/`.

-----

## Read this first

You are Claude Code working in the **meh2** repository. This file is the single
source of truth for what this project is, what it is building toward, and the
rules that govern every change. Read it top-to-bottom at the start of every
session. If anything you are about to do contradicts this file, stop and ask.

The parent project `~/Projects/meh` is **read-only reference** — never modify it.
meh2 builds on meh's foundation and diverges intentionally. Cherry-pick bugfixes
from meh into meh2 via `git cherry-pick`; never merge wholesale.

-----

## Table of contents

1. [What meh2 is](#what-meh2-is)
2. [The prime directive](#the-prime-directive)
3. [Hard scope](#hard-scope)
4. [Architecture](#architecture)
5. [Architecture decisions (ADRs)](#architecture-decisions-adrs)
6. [Build profiles and Cargo features](#build-profiles-and-cargo-features)
7. [Roadmap](#roadmap)
8. [Coding conventions](#coding-conventions)
9. [Performance principles](#performance-principles)
10. [Rules for Claude Code](#rules-for-claude-code)

-----

## What meh2 is

meh2 extends meh with three layered additions, implemented in order:

1. **Rhai script engine** — `defpoll` and `deflisten` sources can be `.rhai`
   files (or inline Rhai blocks) instead of shell commands. The engine runs
   in-process: no fork, no subprocess, no interpreter startup per poll tick.
   Rhai API exposes only what the config needs: file reads, `/proc` helpers,
   `meh2.update()`. Everything else is sandboxed out.

2. **Rhai event handlers** — `:onclick`, `:onscroll`, `:onchange` can reference
   `.rhai` files or inline Rhai expressions. Handlers run on a dedicated thread
   with a timeout guard so a runaway script cannot block GTK.

3. **Rhai plugin system** — plugins are `.rhai` script files discovered from
   `~/.config/meh2/plugins/`. Each plugin can register new `defpoll`/`deflisten`
   sources and contribute `defwidget`-compatible data. No native code, no `.so`,
   no ABI surface.

4. **Full Rhai widget config** (long-term) — widget trees can be defined in
   Rhai alongside yuck. Hybrid: yuck handles layout structure, Rhai handles
   imperative logic, computed values, and dynamic widget composition.

**meh2 keeps everything meh has.** All existing yuck configs work without
modification. The additions are strictly additive: nothing forces users to
use Rhai. A user who never writes a `.rhai` file gets pure meh behaviour
with zero overhead.

-----

## The prime directive

**meh2 must be lighter and faster than equivalent meh configurations that
use shell subprocesses for data, while remaining as light as meh when Rhai
is not used.**

Concretely:

- A config that uses only yuck + CSS and no Rhai: **identical overhead to meh**.
  The Rhai engine must not spin up at all if no `.rhai` source is referenced.
- A config that replaces bash/python poll scripts with Rhai: **lower peak RSS
  and lower poll latency** than the equivalent meh config (no fork, no python).
- Idle CPU target: same as meh — **< 0.1%** on a static bar, **< 0.4%** with
  a 1s clock poll.
- Plugin scripts: each loaded plugin adds at most **~50 KB** (AST cache).
  Ten plugins should cost less than one bash subprocess.

Every PR must answer:
- Does this add overhead when Rhai is not used? (If yes, it needs a feature gate.)
- Does this add overhead when Rhai is used but the specific feature isn't? (If yes, lazy-init it.)
- What is the per-tick cost of this change? Measure it.

-----

## Hard scope

**In scope:**
- Rhai as a scripting layer for poll/listen sources and event handlers.
- Rhai-only plugin system (`.rhai` files, no native code).
- Hybrid yuck+Rhai widget configuration.
- Everything meh already does.

**Out of scope:**
- X11. Wayland-only.
- GTK3. GTK4-only.
- Native (`.so`) plugin loading — ABI hazard, not needed when Rhai covers the use case.
- Lua, WASM, Python, JavaScript, or any other scripting language.
- Features that conflict with the prime directive and cannot be made zero-cost when unused.

If a task suggests X11 / GTK3 / `.so` plugins / other scripting languages, **stop and ask**.

-----

## Architecture

```
meh2/
├── crates/
│   ├── yuck/            # S-expression parser (from elkowar/eww, MIT). No GTK.
│   ├── core/            # Widget tree IR, reactive var graph, IPC types.
│   ├── gtk4-impl/       # GTK4 widget implementations, reactive bindings.
│   ├── layer-shell/     # gtk4-layer-shell window placement.
│   ├── script-vars/     # defpoll / deflisten / defsubscribe. Shell + Rhai sources.
│   ├── rhai-engine/     # (Phase 1) Rhai engine wrapper: sandbox, API surface, timeout.
│   ├── plugin-host/     # (Phase 3) Plugin discovery, manifest, lifecycle.
│   ├── notifier-host/   # StatusNotifierHost. Opt-in systray. From elkowar/eww (MIT).
│   ├── daemon/          # tokio runtime, IPC server, hot reload supervisor.
│   └── cli/             # meh2 binary — daemon + client subcommands.
├── examples/
│   ├── minimal-bar/     # Minimal yuck-only config (same as meh).
│   ├── rhai-bar/        # (Phase 1) Minimal bar using Rhai poll sources.
│   └── plugin-demo/     # (Phase 3) Bar with a sample plugin loaded.
├── benches/
└── CLAUDE.md            # This file.
```

**Crate rules:**
- `yuck/` and `core/` must not depend on `gtk4` or `rhai`.
- `rhai-engine/` must not depend on `gtk4`. It is pure logic + sandbox.
- `script-vars/` depends on `rhai-engine` (behind `rhai` feature). Shell path unchanged when feature is off.
- `gtk4-impl/` depends on `script-vars` for event handlers.
- `plugin-host/` depends on `rhai-engine`, not on `gtk4` directly.

-----

## Architecture decisions (ADRs)

Append-only. Add a new ADR when you make a decision not already covered.
Never edit accepted ADRs in place; supersede with a new one.

### ADR-M001 — Inherit all meh ADRs

**Status:** Accepted · **Date:** 2026-05-26

**Decision.** All ADRs from the meh project (ADR-0001 through ADR-0010) apply
to meh2 unchanged. meh2 extends meh; it does not contradict it. Where meh2
diverges (e.g. meh ADR-0004 said "no plugin system"), the meh2 ADR below
supersedes the meh ADR for this project only.

### ADR-M002 — Rhai, not Lua/WASM/JS for scripting

**Status:** Accepted · **Date:** 2026-05-26

**Context.** Several scripting options exist: Lua (mlua), WASM (wasmtime),
JavaScript (boa/deno), Python (pyo3), Rhai. We need one that: (a) embeds
cleanly in Rust with no C FFI hazard, (b) is sandboxed by default, (c) has
a small binary footprint, (d) is fast enough for sub-second poll scripts.

**Decision.** Rhai (`crates.io/crates/rhai`). It is purpose-built for embedding
in Rust, sandboxed by default (no filesystem/network unless explicitly granted),
adds ~1–2 MB to the binary, and handles our use case (data fetch + format) in
< 1 ms per call.

**Consequences.** Positive: clean Rust API, no unsafe FFI, small footprint,
fast enough for UI data scripts. Negative: Rhai is not Lua (smaller community,
less documentation); not suitable for heavy compute (interpreted, ~10–50×
slower than native Rust for tight loops — but our scripts are I/O-bound).

**Alternatives.** Lua (mlua) — excellent but requires C FFI. JS (boa) —
larger footprint, immature embedding API. WASM (wasmtime) — best sandbox but
500 KB+ per module, overkill for a bar script. Python (pyo3) — huge dep.

### ADR-M003 — Rhai sandbox: explicit allowlist, deny-by-default

**Status:** Accepted · **Date:** 2026-05-26

**Context.** Plugin scripts from the community run in the same process as the
GTK daemon. A malicious or buggy script must not be able to: read arbitrary
files, open network connections, spawn subprocesses, or call `std::process::exit`.

**Decision.** The `rhai-engine` crate creates a sandboxed `rhai::Engine` with:
- File I/O disabled by default. Opt-in: `meh2.read_file(path)` checks a
  path allowlist defined in the user's `meh2.yuck`.
- No subprocess spawning (`std::process` module not registered).
- No network access (no HTTP module registered).
- `meh2.update(var, value)` — the only write-back channel to the daemon.
- Per-call timeout: 500 ms hard limit via `Engine::set_max_operations`.
  Scripts exceeding this are interrupted and the error logged; the last
  good value is kept.
- Stack depth limit: `Engine::set_max_call_levels(64)`.

Plugin scripts that need file access declare it in their manifest and the user
approves. The path allowlist is stored in the daemon at startup.

**Consequences.** Positive: community plugins cannot exfiltrate data or crash
the daemon. Negative: scripts that genuinely need shell access must still use
`deflisten`/`defpoll` with a bash source; Rhai cannot replace all shell use
cases (e.g. piped commands, dbus-monitor output). That is acceptable.

### ADR-M004 — Plugin system: Rhai-only, no native `.so`

**Status:** Accepted · **Date:** 2026-05-26

**Context.** meh ADR-0004 rejected plugins entirely. meh2 revisits this but
constrains the design. A Rust dylib plugin API would require a stable ABI,
linker symbol resolution at runtime, and exposes GTK FFI through `dlopen` —
all serious hazards.

**Decision.** Plugins are `.rhai` files only. No native code. The `plugin-host`
crate discovers plugins from `~/.config/meh2/plugins/` and `~/.local/share/meh2/plugins/`.
Each plugin is a directory containing:
- `plugin.toml` — manifest: name, version, author, description, file-access
  allowlist, declared vars (name + type + poll interval or listen mode).
- `main.rhai` — entry point. Exports named functions matching declared vars.

The daemon loads plugins at startup, registers their declared vars into the
var graph, and calls the relevant Rhai functions on each tick / event.

**Consequences.** Positive: zero ABI surface, sandboxed by ADR-M003, small
per-plugin overhead (~50 KB AST), crashes are caught exceptions not segfaults.
Negative: plugins cannot do anything that requires native Rust (custom GTK
widgets, OpenGL, DBus services). For those, fork meh2 and add a crate.

### ADR-M005 — Rhai engine: one persistent instance per daemon

**Status:** Accepted · **Date:** 2026-05-26

**Context.** Creating a new `rhai::Engine` per poll call would cost ~200 KB
per call and re-JIT every script from scratch. The engine is heavy to construct
but cheap to call once constructed.

**Decision.** One `Engine` instance lives for the daemon lifetime, wrapped in
an `Arc<Engine>`. Scripts are compiled once to AST (`Engine::compile_file` or
`Engine::compile`) and cached in a `HashMap<PathBuf, AST>`. Hot-reload
invalidates the AST cache for changed files. Calling a compiled script
(`Engine::call_fn`) is the only per-tick cost: typically < 0.5 ms for
data-fetch scripts.

**Consequences.** Positive: near-zero per-call overhead, AST cache benefits
repeated calls. Negative: engine and all ASTs stay in RAM for daemon lifetime
(~500 KB base + ~50 KB per unique script). Acceptable: this is less than a
single bash fork.

### ADR-M007 — rhai-widget uses Map-based IR, not builder API or yuck strings

**Status:** Accepted · **Date:** 2026-05-26

**Context.** Phase 4 adds `(rhai-widget ...)`. Three options for how Rhai
produces widget trees:
1. **Yuck string** — function returns a yuck s-expression string; daemon parses it.
2. **Map-based IR** — function returns `#{ type: "box", children: [...] }`.
3. **Builder API** — `meh2.label(text)`, `meh2.box(children)` registered functions.

**Decision.** Map-based IR (option 2).

- Zero new registered Rhai types — uses built-in `Map`. No GC pressure beyond
  the map allocation itself, which is dropped immediately after conversion.
- Simpler implementation than option 3 (no opaque Rhai type registration).
- Faster than option 1 (no string parse overhead on every rebuild).
- Widget rebuilds are rare by design — triggered only by `:watch` var changes,
  not every poll tick. Normal var updates (CPU%, time) still flow through the
  existing reactive binding path and never touch rhai-widget.
- Builder API helpers can be added on top later as thin Rhai functions that
  return maps — they're purely additive and don't require a design change.

**Transfer type.** `RhaiWidgetData` (defined in `rhai-engine/src/lib.rs`,
always compiled). Contains: `widget_type: String`, `attrs: Vec<(String, String)>`,
`children: Vec<RhaiWidgetData>`. Rhai-free — `gtk4-impl` converts it to
`WidgetUse` without importing any `rhai` types.

**Reactivity.** `:watch "VAR1 VAR2"` lists vars that trigger a rebuild.
`RhaiWidgetBinding` (in `gtk4-impl/src/rhai_widget.rs`) stores last-seen values
of watched vars; on change, calls Rhai, converts to WidgetUse, clears container,
appends new child.

**Consequences.** Positive: clean implementation, efficient at runtime, no new
allocations between rebuilds. Negative: widget builder scripts must run
synchronously on the GTK thread (same constraint as `LoopBinding`). Rhai's
500ms operation limit prevents stalls; widget builder scripts should be
microseconds, not milliseconds.

### ADR-M006 — Hybrid config: yuck for layout, Rhai for logic

**Status:** Accepted · **Date:** 2026-05-26

**Context.** Phase 4 adds full Rhai widget definition. The question is whether
yuck and Rhai are alternatives (pick one) or complementary (both at once).

**Decision.** Complementary. yuck remains the layout language — `defwindow`,
`defwidget`, `box`, `label`, etc. Rhai handles imperative logic: computed
values, conditional formatting, dynamic var transforms. A user can:
- Keep pure yuck and get pure meh behaviour.
- Add `.rhai` poll/listen sources while keeping yuck layouts.
- Use Rhai blocks inside yuck for computed attribute expressions (Phase 4).
- Define entire widgets in Rhai (Phase 4, optional).

The principle: yuck is "what it looks like", Rhai is "what it computes".
They do not compete; each does what it is best at.

**Consequences.** Positive: existing yuck configs are fully supported at every
phase; Rhai adoption is gradual. Negative: two languages to document; edge
cases in the interop layer need careful design. Phase 4 design is deferred
until Phases 1–3 are complete and the interop surface is well understood.

-----

## Build profiles and Cargo features

Inherits meh's three profiles. Adds new features for the scripting layer.

### Profiles

| Profile | Command | Includes |
|---|---|---|
| `minimal` | `cargo build --release --no-default-features --features minimal` | Core widgets, poll/listen (shell only), no Rhai, no tray |
| `default` | `cargo build --release` | Everything minimal + systray, subscribe vars, animations, Rhai engine, plugins |
| `full` | `cargo build --release --features full` | Default + GL shader, experimental |

### New features in meh2

| Feature | Adds | Idle cost when unused | Profile |
|---|---|---|---|
| `rhai` | `rhai-engine` crate, `rhai` dep (~1–2 MB) | Zero — engine not created unless a `.rhai` source exists in config | `default` |
| `rhai-plugins` | `plugin-host` crate, plugin discovery | Zero — no plugin loaded unless `~/.config/meh2/plugins/` exists and has entries | `default` |

**Rule:** `--no-default-features --features minimal` must produce a binary with
no Rhai code linked. The `rhai` feature gates the entire `rhai-engine` crate.

-----

## Roadmap

### Phase 0 — Fork baseline (COMPLETE 2026-05-26)

- [x] Fork `~/Projects/meh` to `~/Projects/meh2`
- [x] Rename binary `meh` → `meh2`
- [x] Update config dir `~/.config/meh` → `~/.config/meh2`
- [x] Update cache/log dirs `~/.cache/meh` → `~/.cache/meh2`
- [x] Update socket/pid prefix `meh-server_` → `meh2-server_`
- [x] Init new git repo, initial commit
- [x] Write this CLAUDE.md

meh2 at this point is a usable bar. `meh2 daemon && meh2 open bar` works
with any config that worked with meh (point it at `~/.config/meh` via
`meh2 --config ~/.config/meh daemon` if needed).

---

### Phase 1 — Rhai poll/listen sources (TARGET: fully usable after this phase)

**Goal:** `defpoll` and `deflisten` accept `.rhai` files as their `script`
source. Shell sources still work unchanged. The Rhai engine runs in-process:
no fork, no subprocess. Poll latency drops from ~50–200 ms (fork+exec) to
< 1 ms.

**Deliverables:**

- [x] Add `crates/rhai-engine/` crate:
  - `RhaiEngine` struct wrapping `rhai::Engine` + AST cache
  - Sandbox setup per ADR-M003: no FS, no net, no subprocess
  - `meh2` module registered: `meh2.update(var, val)`, `meh2.read_file(path)`,
    `meh2.shell(cmd)` (explicitly allowed, logs a warning, returns stdout)
  - Per-call timeout via `Engine::set_max_operations`
  - `compile(path) -> AST`, `call(ast, fn_name, args) -> DynVal`
  - Feature-gated behind `rhai` Cargo feature

- [x] Wire into `crates/script-vars/`:
  - `run_source()` dispatcher: `.rhai` ext or `rhai:` prefix → engine, otherwise shell
  - `defpoll :script "path.rhai"` works; `defpoll :script "rhai: expr"` works
  - `deflisten :script "path.rhai"` works (poll-style loop at 1s interval)
  - Shell path completely unchanged; existing configs unaffected

- [x] yuck parser update — no change needed (routing by extension at runtime)

- [x] Add `examples/rhai-bar/` config:
  - CPU usage from `/proc/stat` in Rhai (two-sample diff, no subprocess)
  - RAM from `/proc/meminfo` returned as JSON
  - Time via `rhai: run_shell("date +%H:%M")` inline
  - Hostname from `rhai:` inline
  - Onclick handler via `scripts/greet.rhai` (return value run as shell cmd)

- [x] Performance comparison doc `benches/baselines/rhai-vs-shell.md`
- [x] Update PKGBUILD for meh2 package name

- [x] Real config migrated (`~/.config/meh2/`) — all shell script paths, IPC calls, toggle scripts updated to meh2
- [x] getSysStats Python → `scripts/getSysStats.rhai` (eliminates Python startup per tick)

**Usability gate:** Phase 1 complete and usable. Existing yuck configs work unchanged.

---

### Phase 2 — Rhai event handlers (TARGET: fully usable, improves interactivity)

**Goal:** `:onclick`, `:onscroll`, `:onchange`, `:onhover` can reference `.rhai`
files or inline Rhai expressions. Handlers run off the GTK main thread with a
timeout so a slow script never blocks the UI.

**Deliverables:**

- [x] `rhai-engine` eval runs on `tokio::task::spawn_blocking` — never blocks GTK thread.
  Timeout enforced by `Engine::set_max_operations(500_000)`.

- [x] `gtk4-impl` `spawn_cmd()` updated:
  - Ends in `.rhai` → run via engine in `spawn_blocking`, non-empty return → `sh -c`
  - Starts with `rhai:` → eval inline via engine
  - Otherwise → existing shell spawn (unchanged)

- [x] `CONFIG_DIR` global in `gtk4-impl`; `set_config_dir()` called from daemon
  so relative `.rhai` paths in onclick attrs resolve correctly.

- [x] `examples/rhai-bar/scripts/greet.rhai` — onclick handler example

**Usability gate:** Phase 2 complete. onclick/onscroll/onhover/etc. accept `.rhai`
or `rhai:` in addition to shell commands. Fully backward compatible.

---

### Phase 3 — Rhai plugin system (COMPLETE 2026-05-26)

**Goal:** Users can drop a plugin directory into `~/.config/meh2/plugins/` and
it contributes new data sources (vars) to the bar. Plugins are pure Rhai —
no compilation, no binary. The user references plugin-provided vars in their
yuck config like any other var.

**Deliverables:**

- [x] Add `crates/plugin-host/` crate:
  - Discover plugin dirs from `~/.config/meh2/plugins/` and
    `~/.local/share/meh2/plugins/`
  - Parse `plugin.toml` manifest: name, version, declared vars (name, type,
    interval or listen), file-access allowlist
  - Load `main.rhai`, compile to AST, register declared vars into daemon's var graph
  - On each tick: call `fn get_<var_name>() -> String` in the plugin's Rhai scope
  - Plugin errors are isolated: one broken plugin does not crash the daemon

- [x] Plugin manifest format (`plugin.toml`) — see `docs/plugins.md`

- [x] Hot reload: `meh2 reload` invalidates plugin AST cache so next tick
  recompiles changed scripts. Adding/removing plugins requires daemon restart.

- [x] Add `examples/plugin-demo/` — sysinfo plugin providing `PLUGIN_CPU` and
  `PLUGIN_RAM` from `/proc` with no subprocess

- [x] Document plugin authoring in `docs/plugins.md`

**Key files:**
- `crates/plugin-host/src/inner.rs` — discovery, `start_plugins`, poll tasks
- `crates/plugin-host/src/manifest.rs` — `plugin.toml` types
- `crates/rhai-engine/src/inner.rs` — `call_fn` method added for named function calls
- `crates/script-vars/src/lib.rs` — `start_all` now returns `(rx, tx)` so plugins
  share the same update channel
- `crates/daemon/src/lib.rs` — calls `start_plugins` after `start_all`;
  `IpcCmd::Reload` intercept invalidates plugin ASTs before GTK reload

**Usability gate:** After Phase 3, community plugins are possible. A user
installs a plugin by cloning a directory — no compilation, no `sudo`.

---

### Phase 4 — Rhai widget construction

**Status: Complete.** Implemented 2026-05-26. See ADR-M007.

**Deliverables:**

- [x] `RhaiWidgetData` transfer type in `rhai-engine/src/lib.rs` — Rhai-free struct,
  always compiled, so `gtk4-impl` never imports Rhai types directly
- [x] `call_fn_as_widget_data(path, config_dir, fn_name)` on `RhaiEngine` — calls
  a named Rhai function, converts the returned `Map` to `RhaiWidgetData` recursively
- [x] `dynamic_to_widget_data()` — recursive `Dynamic → RhaiWidgetData` converter;
  handles string/i64/f64/bool attrs, nested `children` arrays
- [x] `crates/gtk4-impl/src/rhai_widget.rs` — `build_rhai_widget()` builds the initial
  widget; `RhaiWidgetBinding` subscribes to `:watch` vars and rebuilds on change
- [x] `AnyBinding::RhaiWidget` variant in `gtk4-impl` — integrated into the
  existing reactive binding dispatch loop
- [x] `"rhai-widget"` wired into `build_basic()` dispatch — available in any yuck config

**Usage:**
```
(rhai-widget :src "widgets.rhai" :fn "my_widget" :watch "CPU MEM")
```
The Rhai function must return a `Map` with at least a `"type"` key:
```rhai
fn my_widget() #{
    type: "box",
    orientation: "h",
    children: [
        #{ type: "label", text: "CPU: " + some_value() }
    ]
}
```
Rebuild cost: one `Engine::call_fn` + container swap (~50 µs typical), triggered
only when a watched var changes. No overhead on ticks that don't touch watched vars.

Example: `examples/rhai-widget/`

---

### Phase 5 — Hybrid yuck+Rhai (long-term, post Phase 4)

**Goal:** Rhai expressions usable inline inside yuck attribute values beyond
what SimplExpr currently supports. Computed layout (e.g. `(for …)` blocks
driven by Rhai arrays). Bidirectional: yuck can call Rhai functions; Rhai can
reference yuck defwidgets.

**Design is deferred.** Start this only after Phase 4 ships and real usage
patterns emerge.

-----

## Coding conventions

Inherits all conventions from meh's CLAUDE.md. Additions:

- **Rhai API surface is minimal.** Every function registered on the `meh2`
  module needs a justification. Don't expose things "just in case".
- **Rhai errors are never panics.** All `Engine::call*` results are `match`ed;
  errors are logged with `tracing::error!` and a fallback/last-good-value
  returned. The daemon must never crash due to a script error.
- **Plugin isolation is non-negotiable.** One plugin's error must not affect
  other plugins or the daemon. Wrap every plugin call in `catch_unwind` +
  the engine's own error handling.
- **Measure before claiming perf.** Every Rhai vs shell comparison needs a
  number in `benches/baselines/`.

-----

## Performance principles

Same as meh, plus:

- **Rhai engine is lazy-init.** `RhaiEngine` is created only when the first
  `.rhai` source appears in a loaded config. Static yuck-only configs: zero
  Rhai overhead, same as meh.
- **AST cache is the hot path.** `compile()` is called once per file per
  hot-reload cycle. `call()` is called every poll tick and must stay < 1 ms
  for data-fetch scripts.
- **Plugin load is startup-only.** Plugin discovery, manifest parsing, and
  AST compilation happen once at daemon start (and on `meh2 reload`). No
  per-tick plugin discovery.
- **Timeout is non-negotiable.** A Rhai script that loops forever must not
  stall the daemon. The 500 ms operation limit is enforced by the engine
  itself; no spawned thread needed for the limit.

-----

## Rules for Claude Code

- **Read this file top-to-bottom at the start of every session.**
- **The prime directive is non-negotiable.** Zero overhead when Rhai is unused.
- **meh is read-only.** Never modify `~/Projects/meh`. Cherry-pick only.
- **Rhai errors are never panics.** All script errors are caught and logged.
- **ADRs are append-only.** Add a new ADR when making a design decision.
- **No native plugins.** If someone asks for `.so` plugin support, the answer is no.
- **No X11 / GTK3 / other scripting languages.** Stop and ask if a task implies them.
- **Measure perf before claiming improvement.** Every Phase 1/2 change touching
  poll latency or RSS needs a number.
- **Phases are sequential.** Do not start Phase 2 work until Phase 1 is shippable.
  Do not start Phase 3 until Phase 2 is shippable. "Shippable" = the binary builds,
  existing yuck configs work, new feature works in the example config.
- **After Phase 1 and Phase 2: meh2 must be fully usable as a daily bar.**
  No half-finished states that break the bar. Feature flags ensure this.
- **When unsure between two approaches, ask.** Don't pick silently.

-----

## Getting started — building meh2

```bash
cd ~/Projects/meh2
cargo build --release
sudo install -m755 target/release/meh2 /usr/bin/meh2
mkdir -p ~/.config/meh2
# Copy your meh config or create a new one:
# cp -r ~/.config/meh/* ~/.config/meh2/
meh2 daemon &
meh2 open bar
```

To run alongside meh (both at once):
```bash
# meh runs on ~/.config/meh — unchanged
meh daemon &
meh open bar

# meh2 runs on ~/.config/meh2 — separate socket, separate config
meh2 daemon &
meh2 open bar
```

The IPC sockets are derived from a hash of the config directory path, so
`meh` and `meh2` never conflict even if they use the same config dir.
