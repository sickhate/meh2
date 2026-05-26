# meh

> **meh** = eww + GTK4. A modern, Wayland-only, performance-first reimagining
> of [elkowar/eww](https://github.com/elkowar/eww) on GTK4. Built primarily for
> personal use. Public so anyone who wants it can use it.
> 
> Pronounced “meh”, as in the noise you make when someone shows you yet another
> bar for Wayland. The original eww is pronounced “with sufficient amounts of
> disgust” — we’re keeping the energy.
> 
> Binary name: `meh`. Daemon command: `meh daemon`. Config dir: `~/.config/meh/`.

-----

## Current state (last updated 2026-05-24)

**Phase 1 complete. Phase 2 complete (all 5 items done 2026-05-24).**

Widget verification config is at `examples/widget-test/` — run with
`meh --config ~/Projects/meh/examples/widget-test daemon` then
`meh --config ~/Projects/meh/examples/widget-test open widget-test`.

To test systray: add `(systray)` to a window in your yuck config. The widget
shows one button per running tray item; left-click activates it. Right-click
menus and icon-change signals are Phase 2 work.

Everything else in Phase 1 is complete:
- Three build profiles: minimal=4.2 MiB, default=6.9 MiB, full=6.9 MiB
- Reactive binding system (ADR-0007): `BINDING_COLLECTOR` thread-local, `Binding`
  structs, `update_bindings()` — O(bindings) updates, no full tree rebuild
- Poll subprocess gating (ADR-0008): polls paused when no windows open; daemon
  idle CPU is ~0.17% (static bar) / ~0.35% (1s clock poll)
- CI at `.github/workflows/ci.yml`; minimal-bar example at `examples/minimal-bar/`
- `jq` and `tz` features gate the jaq and chrono-tz link-time cost (saves ~2.7 MiB
  from the minimal build)

Known outstanding issues:
- `hostname` command not in PATH on this machine — HOSTNAME shows "" in state
- `minimal` binary target was 5 MiB; achieved 4.2 MiB

-----

## Read this first

You are Claude Code working in the **meh** repository. This file is the single
source of truth for what this project is, what it isn’t, and how it’s built.
Read it top-to-bottom at the start of every session. If anything you’re about
to do contradicts this file, stop and ask before doing it.

-----

## Table of contents

1. [What meh is, in one paragraph](#what-meh-is-in-one-paragraph)
1. [The prime directive](#the-prime-directive)
1. [Hard scope](#hard-scope)
1. [Lineage and where to fork from](#lineage-and-where-to-fork-from)
1. [Background — how we got here](#background--how-we-got-here)
1. [Features inherited from eww](#features-inherited-from-eww)
1. [Architecture](#architecture)
1. [Architecture decisions (ADRs)](#architecture-decisions-adrs)
1. [Build profiles and Cargo features](#build-profiles-and-cargo-features)
1. [Coding conventions](#coding-conventions)
1. [Performance principles](#performance-principles)
1. [Roadmap](#roadmap)
1. [Rules for Claude Code](#rules-for-claude-code)
1. [Getting started — step by step](#getting-started--step-by-step)
1. [Claude Code setup](#claude-code-setup)

-----

## What meh is, in one paragraph

A widget system and status bar for Wayland compositors (Hyprland, Sway, river,
niri, …) written in Rust on GTK4. It’s a fork descended from elkowar/eww. Goals,
in order: (1) be as light and fast as a bar can be — sub-0.1% idle CPU, small
binary, low RAM; (2) keep every feature eww had so existing users feel at home;
(3) add a small number of new things that GTK4 makes possible (animations, GL
shaders, granular hot reload); (4) keep the yuck config language so existing
configs and the wider eww rice ecosystem still apply.

It is **not** a plugin platform. It is **not** trying to compete with Waybar,
Quickshell, or Astal. It is a focused, well-engineered single-purpose tool.

-----

## The prime directive

**meh must always run as light and fast as physically possible while still
offering the full feature set to users who want it.**

This is non-negotiable. Every line of code in this repo must respect this:

1. **Off by default if it has any runtime cost.** Tray DBus connections, GL
   contexts, animation tickers — none of these spin up unless the user’s yuck
   config actually uses them.
1. **Pay-for-what-you-use compilation.** Heavy features are Cargo features,
   not unconditional dependencies. `cargo build --release --no-default-features`
   must produce a working minimal bar.
1. **Idle cost is sacred.** A running `meh` showing a clock and a workspace
   indicator must sit at **< 0.1% CPU** on modern hardware. If a change moves
   that number up, it needs justification.
1. **Lazy init, always.** Closed window → no widgets allocated. No `(systray)`
   in config → no DBus connection. Unused `poll` var → command never runs.
1. **No background tasks “just in case”.** Every spawned tokio/glib task
   needs a concrete user-visible reason to be running right now.

When in doubt, the **light path is the default**. The feature-rich path is
opt-in via Cargo feature, via yuck config, or both.

The user’s words: *“sempre com opção para correr o mais leve e rápido e
consumir menos possível, contudo mantendo as features todas.”* Ship every
feature. Make the user who only uses 10% of them pay for 10% of them.

-----

## Hard scope

- **Wayland only.** No X11, no XCB, no `_NET_WM_STRUT`. Ripped out, not
  feature-gated. macOS and Windows are out of scope.
- **GTK4 only.** No gtk3 shims, no gtk3-compat layer.
- **Yuck for configuration.** No rhai, no Lua, no scripting language.
- **No plugin system.** New widgets and var sources are added by writing
  Rust against the core crates and shipping a new `meh` build. Users who
  want exotic widgets fork or contribute upstream. See [ADR-0004](#adr-0004--no-plugin-system).
- **System tray is opt-in.** Compiled in `default` and `full` profiles;
  excluded from `minimal`. See [ADR-0005](#adr-0005--system-tray-stays-as-an-opt-in-feature).

If a task suggests adding X11 / macOS / rhai / gtk3 / plugin loading,
**stop and ask why** before doing anything.

-----

## Lineage and where to fork from

Three upstream repos matter. Clone all three next to the `meh/` directory.

|Repo                  |URL                                     |Branch  |License|Why we care                                                                                                                   |
|----------------------|----------------------------------------|--------|-------|------------------------------------------------------------------------------------------------------------------------------|
|**elkowar/eww** (main)|https://github.com/elkowar/eww          |`master`|MIT    |Yuck parser (`crates/yuck/`), reference for widget semantics, the systray implementation (`crates/notifier_host/`) we’ll port.|
|**elkowar/eww** (gtk4)|https://github.com/elkowar/eww/tree/gtk4|`gtk4`  |MIT    |Partial GTK4 port elkowar started and shelved. Useful as a reference for widget mappings (gtk3 → gtk4 patterns). Stale.       |
|**Ewwii-sh/ewwii**    |https://github.com/Ewwii-sh/ewwii       |`main`  |GPL-3.0|Complete GTK4 port with working `gtk4-layer-shell` integration and hot reload. Uses rhai for config (we replace it).          |

```bash
# from the parent directory where meh/ lives
git clone https://github.com/elkowar/eww          eww-upstream
git clone -b gtk4 https://github.com/elkowar/eww  eww-gtk4-branch
git clone https://github.com/Ewwii-sh/ewwii       ewwii-upstream
```

**Our base.** Fork from `Ewwii-sh/ewwii`. The GTK4 port is done there, layer-shell
works, hot reload is wired. We pull `crates/yuck/` from `elkowar/eww` (MIT) and
plug it into Ewwii’s widget backend, removing rhai entirely. See ADR-0001 and
ADR-0002.

**Licensing of meh.** GPL-3.0, inherited from Ewwii. Code copied from
`elkowar/eww` keeps its MIT header. New files we write are GPL-3.0.

-----

## Background — how we got here

This section captures the reasoning that produced this file. Read it once so
you don’t have to re-litigate decisions.

### Why fork Ewwii and not eww directly

elkowar/eww is GTK3. Porting GTK3 → GTK4 by hand is hundreds of hours of
mechanical work: signals become `EventController`s, `add()` and `pack_start()`
become `set_child()` and `append()`, the `draw` signal is gone, gtk3-rs itself
is no longer maintained. elkowar started a `gtk4` branch but stalled — the
stated blocker was preserving X11 and macOS support, which GTK4 made awkward
(notably, early GTK4 couldn’t use `wlr-layer-shell` on Wayland). We don’t
have that constraint: Wayland only.

`gtk4-layer-shell` now exists and works well. Ewwii already does the GTK4
port, already integrates `gtk4-layer-shell`, already has hot reload working.
The downsides: GPL-3.0 (manageable), small project (~74 stars at last count),
and they replaced yuck with rhai for configuration. We fix the rhai problem
by pulling yuck back in from eww.

### Why we removed the plugin system

An earlier draft of this file specified a custom plugin system. We decided
against it: eww never had plugins and didn’t need them — `defwidget`
composition plus `poll`/`listen` scripts covers most user extensibility. A
plugin API means a stable ABI surface, dylib loading hazards on top of GTK
FFI, and a non-trivial design discussion we don’t need to have. Adding
plugins later if there’s demand is much easier than removing them once
shipped. See ADR-0004.

### Why tray is opt-in but kept

System tray is the single most-requested eww feature (issue #111 was open for
years), and it’s how users interact with apps like NetworkManager, Steam,
Discord. We keep it. But it pulls in `zbus`, which a minimal build shouldn’t have to
link. So it’s a Cargo feature, on by default in the `default` profile, off
in `minimal`. (`dbusmenu` was originally planned but dropped — GTK3-only.)
See ADR-0005.

### What this project is not

- Not a drop-in eww replacement. Configs will work or work with small edits.
- Not competing with Waybar / Ironbar / Quickshell / Astal. The user doesn’t
  care about competition; the goal is “the bar I want to run on my machine”.
- Not a research project. Boring tech, good defaults, performant idle.

-----

## Features inherited from eww

Everything below comes from elkowar/eww and needs to land in meh. Status
notes refer to the path from Ewwii’s GTK4 base — most things are already
ported but need re-verification once yuck replaces rhai.

**Layout widgets:** `box`, `centerbox`, `eventbox`, `expander`, `revealer`,
`overlay`, `stack`, `scroll`.

**Display widgets:** `label`, `image`, `progress`, `circular-progress`,
`graph`, `transform`.

**Interactive widgets:** `button`, `checkbox`, `slider`, `input`,
`combo-box-text`, `color-button`, `color-chooser`, `calendar`.

**System widgets:** `systray` (opt-in, see ADR-0005).

**Menus.** eww has no generic menu widget. Patterns:

- `combo-box-text` for dropdowns.
- Right-click context menus on tray icons — deferred to Phase 2 (`dbusmenu` is GTK3-only; needs a GTK4 replacement such as `gtk4::PopoverMenu` built from the SNI menu path).
- For custom popup menus, build them as separate eww windows triggered by
  `meh open <window>` from a button’s `onclick`. Same approach in meh.

**Configuration:** yuck (S-expression based), CSS for styling.

**Script vars:** `poll` (run command on interval), `listen` (stream lines
from a long-running command). We add `subscribe` for DBus/inotify-driven vars.

**IPC:** unix socket; `meh open`, `meh close`, `meh reload`, `meh update var=value`.

**Hot reload:** Ewwii has window-level. Our roadmap targets widget-level.

-----

## Architecture

```
meh/
├── crates/
│   ├── yuck/           # parser, copied from elkowar/eww (MIT). Standalone, no GTK deps.
│   ├── core/           # widget tree IR, reactive var graph, evaluation.
│   ├── gtk4/           # GTK4 widget implementations: one module per widget.
│   ├── layer-shell/    # gtk4-layer-shell window placement, exclusive zones, anchors.
│   ├── script-vars/    # poll / listen / subscribe (DBus via zbus, inotify, /proc).
│   ├── notifier-host/  # StatusNotifierHost — ported from elkowar/eww. Opt-in feature.
│   ├── daemon/         # tokio runtime, IPC over unix socket, hot reload supervisor.
│   └── cli/            # `meh` binary — both the daemon entry and the client commands.
├── examples/           # sample configs (minimal-bar, full-bar, …).
├── benches/            # criterion benches for hot paths.
├── CLAUDE.md           # this file.
└── README.md
```

**Crate-level rules.**

- `yuck/` and `core/` must not depend on `gtk4` (the crate). They’re pure data
  pipelines. This makes them fast to compile and fast to test.
- `gtk4/` is the only crate that knows about GTK widgets. Everything above
  it works in terms of an `Element` IR.
- `notifier-host/` lives in its own crate, gated by the `systray` feature
  at the top level. `gtk4/` calls into it only when that feature is on.
- `daemon/` and `cli/` are merged into one binary (`meh`) using clap subcommands.

-----

## Architecture decisions (ADRs)

Append-only log. When you make a decision that isn’t already covered here,
**add a new ADR** with the next number. Never edit accepted ADRs in place —
if you change your mind, write a new ADR that supersedes the old one.

### ADR-0001 — Fork Ewwii-sh/ewwii, not elkowar/eww or its gtk4 branch

**Status:** Accepted · **Date:** 2026-05-22

**Context.** elkowar/eww main is GTK3 (months of porting). Its `gtk4` branch is
stale and was scoped for X11+macOS we don’t want. Ewwii is GTK4, has
`gtk4-layer-shell`, has hot reload — but uses rhai for config and is GPL-3.0.

**Decision.** Fork Ewwii. Pull `crates/yuck/` from elkowar/eww (MIT). Strip X11.
Iterate from there.

**Consequences.** Positive: hundreds of hours of port work already done.
Negative: GPL-3.0 inherited; upstream patches need cherry-picking not merging;
Ewwii is small so no upstream momentum to lean on.

**Alternatives.** Port elkowar/eww main ourselves (too much mechanical work).
Resume the eww gtk4 branch (stale, X11-encumbered). Use Ewwii unmodified
(rhai). Write from scratch (yuck parser, var graph, daemon, script-vars
already exist).

### ADR-0002 — Yuck for configuration, not rhai

**Status:** Accepted · **Date:** 2026-05-22

**Context.** Ewwii replaced eww’s yuck with rhai. Yuck is declarative and
familiar to the eww community; rhai is imperative and unfamiliar.

**Decision.** Pull `crates/yuck/` from elkowar/eww (MIT) into our tree as-is.
Wire it into the widget backend from Ewwii. Rhai is removed. Yuck compiles to
an IR once at load; runtime updates mutate the reactive graph, no re-parse.

**Consequences.** Positive: existing eww configs work (or work with small
edits); rice ecosystem stays relevant; yuck parser is production-tested.
Negative: less expressive than rhai (no recursive macros); have to re-implement
yuck → widget mapping on Ewwii’s backend.

**Alternatives.** Keep rhai (alienates the ecosystem). Design a new language
(yak shaving). Use Ewwii’s StaticScript transpiler (incomplete, still rhai
underneath).

### ADR-0003 — Wayland-only, no X11

**Status:** Accepted · **Date:** 2026-05-22

**Context.** Maintaining both X11 and Wayland means two backends, doubled CI,
manual `_NET_WM_STRUT` re-implementation, CSS quirks per backend. User’s setup
is Hyprland; X11 is dead weight for them.

**Decision.** Wayland only. X11 code removed, not feature-gated. Window
placement exclusively via `gtk4-layer-shell`.

**Consequences.** Positive: smaller codebase, no `cfg(feature = "x11")` noise,
single-row CI. Negative: X11 users keep using eww; macOS implicitly unsupported.

**Alternatives.** Feature-flagged X11 (feature-flagged backends nobody runs
rot). XWayland only (layer-shell doesn’t work through XWayland).

### ADR-0004 — No plugin system

**Status:** Accepted · **Date:** 2026-05-22

**Context.** Earlier drafts proposed a plugin API. eww itself never had one
and the user community managed fine with `defwidget` plus `poll`/`listen`
scripts. A plugin system means a stable ABI surface, dylib hazards on top of
GTK FFI, and ongoing maintenance of a public extension contract. None of that
serves the prime directive.

**Decision.** No plugin system. Extensions happen by writing Rust against the
`gtk4/` and `script-vars/` crates and shipping a new `meh` build. If a user
needs an unusual widget, they can fork meh or contribute upstream.

**Consequences.** Positive: smaller surface area, no ABI commitment, simpler
build, faster startup (no dlopen, no symbol resolution). Negative: harder for
users to ship one-off custom widgets without forking.

**Alternatives.** Rust dylib plugin API (rejected — ABI hazard, complexity).
Lua/WASM scripting (rejected — second language, runtime cost). Mirror Ewwii’s
rhai-based plugin API (rejected — contradicts ADR-0002).

We can revisit this if real user demand appears. Removing a non-existent plugin
system later is free; removing a shipped one is impossible.

### ADR-0005 — System tray stays, as an opt-in feature

**Status:** Accepted · **Date:** 2026-05-22

**Context.** eww users overwhelmingly want tray (issue #111). It pulls in `zbus`
and `dbusmenu` — non-trivial dependencies a minimal build shouldn’t link.

**Decision.** Keep tray. Implement as `crates/notifier-host/` ported from
elkowar/eww (already a clean separate crate there). Gate behind a `systray`
Cargo feature. Included in `default` and `full` profiles, excluded from
`minimal`. When the feature is off, the yuck parser rejects `(systray …)` at
parse time with a clear error pointing the user at the right build flag.

**Consequences.** Positive: minimal builds stay tiny; users who want tray get
it with no work. Negative: a Cargo feature to maintain; the parser needs
feature-conditional widget registration.

**Alternatives.** Always on (penalises minimal users). Always off (loses
eww parity). External crate users link themselves (ergonomically awful).

### ADR-0006 — Zero-cost when unused (prime directive, formalised)

**Status:** Accepted · **Date:** 2026-05-22

**Context.** The user explicitly asked: ship every feature, but make features
free when not used. Many bars (Waybar, ironbar) link everything and start every
subsystem regardless of config; we want the opposite.

**Decision.** Three build profiles (`minimal` / `default` / `full`), every
non-essential capability behind a Cargo feature, and every Cargo feature is
also gated at the yuck parser level. Runtime activation is lazy: a feature
compiled in but not referenced in yuck must spawn zero background work. Idle
CPU target: < 0.1% with a clock + workspace minimal config.

**Consequences.** Positive: minimalist users get a truly minimal binary; power
users get everything; idle cost is enforced and measurable. Negative: every PR
that adds a capability must think about feature gates and update the feature
catalogue table; CI must build the three profiles to catch missing gates.

**Alternatives.** Single fat binary (punishes minimal case). Plugin-only
optional features (we said no plugins, ADR-0004). Runtime-only gating (still
pays binary size, still has subsystem-leak risk).

### ADR-0007 — Thread-local binding collector for reactive widget updates

**Status:** Accepted · **Date:** 2026-05-23

**Context.** When a script var updates (e.g., `TIME` from a 1s poll), meh must
push the new value to the relevant GTK widgets.  Naïvely rebuilding the entire
widget tree on every update costs ~2% CPU (full GTK layout pass per second).
The `core` crate must not depend on `gtk4`, so reactive bindings (which capture
widget references) cannot live in `EvalCtx`.

**Decision.** Reactive bindings live in `crates/gtk4-impl` only.  During widget
tree construction, a thread-local `BINDING_COLLECTOR` accumulates `Binding`
objects (each holding a `SimplExpr`, the local variable scope, a boxed
`FnMut(String)` setter, and the last-seen value).  `maybe_bind()` skips attrs
whose expression has no variable references — static attrs get zero bindings.
`LiveWindow` bundles the GTK window with its `Vec<Binding>`.  On `SetVarBatch`,
`update_bindings()` evaluates only changed expressions and calls only the
relevant setters — no layout passes for unchanged widgets.

**Consequences.** Positive: O(bindings) per update, not O(widget-tree); setters
only fire when the value actually changes; no GTK layout overhead for unchanged
widgets.  Negative: widget constructors must call `maybe_bind()` for each
bindable attribute; forgetting it means the attribute won't live-update.
Thread-local state is non-obvious — the collector must be active (via
`collect_bindings()`) when the widget tree is built.

**Alternatives.** Rebuild widget tree on every var update (O(widgets), 2% CPU).
Store bindings in `EvalCtx` (breaks `core` crate's no-GTK rule).  Observer
pattern with per-widget subscriptions (more infrastructure, same end result).

### ADR-0008 — Poll subprocesses gated on `windows_open`

**Status:** Accepted · **Date:** 2026-05-23

**Context.** `defpoll` vars run a shell command on an interval.  With no windows
open there is nothing to display, yet the original code kept spawning subprocesses
continuously.  Measured overhead: ~0.3–0.5% CPU from idle subprocess spawning.

**Decision.** `run_poll` skips `run_shell()` when `windows_open` is false.  An
initial value is always fetched at daemon start (so `var_state` is populated for
the first window open).  A `tokio::sync::Notify` (`window_opened`) fires when
the first window opens, unblocking `forward_var_updates` to flush accumulated
initial values without waiting for the next poll tick.

**Consequences.** Positive: daemon-only CPU drops from ~0.5% to ~0.17%; 
subprocess never runs when invisible.  Negative: after a long window-close period
the displayed value is stale by up to one poll interval on reopen (acceptable for
TIME=1s; noticeable for DATE=60s but rare).  `listen` vars are not yet gated
(continuous subprocess; deferred to a follow-up).

**Measured CPU floor (release build, AMD Zen2, Hyprland, 60s window):**
- Static bar (no polls): 0.17% total (0.10% user + 0.07% kernel)
- 1s clock bar (`defpoll TIME`): 0.35% total — subprocess fork/exec costs ~0.18%

The 0.1% prime directive target is achievable for static/slow-poll configs.
A native (no-subprocess) time var source would reduce the 1s clock to ~0.17%.
See `benches/baselines/cpu.md` for full methodology.

**Alternatives.** Kill/restart poll tasks on window open/close (higher overhead
from task lifecycle).  Always run polls and accept the cost (violates prime
directive).  Rate-limit polls to slower intervals when no windows are open
(partial fix, still wastes CPU).

### ADR-0009 — Animations via AdwTimedAnimation, not CSS transitions

**Status:** Accepted · **Date:** 2026-05-24

**Context.** eww has no animation support. GTK4 CSS transitions exist but are
fire-and-forget (can't be interrupted cleanly, no Rust API to drive programmatically).
We want `:animate-duration` on `progress` and `:opacity` bindings.

**Decision.** Use `libadwaita::TimedAnimation` with `CallbackAnimationTarget`. The
animation drives a Rust closure (sets `fraction` or `opacity`) at each frame tick —
zero cost when FINISHED/IDLE (frame clock callback is removed by GTK). Interruption
is handled cleanly: incoming value pauses the running animation (at its current
interpolated position), then starts a new animation from that position to the new
target. `follow_enable_animations_setting(true)` on every animation respects the
system reduced-motion preference. The entire subsystem is behind the `animations`
Cargo feature — `libadwaita` is an optional dep; `#[cfg(not(feature = "animations"))]`
paths use direct property setters.

**Consequences.** Positive: zero idle CPU (no frame tick unless something is PLAYING);
smooth interruption; reduced-motion respected; opt-in compilation.
Negative: `libadwaita` added as optional dep (binds to system libadwaita ≥ 1.3);
`adw_init()` must be called before first animation (handled via `ensure_adw_init()`
OnceLock). `libadwaita 0.8` is required to pair with the current `gtk4-rs 0.10` in
the workspace; upgrading gtk4-rs to 0.11 would allow libadwaita 0.9.

**Alternatives.** CSS transitions (not interruptible, no Rust-side value tracking).
Custom tick-callback (reimplements what Adwaita already does, more code, more risk).
GStreamer animations (overkill, wrong dep). Always-on (violates prime directive).

### ADR-0010 — `defsubscribe` for reactive inotify and DBus vars

**Status:** Accepted · **Date:** 2026-05-24

**Context.** `defpoll` and `deflisten` require a shell subprocess for every var update.
For battery level, network state, media player info, and file-based values (e.g.,
`/sys/class/power_supply/BAT0/capacity`), the kernel already tracks these values
and can push notifications via inotify or DBus. A polling shell process is wasteful
and adds latency. ADR-0008 noted: "prefer subscribe over poll whenever possible."

**Decision.** Add a third yuck var form `defsubscribe` with two source types.
`:file "/path"` watches a file via inotify (Linux `IN_MODIFY`/`IN_CREATE` events
from the `notify` crate, behind `inotify-vars` Cargo feature). `:dbus-service …`
subscribes to a DBus property via `org.freedesktop.DBus.Properties.PropertiesChanged`
(behind `dbus-vars` feature, using `zbus` which was already a dep for systray).
Subscribe vars always run once started — their idle cost is a kernel file descriptor
(inotify) or a DBus message match rule, effectively zero CPU. They do NOT gate on
`windows_open` because their cost is already zero; keeping them running ensures the
first window open immediately sees the current value without a stale-by-one-interval
problem.  The initial value is emitted at daemon start so `var_state` is pre-populated.

**Consequences.** Positive: battery level, network state, and file-based values
update with true zero latency and zero subprocess cost.  `defpoll` for these sources
becomes unnecessary.  `inotify` and `zbus` are both already present or optional deps.
Negative: complex DBus types (arrays, dicts, structs) return empty string — users
should use `deflisten` with `dbus-monitor` for those.  Subscribe vars don't gate on
`windows_open` (minor: a few bytes of data flow through the var channel while closed).

**Alternatives.** Shell `deflisten` with `inotifywait` (subprocess overhead, extra
dep on `inotify-tools`).  Native `inotify` bindings without the `notify` abstraction
(more code, no cross-platform story if we ever add macOS).  DBus polling via `defpoll`
(higher latency, subprocess overhead, defeats the prime directive).

-----

## Build profiles and Cargo features

Three named profiles, all producing a working `meh`.

### `minimal` — “I just want a bar”

```bash
cargo build --release --no-default-features --features minimal
```

Includes: core layout widgets (`box`, `centerbox`, `eventbox`, `label`,
`button`, `image`, `revealer`), `poll` and `listen` script-vars, yuck,
layer-shell, IPC daemon, window-level hot reload.

Excludes: tray, subscribe vars (inotify + DBus), animations, GL shader, granular hot reload,
`jq()` expression function, `formattime()` with named timezones.

**Binary footprint target: < 5 MB stripped.** (Achieved: 4.2 MiB, 2026-05-23)

### `default` — what most users want

```bash
cargo build --release
```

`minimal` plus: `systray`, DBus `subscribe` vars (UPower, NetworkManager),
`circular-progress`, `graph`, `transform`, declarative animations, granular
hot reload, `jq()` expression function, `formattime()` with named timezones.

**Binary footprint target: < 12 MB stripped.** (Achieved: 6.9 MiB, 2026-05-23)

### `full` — everything

```bash
cargo build --release --features full
```

`default` plus: `(shader …)` widget with `GtkGLArea`, experimental features.

### Cargo feature catalogue

Every feature listed is **truly optional**. `cargo build --no-default-features`
must succeed and produce a usable binary.

|Feature          |Pulls in                                 |Idle cost when unused                        |
|-----------------|-----------------------------------------|---------------------------------------------|
|`systray`        |`notifier-host`, `zbus`, `quick-xml`     |Zero — no DBus until `(systray)` in config   |
|`dbus-vars`      |`zbus`, subscription machinery           |Zero — no proxies until `defsubscribe :dbus-service …` in config |
|`animations`     |`libadwaita` (AdwTimedAnimation)         |Zero — no tickers until a widget animates    |
|`shader`         |`glow`, `GtkGLArea` plumbing             |Zero — no GL context until `(shader)` appears|
|`inotify-vars`   |`notify`                                 |Zero — no watchers until `defsubscribe :file …` in config |
|`granular-reload`|extra IR diffing                         |Negligible — slightly larger reload code     |
|`extra-widgets`  |`circular-progress`, `graph`, `transform`|Zero unless used                             |
|`jq`             |`jaq-core`, `jaq-parse`, `jaq-std`, `jaq-interpret`, `jaq-syn` (~2.7 MiB link) | Zero — only called when `jq(…)` expression used |
|`tz`             |`chrono-tz` timezone database (~2.5 MiB link) | Zero — only called when `formattime(ts, fmt, tz)` used |

Rules:

- Every feature gate at the crate level must also gate the yuck parser
  accepting that widget/var-type. If `systray` is off, `(systray …)` in a
  config is a **parse-time error with a helpful message**, not a runtime panic.
- Never add `default = ["foo"]` where `foo` adds a background task. Background
  work comes from yuck, not from compilation.
- `--no-default-features` must always produce a usable binary.

### Verifying the prime directive

Before merging anything user-visible, run `/audit-weight` (see slash commands).
It runs the minimal build, checks `cargo bloat`, audits spawned tasks, and
diffs the feature catalogue against `Cargo.toml`.

-----

## Coding conventions

- **Edition:** Rust 2024.
- **MSRV:** latest stable. No old-toolchain support.
- **Async:** tokio. No blocking calls on the daemon main loop.
- **Errors:** `anyhow` at binary boundaries; `thiserror` inside library crates.
- **Logging:** `tracing` everywhere. No `println!` in committed code.
- **Format:** `cargo fmt` on save (PostToolUse hook handles this).
- **Lints:** `cargo clippy --workspace -- -D warnings` must pass before commit.
- **Doc comments:** every public item gets one. `#![warn(missing_docs)]` on library crates.

### GTK4 patterns

- Use idiomatic `gtk4-rs`. Don’t translate gtk3 1:1 — many gtk3 patterns have
  cleaner gtk4 equivalents.
- Input → `EventController*`, not legacy signal connections where avoidable.
- Containers → `set_child()` for single-child, `append()` for `Box`. Never
  look for `add()` or `pack_start()`.
- Drawing → `gtk::DrawingArea::set_draw_func`. The `draw` signal is gone.
- CSS: target GTK4’s subset. If a gtk3 selector or property doesn’t work in
  gtk4, document it in `docs/css-notes.md` so users know.

### Dependency discipline

- Don’t add a direct dependency without a one-line justification in the PR.
- Prefer existing transitive deps over new ones.
- Audit periodically with `cargo machete` (unused deps) and `cargo udeps`
  (more aggressive, requires nightly).

-----

## Performance principles

1. **Idle cost is the headline metric.** A clock + workspaces config sits
   at < 0.1% CPU on modern hardware. Anything that moves this needs justification.
1. **Lazy windowing.** A defined-but-closed window allocates nothing until opened.
1. **Variable graph.** Re-render only widgets whose dependencies actually changed.
1. **No re-parse on update.** Yuck compiles to an IR at load; updates mutate state.
1. **Subscribe over poll.** Use DBus signals, inotify, netlink before `poll`.
1. **Benchmark before claiming perf.** `criterion` for hot paths. Baseline in
   `benches/baselines/`.
1. **`cargo bloat` is a tool we use.** Surprising entries get investigated.

-----

## Roadmap

**Phase 1 — Foundations** (get to feature parity with eww)

- [x] Fork Ewwii-sh/ewwii. Strip X11 backend.
- [x] Pull `crates/yuck/` from elkowar/eww. Wire into Ewwii’s widget backend.
- [x] Remove rhai. Remove StaticScript. Remove `eiipm`.
- [x] Remove Ewwii’s plugin API (`ewwii_plugin_api` references).
- [x] Set up the three build profiles. Confirm `minimal` < 5 MB stripped. (4.2 MiB achieved)
- [x] CI: build all three profiles, run clippy with `-D warnings`, run benches. (`.github/workflows/ci.yml`)
- [x] Measure idle CPU. (Static bar: 0.17%; 1s poll clock: 0.35% — see `benches/baselines/cpu.md` and ADR-0008)
- [x] Verify every eww widget works against our yuck → IR → GTK4 pipeline.
      Status: comprehensive test config at `examples/widget-test/` exercises
      all widget types: label, button, box, centerbox, eventbox, image, scale,
      progress, circular-progress, scroll, overlay, revealer, stack, expander,
      checkbox, input, calendar, combo-box-text, color-button, literal.
      Reactive widgets (label, scale, progress, circular-progress) wired to
      `defpoll` vars. Interactive widgets (revealer, stack) use `meh update`.
- [x] Port `notifier-host` crate from elkowar/eww (MIT). Gate behind `systray` feature.
      Status: complete (2026-05-23). Full protocol implementation: Watcher, Host,
      Item, icon resolution (name + theme-path + ARGB pixmap), proxy files.
      GTK3 dbusmenu removed; context menus deferred to Phase 2. `build_systray()`
      in gtk4-impl wired to `TOKIO_HANDLE`; tokio handle set by daemon at startup.
      Right-click menus not yet implemented (Phase 2).

**Phase 2 — Differentiators**

- [x] Declarative animations via `AdwTimedAnimation`.
      Status: complete (2026-05-24). `libadwaita 0.8` (pairs with gtk4-rs 0.10) added as
      optional dep behind `animations` feature. Two animation sites:
      (1) `progress` widget: `:animate-duration` (ms) + `:animate-easing` on the `value`
          binding → smooth `AdwTimedAnimation` via `CallbackAnimationTarget` on `fraction`.
          Interruption-safe: pauses prev anim at current fraction, starts new from there.
      (2) Any widget: `:opacity` is now reactive (bindable) everywhere; adds smooth fade
          when `:animate-duration` > 0 and `animations` feature is on.
      All animations: `follow_enable_animations_setting(true)` respects system reduced-motion.
      Zero idle cost when duration == 0 or feature is off (no frame-clock callbacks registered).
      `ensure_adw_init()` (OnceLock) initialises libadwaita once on first animated widget.
      `parse_adw_easing()` maps CSS-style strings to `libadwaita::Easing` variants.
      Default easing: `ease-in-out-cubic`. Use: `(progress :value volume :animate-duration 200 :animate-easing "ease-out-cubic")`.
- [x] Granular hot reload — change one widget, reload one widget.
      Status: complete (2026-05-24). `granular-reload` Cargo feature in `default` profile.
      On `meh reload`: loads new config, computes per-window IR hash (window def +
      transitive widget deps serialised to JSON, span fields stripped), diffs against
      old config hashes, closes+reopens only changed windows. CSS always reloaded.
      Key files: `crates/gtk4-impl/src/app.rs` (`ir_hash`, `collect_deps`, `strip_spans`,
      `#[cfg(feature = "granular-reload")] reload_config`). Unchanged windows keep
      their GTK state (scroll position, focus, reactive bindings) across a reload.
      `minimal` profile falls back to the original close-all/reopen-all path.
- [x] Reactive multi-monitor — connect/disconnect handled live, no restart.
      Status: complete (2026-05-24). GDK monitors ListModel `items-changed` signal
      wired in daemon. `LiveWindow` tracks `monitor_connector`. On disconnect: window
      closed, queued in `pending_reopen`. On reconnect: reopened at correct index.
      `pending_reopen` cleared on `close_all` and `reload_config`.
- [x] Fractional scaling that actually works on Hyprland and Sway.
      Status: no code needed (2026-05-24). GTK4 4.14+ handles wp_fractional_scale_v1
      natively. All meh sizes are in logical pixels; cairo draw contexts are
      pre-scaled by GTK4. Verified: libgtk-4.so contains wp_fractional_scale_v1
      symbols; Hyprland 0.55.2 supports the protocol; no GDK_SCALE override in meh.
- [x] `subscribe` vars for DBus (`UPower`, `NetworkManager`, `MPRIS2`) and inotify.
      Status: complete (2026-05-24). New `defsubscribe` keyword in yuck. Two sources:
      (1) `:file "/path"` — inotify watch via `notify` crate (behind `inotify-vars` feature,
          on by default). Reads initial value, then re-reads and emits on every
          Modify/Create event. Zero CPU: kernel inotify fd, no polling.
      (2) `:dbus-service … :dbus-object … :dbus-iface … :dbus-prop …` — watches a DBus
          property via `org.freedesktop.DBus.Properties.PropertiesChanged` signal (behind
          `dbus-vars` feature, on by default). Gets initial value via `Properties.Get`,
          then updates reactively on signal. Handles `InvalidatedProperties` by re-reading.
          `:dbus-bus "system"|"session"` selects the bus (default: session).
      Both: always running (near-zero idle), initial value emitted at startup.
      Feature catalogue updated: `inotify-vars` added to `default` profile.
      Examples in `examples/widget-test/meh.yuck`. See ADR-0010.

**Phase 3 — Optional power features (complete)**

- [x] `(shader …)` widget via `GtkGLArea`. Behind `shader` feature (`full` profile only).
      Status: complete (2026-05-24). `(shader :frag "path.glsl" :width 300 :height 200)`.
      OpenGL 3.3 core, fullscreen triangle, glow 0.17. GL function loading via
      `eglGetProcAddress` (EGL/Wayland); links `-lEGL` via `gtk4-impl/build.rs`.
      Available uniforms: `uniform float iTime` (seconds since realize), `uniform vec2 iResolution`.
      Hardcoded vertex shader (fullscreen triangle); user provides fragment shader file.
      Magenta fallback if shader file missing or compile fails.
      Zero cost in `minimal`/`default` — glow not linked, GLArea never allocated.
- [x] Optional `libadwaita` styling integration.
      Status: complete (2026-05-24). `init_platform()` calls `adw::init()` at daemon
      startup (when `animations` feature is on) so libadwaita CSS tokens
      (`@accent_color`, `@window_bg_color`, etc.) are available in SCSS. Exposes
      `MEH_DARK` as a reactive built-in var (string `"true"`/`"false"`) that updates
      live when the OS switches dark/light via `adw::StyleManager` notify signal.
      No-op in `minimal` build. Usage: `(label :text {if MEH_DARK "dark" "light"})`
      or drive SCSS class switching. Key files: `gtk4-impl/src/app.rs`
      (`init_platform`, `connect_color_scheme`), `daemon/src/lib.rs` (wiring).

-----

## Rules for Claude Code

These are the rules **you** follow when working in this repo.

- **Read this file top-to-bottom at the start of every session.** Confirm you
  have before taking action.
- **The prime directive (zero-cost when unused) is non-negotiable.** Before
  adding any new capability, answer in your reply:
  - What Cargo feature gates this? (If “none”, you need a strong reason.)
  - Which build profile (`minimal` / `default` / `full`) does it belong to?
  - What is its idle cost when compiled in but not active in yuck? (Goal: zero.)
  - What changes in the feature catalogue table? Update it in the same change.
- **Default to proposing, not patching.** For non-trivial changes, sketch the
  approach first and wait for confirmation.
- **Cite upstream when porting.** When copying or adapting code from
  `eww-upstream`, `eww-gtk4-branch`, or `ewwii-upstream`, put the source path
  and commit hash in the commit message.
- **Watch the redraw budget.** If you’re touching widget rendering, ask
  yourself whether this adds work to a hot path. Say so in the PR.
- **No new dependency without flagging it.** Justify each in the PR.
- **If you find yourself writing X11 / rhai / gtk3 / plugin-loader code, STOP**
  and ask. These are all explicitly out of scope.
- **Prefer subscribe over poll.** Every time.
- **ADRs are append-only.** Add a new ADR when you make a decision that isn’t
  already documented. Never edit accepted ADRs in place; supersede them.
- **When unsure between two reasonable approaches, ask.** Don’t pick silently.
- **Continuity matters.** I am the user from the “Background” section. Same
  person, same preferences, same scope rules.

-----

## Getting started — step by step

When the user first opens Claude Code in this repo (or starts working on a
fresh checkout), do this in order:

1. **Read this file in full.** Then summarise in 5 bullets what meh is, what’s
   in scope, and what the prime directive means.
1. **Check the upstream clones exist** at `../eww-upstream`, `../eww-gtk4-branch`,
   `../ewwii-upstream`. If any is missing, print the `git clone` command and stop.
1. **Inspect Ewwii’s structure.** `view ../ewwii-upstream/crates/` to understand
   what we’re forking. Note the rhai-touching files (those go away).
1. **Inspect eww’s yuck crate.** `view ../eww-upstream/crates/yuck/` to confirm
   it’s standalone (no GTK deps). Note its `Cargo.toml`.
1. **Propose Phase 1 step 1** as a concrete PR: a `Cargo.toml` workspace
   layout and an initial directory skeleton matching the [Architecture](#architecture)
   section. Don’t write any non-trivial Rust until I confirm the layout.

Do **not** start writing widget code, daemon code, or anything beyond
configuration files in this first pass. The shape comes first.

-----

## Claude Code setup

### Launch command

From `~/projects/meh/`:

```bash
claude --model opus --effort high \
  --add-dir ../eww-upstream \
  --add-dir ../eww-gtk4-branch \
  --add-dir ../ewwii-upstream
```

### `.claude/settings.json`

Create this file with:

```json
{
  "model": "claude-opus-4-7",
  "permissions": {
    "allowedTools": [
      "Read", "Write", "Edit",
      "Bash(cargo *)", "Bash(rustup *)", "Bash(git *)",
      "Bash(rg *)", "Bash(fd *)", "Bash(find *)"
    ],
    "deny": [
      "Read(./.env)", "Read(./.env.*)", "Write(./Cargo.lock)"
    ]
  },
  "hooks": {
    "PostToolUse": [
      { "matcher": "Write(*.rs)", "hooks": [{ "type": "command", "command": "cargo fmt --quiet" }] },
      { "matcher": "Edit(*.rs)",  "hooks": [{ "type": "command", "command": "cargo fmt --quiet" }] }
    ]
  }
}
```

### Recommended plugins (install inside Claude Code)

**1. Rust LSP — essential.** Gives Claude rust-analyzer diagnostics on every
edit. Eliminates “let me run cargo check” round-trips.

```
/plugin marketplace add Piebald-AI/claude-code-lsps
/plugin install rust-lsp
```

Alternative with cargo-bloat / cargo-udeps / cargo-audit / cargo-deny bundled
— better fit for meh because we genuinely care about binary size and clean deps:

```
/plugin marketplace add zircote/rust-lsp
/plugin install rust-lsp
```

**2. Skip the all-in-one packs.** Don’t install full-stack skill bundles or
multi-language packs. They pollute context with irrelevant languages.

### MCP servers worth considering

- **GitHub MCP** so Claude can read issues from elkowar/eww and Ewwii-sh/ewwii
  directly, follow upstream releases, and create PRs in your fork without copy-paste.

### Slash commands to create

Put each in `.claude/commands/<name>.md`. They become `/<name>` in the REPL.

`port-widget.md`:

```
Port the GTK3 widget $ARGUMENTS from elkowar/eww (../eww-upstream/) to our
GTK4 codebase. Follow patterns already in crates/gtk4/. Map gtk3 signals to
gtk4 EventControllers. Replace add()/pack_start() with set_child()/append().
Cite the upstream file path and commit hash in the commit message.
```

`yuck-bind.md`:

```
Add the yuck parser binding for the widget $ARGUMENTS. Update the widget
registry in crates/core/, generate attribute parsing in crates/yuck/, add
a sample config under examples/ and a test under tests/.
```

`audit-weight.md`:

```
Audit the codebase against the prime directive (ADR-0006). Report on:

1. Does `cargo build --release --no-default-features --features minimal` succeed?
2. `cargo bloat --release --crates -n 20` on the minimal build. Flag any
   crate over 500 KB that isn't core (gtk4, glib, tokio, anyhow, yuck).
3. Any dep in `[dependencies]` that should be in `[features]` instead — i.e.
   anything pulled in unconditionally but only used by an optional widget/var.
4. Every `tokio::spawn` and `glib::spawn_future` call: confirm there's a path
   where it doesn't run when the relevant yuck construct is absent.
5. Diff the feature catalogue table in CLAUDE.md against `[features]` in
   the workspace `Cargo.toml`. Flag drift.

Produce a markdown report. Do not change code.
```

`bench.md`:

```
Run `cargo bench --workspace`. Compare against the baseline in
benches/baselines/main.json. Flag regressions over 5%. For improvements,
ask before updating the baseline.
```

`sync-upstream.md`:

```
Check Ewwii-sh/ewwii main for new commits since our last sync (see
.upstream-sync). Identify patches relevant to us — skip X11, rhai,
plugin-API, generic refactors. Propose which to cherry-pick. Don't apply
yet; wait for confirmation.
```

`adr.md`:

```
Add a new ADR to CLAUDE.md under "Architecture decisions". Use the next
free ADR number. Topic: $ARGUMENTS. Required sections: Context, Decision,
Consequences (positive and negative), Alternatives. Append only — never
edit existing ADRs.
```

-----

## First-session prompt

Paste this into Claude Code on your first session:

> Read CLAUDE.md top-to-bottom. Summarise in 5 bullets what meh is, what’s in
> scope, what’s out of scope, what the prime directive means, and what the
> first concrete steps are. Then check that ../eww-upstream, ../eww-gtk4-branch
> and ../ewwii-upstream exist. Don’t write any code yet.

If Claude’s summary is right, you’re good. If it invents something, fix this
file before continuing.