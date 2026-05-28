# Changelog

All notable changes to meh2 are documented here.

## [Unreleased]

### Added
- **`builtin-default-config` Cargo feature** — embeds `examples/minimal-bar/` yuck + SCSS into the binary. When no `~/.config/meh2/` config exists, the embedded minimal bar (clock, hostname, date, username, quicklaunch icons) is used automatically. Feature is opt-in (`--features builtin-default-config`), not part of any default profile.
- **`meh2-default-config/` PKGBUILD** — variant that builds with `builtin-default-config` for AUR. `makepkg -si` produces a `meh2` binary that works out of the box with zero configuration.
- **Performance section in README** — documents idle CPU, poll latency, Python elimination, RSS savings in a comparison table.
- **Minimal bar updated** — rounded bottom edges (`border-radius: 0 0 12px`), username via `whoami` poll on the left end, quicklaunch icon buttons (browser ``, terminal ``, file manager ``) for config reference.

### Fixed
- **CI: gtk4 version requirement downgraded from v4_18 to v4_14** — same fix as meh. Required GTK ≥ 4.18 but Ubuntu 24.04 has 4.14.5. No v4_16+ APIs used, so `v4_14` works on both Ubuntu CI and Arch Linux (GTK 4.18+).
- **CI: branch trigger fixed** — workflow was listening for pushes to `main` but the default branch is `master`. CI never fired.
- **CI: `-Dintrospection=false` added to gtk4-layer-shell meson build** — `gobject-introspection-1.0` not available on Ubuntu 24.04 runner.
- **CI: `libadwaita-1-dev` added to system dependencies** — needed by `libadwaita-sys` for the `animations` feature.
- **CI: clippy fixes** — `collapsible_if` and `too_many_arguments` in `plugin-host/src/inner.rs`, `doc_overindented_list_items` in `plugin-host/src/lib.rs`.

### Runtime optimisations (2026-05-27)

#### tokio runtime — thread count reduction
- **`worker_threads(4)`** — was using all CPU cores (12); a bar app is I/O-bound, not CPU-bound. Saves 8 × 2 MB virtual stack entries.
- **`thread_stack_size(512 KiB)`** — default tokio stack is 2 MB per thread; 512 KiB is safe for the async poll/listen tasks used here.
- **Tokio feature set trimmed** — from `features = ["full"]` to an explicit list of what is actually used (`rt`, `rt-multi-thread`, `sync`, `time`, `process`, `net`, `fs`, `io-util`, `macros`, `signal`). Removes dead code from the `io-std`, `parking_lot`, and `test-util` sub-features.

#### Config-level changes (applied in `~/.config/meh2/`)
- **Eliminated `deflisten PLAYER_STATUS` and `deflisten PLAYER_TITLE`** — both used `playerctl --follow`, keeping two persistent subprocesses alive (~5 MB RSS each). Status field merged into `PLAYER_META` JSON; title accessed via `PLAYER_META.title`.
- **Merged `PLAYER_POSITION` + `PLAYER_POSITION_FMT`** — both polled `playerctl position` every 2s. Combined into one defpoll returning `{"pos": <f64>, "fmt": "MM:SS"}`, halving playerctl calls.
- **Interval bumps**: BLUETUITH_RUNNING 2s→5s, NCMPCPP 2s→3s, HOTSPOT/IMPALA 2s→5s, PULSEMIXER 2s→3s.

### meh2 — Rhai scripting + plugin system

#### Phase 1 — Rhai poll/listen sources
- **Rhai scripting engine** (`crates/rhai-engine/`) — in-process poll/listen sources, no fork
- **`.rhai` file sources** and `rhai:` inline expressions for `defpoll`/`deflisten`
- **Rhai API**: `read_file()`, `run_shell()`, `parse_int()`, `parse_float()`, `env_var()`, `path_exists()`, `read_or()`, `write_cache()`, `read_cache()`
- **`json_decode(json_str)`** — parse JSON arrays AND objects into Rhai maps/arrays/values (via serde_json)
- **`json_encode(value)`** — serialize any Rhai value back to JSON string
- **`read_cache(key)`** — reads from `~/.cache/meh2/<key>`; symmetric with `write_cache()`
- **AST cache** — scripts compiled once per hot-reload cycle; per-tick cost < 1 ms
- **`examples/rhai-bar/`** — demo bar using Rhai for CPU, RAM, time, onclick handler
- **Performance baseline**: fork+exec ~1.3–1.8 ms vs Rhai ~0.2 ms for `/proc` reads

