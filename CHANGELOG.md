# Changelog

All notable changes to meh are documented here.

## [Unreleased]

### Added
- **`(launcher)` attr `:terminal`** — when set (e.g. `:terminal "foot"`), PATH
  executable results are launched inside that terminal with `-e`. Defaults to `""`
  (bins run directly, suitable for GUI apps). meh itself has no terminal dependency;
  the user declares it explicitly in their config.
- **stat-ring tooltips** — each system stats ring (CPU, RAM, home, temp) now shows
  a tooltip with the human-readable value and percentage on hover.
- **Aethertune bar icon turns orange** when running (classes `playing` / `hidden`).
  Added `aethertune` identifier class to the bar button for CSS targeting.
- **Launcher results dark background** — `.launcher-results` gets a solid dark
  background so results are readable regardless of what's behind the window.
- **eww switch button** in notification centre settings Row 3 — writes `eww` to
  `~/.local/share/bar_choice` and calls `bar-launch.sh`; the reverse
  `switch-to-meh.sh` lives in the eww scripts dir.

### Fixed
- **Launcher bin results empty** — `:show-bins` was set to `false` in `popups.yuck`.
  Changed to `true` so PATH executables appear in search results.
- **`(launcher)` attrs `:show-bins` and `:show-run-command`** — `:show-bins false`
  restricts results to desktop apps only (no PATH executables); `:show-run-command false`
  removes the "run command" literal fallback row. Both default `true` for existing configs.
- **Launcher: arrow key navigation fixed** — `EventControllerKey` now runs in
  `PropagationPhase::Capture` so Up/Down/Enter/Esc are captured before the GtkEntry's
  default handler; arrow navigation now works reliably.
- **Launcher CSS** — `.launcher` container is now transparent; `.launcher-input` retains
  grey background; input height reduced. `.launcher-row`, `.launcher-row.selected`,
  `.launcher-name`, `.launcher-desc`, `.launcher-run-prefix` classes added to dark and
  light SCSS themes.
- **Bar "Glass" toggle** — `BAR_GLASS` defpoll + `toggle-bar-transparent.sh`; `.right.glass`
  CSS rule makes the bar background fully transparent. Toggle button added to notification
  center settings Row 1.
- **Discord and WeChat bar icons** — `getDiscord` / `getWechat` poll scripts; `discord.sh` /
  `wechat.sh` onclick scripts; `discord-widget` / `wechat-widget` in `bar-launchers`
  (left of WhatsApp). Detect unread count from window title, show running state.

### Fixed
- **`Unknown variable` errors when opening popups** — `MehConfig::load` only pre-populated
  `defvar` initial values; `deflisten` / `defpoll` / `defsubscribe` initial values were sent
  asynchronously and could arrive after the first `open_window` call. Fixed by adding
  `ScriptVarDefinition::initial_value()` and pre-populating all script vars into `var_state`
  at load time. Fixes wifi popup "Unknown variable WIFI_STATE" and invisible play button
  in volume popup.
- **Slow-polling vars blank after reload** — `reload_config` discarded live var values when
  loading the new config; vars with long intervals (e.g. `USERNAME`, `SYS_INFO` at 60 s)
  stayed blank until the next tick. Added `carry_var_state()` which copies all current var
  values into the freshly loaded config before replacing it. The async updater overwrites
  each value on its next tick so staleness is bounded by the poll interval.
- **Launcher crash on Enter with no results** — the `Enter` key handler fell through to the
  literal run-command branch even when `:show-run-command false`, running whatever was typed
  (e.g. `meh`) as a shell command and crashing the daemon. Gated the branch on `show_run_command`.
- **IRC / Discord icon colours ignored** — `.module.irc.running` and `.module.discord.running`
  had equal specificity to but appeared before `.launchers .module.running` in the stylesheet,
  so the later rule won. Moved the per-icon overrides into the `.launchers` block so they
  appear after and take effect correctly.

### Fixed
- **Duplicate deflisten processes** — every script using `cmd | while read; do ...; done`
  spawned the while loop as a visible bash subshell. Fixed with `shopt -s lastpipe` in
  all three bash deflisten scripts (cava-meh, wifi-available, player-meta). The two
  inline `sh -c` deflisten blobs (pacman, calendar) were extracted into proper
  `getPacman-listen.sh` / `calendar-listen.sh` scripts with the same fix.
- **Orphaned scripts survive daemon restart** — `kill_orphaned_scripts` used SIGTERM
  which bash shells blocked in inotifywait kernel waits can ignore. Switched to SIGKILL.
- **Bar launch leaves stale daemon alive** — `bar-launch.sh` now loops up to 2 s waiting
  for `meh ping` to fail before starting a new daemon, then hard-kills any survivor with
  `pkill -9 -x meh`. Prevents dual-daemon situations on repeated launches.
- **Theme switch freezes bar** — `toggle-reverse-theme.sh` was using `pkill meh` + full
  daemon restart on every theme change. Replaced with `meh reload`; CSS reloads in-place
  with no bar flicker or freeze.
- **GTK4 4.10 deprecation warnings in `circular-progress`** — `style_context().add_provider()`
  replaced with `style_context_add_provider_for_display` scoped to a unique CSS class;
  redundant DrawingArea CSS provider removed (draw_func already clears to transparent).
- **tooltip binding never registered when var not yet in scope** — `eval_attr_str`
  for a tooltip attr containing a defpoll var ref returned `None` at window-build
  time (initial poll still running), causing the entire tooltip block including
  `maybe_bind` to be skipped. Changed to `unwrap_or_default()` so the binding is
  always registered; the setter fires on the first real poll value. Matches the
  pattern already used for the `class` binding.
- **orphaned inotifywait processes for /tmp/meh/ triggers** — `kill_orphaned_scripts`
  only matched processes with the config scripts dir in their cmdline. Added `/tmp/meh/`
  as a second match needle so inotifywait processes watching cal_trigger and similar
  files are terminated on daemon restart.
- **deflisten subprocess leak** — listen vars now run for the daemon's lifetime
  without window gating. Killing/restarting on every popup open/close was
  accumulating orphaned grandchild processes (inotifywait, playerctl --follow,
  nmcli monitor, etc.). Subprocesses restart automatically if they die.
- **bar flicker on popup close** — `update_vars()` was calling
  `rebuild_open_windows()` (full window close/reopen) instead of
  `update_bindings()` (reactive, O(bindings)). Now only changed bindings
  are pushed.
- **menus not closing on click-outside** — click-catcher and popup windows
  moved from `stacking "fg"` / `stacking "bottom"` to `stacking "overlay"`,
  ensuring they receive input above app windows on Wayland.
- **deflisten process groups** — spawned with `.process_group(0)` so
  `killpg(SIGTERM)` on shutdown reaches grandchildren, not just the shell
  wrapper.

### Added
- **Native `(launcher)` widget** — instant app search via `gio::AppInfo`,
  PATH executable autocomplete, keyboard nav (↑/↓/Enter/Escape), click-to-launch,
  and a literal "run command" fallback row. No subprocess per keystroke.
- **`dots` CLI** — symlink to `dotbackup`; `dots backup`, `dots restore`,
  `dots check`, `dots list`, `dots prune`, `dots clean` subcommands.
- **PKGBUILD** — Arch Linux package build script.
- **Git repository** — project now tracked in git.

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
