# Rhai vs shell subprocess — poll latency baselines

Hardware: AMD Ryzen 5 3600 (Zen2), release build, Hyprland compositor.

Measurement method: repeated timed loops in bash (`date +%s%N`), 100–1000
iterations per scenario, averaging. All numbers exclude the 200ms intentional
sleep in cpu.rhai (which is inherent to the two-sample CPU diff algorithm, not
engine overhead).

Date: 2026-05-26

---

## Fork+exec baseline

These are the costs shell-based poll sources pay every tick, regardless of what
the script does:

| Operation                  | Latency    | Notes                              |
|----------------------------|------------|------------------------------------|
| `sh -c true` (fork+exec)   | ~1300 µs   | Minimum cost per shell source tick |
| `sh -c "date +%H:%M"`      | ~1780 µs   | date + fork+exec overhead          |
| `sh -c "awk … /proc/meminfo"` | ~1740 µs | awk + fork+exec overhead           |
| `cat /proc/meminfo`        | ~850 µs    | cat + fork+exec (no awk)           |

**Key insight:** even the simplest possible shell source costs ~1.3 ms on each
tick. This is dominated by the kernel's fork+exec overhead (~1.3 ms), not the
script's computation (~0.05–0.1 ms for simple file reads).

---

## Rhai in-process estimates

Rhai scripts run in the meh2 daemon process — no fork, no exec. The costs are:

| Operation                           | Estimated latency | Notes                              |
|-------------------------------------|-------------------|------------------------------------|
| Rhai AST lookup (cache hit)         | < 1 µs            | HashMap lookup                     |
| `read_file("/proc/meminfo")`        | ~100–200 µs       | kernel vfs read, no copy           |
| String parse + arithmetic (ram.rhai) | ~10–50 µs        | Rhai VM ops on ~60 lines of text   |
| Two `read_file("/proc/stat")` calls | ~200 µs each      | Same as above                      |
| `run_shell("date +%H:%M")`         | ~1780 µs          | Subprocess — matches shell baseline |
| **ram.rhai total (no subprocess)**  | **~150–300 µs**   | 5–10× faster than shell awk        |
| **time.rhai (uses run_shell)**      | **~1780 µs**      | Same as shell (subprocess used)    |
| **cpu.rhai (two reads + 200ms sleep)** | **~200.3 ms**  | Dominated by intentional sleep     |

> Note: Rhai latency numbers are estimates derived from Rhai benchmark data
> (see https://rhai.rs/book/about/benchmarks.html) + measured kernel I/O times.
> Exact in-process numbers require `perf`/`flamegraph` on a running meh2 daemon;
> those measurements are pending and will be added here.

---

## Per-interval CPU overhead comparison

Assuming a bar with: clock (5s), CPU (2s), RAM (5s), hostname (60s).

### Shell-based config (meh or meh2 with .sh scripts)

Per second average:
- Clock (5s interval): 1780 µs / 5 s = **356 µs/s**
- CPU (2s interval, 200ms sleep blocks): 1800 µs / 2 s = **900 µs/s** (ignoring sleep)
- RAM (5s interval): 1740 µs / 5 s = **348 µs/s**
- Hostname (60s interval): 1780 µs / 60 s = **30 µs/s**

Total overhead per second from fork+exec: **~1634 µs/s ≈ 0.16%** (on one core)

### Rhai-based config (meh2 with .rhai scripts)

- Clock: still uses `run_shell("date")` = same as shell
- CPU: same 200ms sleep; no subprocess overhead on the stat reads (~0.3 ms saved)
- RAM: Rhai file read instead of awk subprocess = ~250 µs vs ~1740 µs → **~1490 µs/s saved** per 5s interval = **~0.03%** saved per core
- Hostname: inline `run_shell("hostname")` once per 60s = negligible

**RSS comparison (estimates):**
- Each bash subprocess: ~4–6 MB RSS while running, zero between ticks
- Rhai engine (persistent): ~2–4 MB RSS always
- 4 active poll sources with Rhai: ~3 MB total vs ~20 MB peak with shell

For configs with many poll sources, Rhai reduces peak RSS significantly because
there are no concurrent subprocess RSS allocations.

---

## When Rhai is faster

Rhai wins vs shell when the poll source:
1. Reads `/proc` or `/sys` files and does arithmetic — no subprocess needed
2. Runs frequently (1–2s intervals) — fork+exec amortizes poorly at high frequency
3. Has several concurrent poll vars — fewer bash processes running simultaneously

Rhai is **equivalent** to shell when:
- The script calls `run_shell()` internally (same subprocess cost)
- The interval is long (60s+) — fork+exec overhead is negligible

---

## Conclusion

For the `rhai-bar` example config:
- `ram.rhai` (5s interval): ~6× lower per-tick cost vs shell awk (250 µs vs 1740 µs)
- `cpu.rhai` (2s interval): dominated by the intentional 200ms sleep; Rhai saves ~1ms overhead
- `time.rhai` (5s interval): no improvement (uses `run_shell` internally)
- **Total saved:** ~0.03–0.05% CPU/core; more significant in RSS (~4 MB fewer peak)

At higher poll frequencies (e.g. 0.5s CPU, 0.5s RAM), the fork+exec savings
compound: each shell tick costs ~1.5 ms, Rhai costs ~0.2 ms — a **7× reduction**
in poll overhead that becomes measurable in idle CPU.