#### Phase 2 — Rhai event handlers
- **Rhai event handlers** — `:onclick`/`:onscroll`/`:onhover`/`:onchange` accept `.rhai` files or `rhai:` inline
- Handlers run in `spawn_blocking` — never block GTK main thread
- 500ms operation limit enforced by Rhai engine; runaway scripts interrupted, last good value kept

#### Phase 3 — Plugin system
- **Rhai plugin system** (`crates/plugin-host/`) — drop a directory into `~/.config/meh2/plugins/`
- `plugin.toml` manifest: name, version, declared vars, file-access allowlist
- Plugins contribute `defpoll`/`deflisten`-style vars to the bar
- `meh2 reload` invalidates plugin AST cache; adding/removing plugins requires daemon restart
- **`examples/plugin-demo/`** — sysinfo plugin with `PLUGIN_CPU` and `PLUGIN_RAM` from `/proc`

#### Phase 4 — Rhai widget construction
- **`(rhai-widget :src "f.rhai" :fn "fn" :watch "VARS")`** — Rhai functions return map-based widget trees rendered live
- Map-based IR (`RhaiWidgetData`) — no new Rhai types, drop-in conversion to `WidgetUse`
- Rebuild triggered only by `:watch` var changes (~50 µs typical)
- **`examples/rhai-widget/`** — example widget defined in Rhai

#### Phase 4.5 — Plugin-registered defwidgets
- Plugins declare `[[widgets]]` in `plugin.toml`; those names work directly in yuck as `(my-widget :attr "val" :watch "VARS")`
- `WIDGET_REGISTRY` (`OnceCell`) populated by `plugin-host` at startup, checked by `build_basic()` for unknown widget names
- Call-site attrs evaluated once at build time, merged with watched-var values on every rebuild

#### `defsubscribe :file`
- **inotify-backed file watching** — instant response to file changes; zero polling overhead
- Two-phase watcher: watches parent directory if file doesn't exist yet (handles missing/recreated files)
- **Handles atomic writes** — `EventKind::Remove` detected; watcher transitions back to parent dir for next `Create`
- Tilde (`~`) paths expanded
- **10 toggle flag files** converted from 60s/2s polls to inotify: `BAR_EXTENDED`, `PILL_MODE`, `CONTROLS_IN_BAR`, `CAVA_VISIBLE`, `ICON_DIM`, `REVERSE_THEME`, `BROWSER`, `BAR_ISLAND`, `BAR_AUTOHIDE`, `BAR_GLASS`
- `BAR_POS`, `NOTIF_SETTINGS`, `BAR_MONITOR_LABEL` also converted

#### Python elimination — full poll path migration
All high-frequency scripts migrated to `.rhai`; Python fully eliminated from daemon polling.

| Script | Replaces | Interval | Notes |
|---|---|---|---|
| `getSysStats.rhai` | Python | 3s | CPU/RAM/temp from `/proc`+`/sys`; delta cache via `write_cache` |
| `getDiscord.rhai` | Python | 2s | `json_decode(hyprctl clients -j)` for unread count |
| `getWhatsapp.rhai` | Python | 2s | `json_decode(hyprctl clients -j)`; handles multiple title formats |
| `getIrc.rhai` | bash | 3s | pgrep |
| `getMail.rhai` | bash | 5s | pgrep |
| `getSpotify.rhai` | bash | 2s | playerctl + `write_cache`/`read_cache` for now-playing state |
| `getVolume.rhai` | bash | 2s | Single `wpctl get-volume` call; extracts vol+mute together |
| `bluetooth.rhai` | bash | 3s | bluetoothctl; toggle still handled by `bt-toggle.sh` |
| `getIphone.rhai` | bash+Python | 5s | Battery icon selection in Rhai (was Python `chr()`) |
| `getNcmpcpp.rhai` | bash+Python | 2s | `json_decode(hyprctl clients -j)` + mpc |
| `getAethertune.rhai` | bash+jq | 3s | `json_decode(hyprctl clients -j)` + `read_file` history |
| `getHeadphones.rhai` | bash | 5s | Single bluetoothctl call per device (was two) |
| `getTorrra.rhai` | bash | 5s | pgrep |
| `getOpencine.rhai` | bash | 5s | pgrep |
| `getPulsemixer.rhai` | bash | 2s | pgrep |
| `getSinks.rhai` | Python | 3s | Parses `pactl list sinks` output in Rhai; no Python |
| `getHotspot.rhai` | bash | 2s | ideviceinfo + nmcli |
| `getImpala.rhai` | bash | 2s | pgrep |
| `getMicVol.rhai` | bash+awk | 2s | Single wpctl call for level+mute |
| `getPlayerPositionFmt.rhai` | bash+awk | 2s | MM:SS formatting in Rhai |
| `getWifiNetworks.rhai` | Python | 15s | Parses nmcli output in Rhai; deduplicates SSIDs |
| `getWallpapers.rhai` | Python | 30s | magick thumbnails via `run_shell`; `push()`/`json_encode` for grid |
| `getPlayers.rhai` | 258-line Python | 2s | PIL→magick; hashlib→md5sum; urllib→curl; regex→string ops |

