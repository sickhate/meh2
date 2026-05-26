# CPU baseline measurements

Hardware: AMD Zen2, release build (`cargo build --release`), Hyprland compositor.

Measurement method: `/proc/PID/stat` utime+stime over a 60-second window after
at least 60 seconds of settling time.  `ps %CPU` is NOT used — it averages since
process start and inflates early readings by 3–5×.

## Results (2026-05-23)

| Scenario                          | user  | kernel | total  |
|-----------------------------------|-------|--------|--------|
| Daemon only, no windows           | ~0.1% | ~0.07% | ~0.17% |
| Static bar (no poll vars)         | 0.10% | 0.07%  | 0.17%  |
| 1s `defpoll` clock bar            | 0.18% | 0.17%  | 0.35%  |

## Analysis

**Static bar (0.17%)** is the GTK4 + Wayland compositor floor.  Even a window
showing a fixed label costs ~0.17% because:
- The 16ms GLib timer fires ~62×/second to drain the IPC channel.
- The Wayland protocol generates I/O (kernel time) even for an idle surface.

This floor is below the 0.1% headline target when only user time is counted
(0.10%), but total CPU is 0.17%.  Reducing further would require removing the
GLib timer (add latency to IPC) or not showing a window.

**1s clock (0.35%)** adds ~0.18% from the `sh -c "date '+%H:%M:%S'"` subprocess:
- fork + exec `sh`, fork + exec `date`, waitpid, read stdout = ~1–2 ms/second.
- Kernel time nearly doubles (syscall overhead from fork/exec).

## Prime directive interpretation

The < 0.1% target is the aspirational floor for a fully *static* display or
configs with no active `defpoll` vars.  A 1s-resolution clock bar cannot hit
0.1% without a native (no-subprocess) time source, because fork+exec overhead
alone costs ~0.15–0.20%.

Optimisations already in place:
- Poll subprocess is **gated on `windows_open`**: `date` never runs when no
  windows are displayed.  Daemon-only cost drops from 0.5% (measured before
  gating) to 0.17%.
- `forward_var_updates` accumulates pending initial values and flushes them
  immediately on `window_opened` notify, so vars are current at first open.
- `update_bindings()` is O(bindings), not O(widget-tree): only changed values
  cause GTK property sets.

## To-do

- [ ] Add a native `defvar :time` var source (format via Rust `chrono`, no
      subprocess) as a future optimisation for common clock use-cases.
- [ ] Investigate GLib timer-less IPC dispatch (`glib::MainContext::channel`)
      to eliminate the 16ms poll; prototyped but shelved due to executor spin
      on some GLib versions.
