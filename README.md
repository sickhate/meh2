# meh

> *Pronounced "meh", as in the noise you make when someone shows you yet another bar for Wayland.*

**meh** is a GTK4 Wayland widget system and status bar — a spiritual successor to [elkowar/eww](https://github.com/elkowar/eww), rebuilt from the ground up on GTK4 for modern Wayland compositors like Hyprland, Sway, and river.

---

## The story

eww is arguably the best widget system ever written for Linux desktops. It gave you a declarative config language (yuck), reactive variables, poll and listen script sources, and the freedom to build literally anything — bars, dashboards, popups, launchers — all from a single binary with zero runtime dependencies on a desktop environment.

The problem: eww is GTK3. And GTK3 is end-of-life.

GTK4 brings real hardware-accelerated rendering, proper fractional scaling on Wayland, smoother animations, and better layer-shell integration. Running a bar on GTK3 in 2025 means fighting scaling bugs, missing Wayland protocols, and depending on a toolkit that's no longer receiving features.

elkowar started a GTK4 branch but stalled — the scope was too large and X11/macOS compat made the port messy. [Ewwii-sh/ewwii](https://github.com/Ewwii-sh/ewwii) made real progress on the GTK4 port but replaced yuck with rhai, breaking the entire existing eww config ecosystem.

**meh** takes Ewwii's completed GTK4 port, rips out rhai, replaces it with eww's original yuck parser, and pushes the result further than either project went: reactive O(bindings) updates, per-window granular hot reload, native inotify and DBus subscribe vars, declarative animations, and a native app launcher — all with a sub-0.1% idle CPU target.

If you have an eww config, it will mostly just work. If you're starting fresh, you get a faster, cleaner foundation.

---

## Features

- **Yuck configuration** — same S-expression language as eww. Your existing config is compatible.
- **GTK4 + Wayland-native** — `gtk4-layer-shell`, fractional scaling, smooth rendering. No X11.
- **Reactive bindings** — variable updates push only to affected widgets. No full tree rebuilds.
- **Granular hot reload** — `meh reload` closes and reopens only windows whose definition changed. Unchanged windows keep their state.
- **Poll gating** — `defpoll` subprocesses pause when no windows are open. Idle daemon sits at ~0.17% CPU.
- **`defsubscribe`** — inotify and DBus property watchers. Zero-cost reactive sources for battery, network state, media players — no polling, no subprocess.
- **Deflisten process groups** — `deflisten` subprocesses run for the daemon's lifetime; SIGTERM on shutdown reaches grandchildren (inotifywait, playerctl --follow, nmcli monitor, etc.).
- **Declarative animations** — `AdwTimedAnimation` on `progress` values and opacity. Interruptible, respects system reduced-motion.
- **Native app launcher** — `(launcher)` widget: instant `gio::AppInfo` search, PATH executable autocomplete, keyboard nav, click-to-launch. No subprocess per keystroke.
- **System tray** — StatusNotifierItem/StatusNotifierHost, ported from eww. Opt-in via `systray` Cargo feature.
- **`(shader)` widget** — GLSL fragment shaders via `GtkGLArea`. Full profile only.
- **Three build profiles**: `minimal` (4.2 MiB), `default` (6.9 MiB), `full`.

---

## Why GTK4 is faster

| | eww (GTK3) | meh (GTK4) |
|---|---|---|
| Rendering | Software (cairo only) | Hardware-accelerated (GSK/GL) |
| Fractional scaling | Buggy on Wayland | Native (`wp_fractional_scale_v1`) |
| Animations | CSS transitions only | `AdwTimedAnimation` — interruptible, zero idle cost |
| Layer shell | `wlr-layer-shell` via gdk3 hacks | `gtk4-layer-shell` — proper protocol |
| Idle CPU (clock + workspaces) | ~0.5–1% | ~0.17–0.35% |

---

## Installation

### Arch Linux (AUR / PKGBUILD)

```bash
git clone https://github.com/sickhate/meh
cd meh
makepkg -si
```

### From source

```bash
git clone https://github.com/sickhate/meh
cd meh
cargo build --release
sudo install -Dm755 target/release/meh /usr/local/bin/meh
```

**Dependencies:** `gtk4`, `gtk4-layer-shell`, `libadwaita`, `cairo`, `glib2`, `pango`

**Build dependencies:** `rust` (stable), `cargo`

### Build profiles

```bash
# Minimal — 4.2 MiB, no tray, no animations, no shader
cargo build --release --no-default-features --features minimal

# Default — 6.9 MiB, everything most users want
cargo build --release

# Full — everything including GLSL shaders
cargo build --release --features full
```

---

## Quick start

```bash
# Start the daemon
meh daemon

# Open a window defined in your config
meh open bar

# Reload config (hot reload — only changed windows restart)
meh reload

# Close a window
meh close bar

# Update a variable
meh update MY_VAR=hello
```

Config lives in `~/.config/meh/`. The main file is `meh.yuck`. CSS in `style.scss`.

---

## Example config

```yuck
(defpoll TIME :interval "1s" `date +%H:%M:%S`)
(defpoll DATE :interval "60s" `date "+%A, %B %d"`)

(defwindow bar
  :monitor 0
  :geometry (geometry :width "100%" :height "30px" :anchor "top center")
  :stacking "fg"
  :exclusive true
  (centerbox
    (label :text "meh")
    (label :text TIME)
    (label :text DATE)))
```

---

## Native launcher

Add to your Hyprland config:

```
bind = SUPER, SPACE, exec, meh open --toggle launcher
```

Add to your `meh.yuck` or `popups.yuck`:

```yuck
(defwindow launcher
  :monitor 0
  :geometry (geometry :anchor "top center" :y "80px" :width "640px")
  :stacking "overlay"
  :focusable "exclusive"
  :wm-ignore true
  (launcher :placeholder "Search applications…"
            :max-results 8
            :window "launcher"))
```

Type to search apps and PATH executables. `↑`/`↓` to navigate, `Enter` to launch, `Escape` to close.

---

## Acknowledgements

- **[elkowar/eww](https://github.com/elkowar/eww)** — the original. yuck parser and concept. MIT licensed.
- **[Ewwii-sh/ewwii](https://github.com/Ewwii-sh/ewwii)** — completed the GTK4 port that made this possible. GPL-3.0.
- The eww community — years of configs, issues, and creativity that proved the concept.

---

## License

GPL-3.0-or-later. See [LICENSE](LICENSE).

Code copied or adapted from elkowar/eww retains its MIT header.