Remaining bash polls: `network` (1s, nmcli-heavy), `getProtonVPN` (10s, `timeout 4`), `getWeather` (600s).
Remaining Python file: `notif-focus.py` — onclick utility only, never polled.

#### Known Rhai quirks
- `string.trim()` / `string.replace()` are **in-place**, return `()` — never assign their result
- Template strings `` `${var}` `` work in `.rhai` files; use `+` inside yuck strings
- `json_decode` on error returns an empty map `#{}` — check `len() > 0` before indexing
- No built-in regex; use `split()`, `contains()`, `index_of()`, `sub_string()` instead

---

### meh upstream (cherry-picked fixes and features)

#### Added
- **`(launcher)` attr `:terminal`** — PATH binaries launched in terminal with `-e`
- **stat-ring tooltips** — CPU/RAM/home/temp rings show value + percentage on hover
- **Aethertune bar icon** turns orange when playing (class `playing`/`hidden`)
- **Launcher results dark background** — readable over any wallpaper

#### Fixed
- **Launcher bin results empty** — `:show-bins` was `false`; fixed to `true`
- **Launcher arrow key navigation** — `EventControllerKey` in `PropagationPhase::Capture`
- **Bar "Glass" toggle** — `BAR_GLASS` flag; `.right.glass` CSS rule for transparency
- **Discord/WhatsApp bar icons** — `discord-widget`/`whatsapp-widget` detect unread count; WhatsApp icon blinks green (#25D366) not cyan
- **`Unknown variable` on popup open** — `deflisten`/`defpoll` initial values pre-populated into `var_state` at load time
- **Slow-polling vars blank after reload** — `carry_var_state()` carries live values into reloaded config
- **Launcher crash on Enter with no results** — gated on `show_run_command`
- **IRC/Discord icon colours ignored** — moved overrides inside `.launchers` block for correct specificity
- **Duplicate deflisten processes** — `shopt -s lastpipe` in all bash deflisten scripts
- **Orphaned scripts survive restart** — `kill_orphaned_scripts` switched to SIGKILL
- **Bar launch stale daemon** — `bar-launch.sh` waits for `meh ping` to fail before restarting
- **Theme switch freezes bar** — replaced daemon restart with `meh reload`
- **GTK4 4.10 deprecation warnings** in `circular-progress` widget
- **tooltip binding skipped when var not in scope** — `unwrap_or_default()` ensures binding is always registered
- **deflisten subprocess leak** — listen vars run for daemon lifetime; subprocesses restart if they die
- **bar flicker on popup close** — `update_bindings()` instead of `rebuild_open_windows()`
- **menus not closing on click-outside** — windows moved to `stacking "overlay"`
- **deflisten process groups** — spawned with `.process_group(0)` for clean SIGTERM on shutdown

---

## [0.1.0] — 2026-05-22

Initial release. GTK4 eww fork with:
- Yuck configuration language (ported from elkowar/eww)
- Reactive binding system (ADR-0007): O(bindings) updates, no full tree rebuild
- Poll subprocess gating (ADR-0008): polls paused when no windows open
- Three build profiles: minimal (4.2 MiB), default (6.9 MiB), full
- System tray (opt-in, `systray` feature)
- Declarative animations via `AdwTimedAnimation` (ADR-0009)
- `defsubscribe` for inotify and DBus vars (ADR-0010)
- Granular hot reload — only changed windows are closed/reopened
- Reactive multi-monitor — connect/disconnect handled live
- `(shader)` widget via GtkGLArea (full profile)
