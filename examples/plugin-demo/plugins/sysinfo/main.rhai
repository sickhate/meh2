// sysinfo plugin — CPU usage and RAM stats from /proc, plus a sysinfo-pill widget.
//
// Exported vars:   get_PLUGIN_CPU(), get_PLUGIN_RAM() — called on each poll tick.
// Exported widget: render_sysinfo_pill()               — called when watched vars change.
//
// Plugin-registered widget usage in yuck:
//   (sysinfo-pill)                        ; watches PLUGIN_CPU + PLUGIN_RAM (default)
//   (sysinfo-pill :watch "PLUGIN_CPU")    ; rebuild on CPU changes only

// ── State for CPU delta ───────────────────────────────────────────────────────

// /proc/stat line: "cpu  <user> <nice> <system> <idle> <iowait> <irq> <softirq> ..."
// CPU% = 100 * (delta_active / delta_total)

fn read_cpu_fields() {
    let raw = read_file("/proc/stat");
    let lines = raw.split("\n");
    let cpu_line = "";
    for line in lines {
        if line.starts_with("cpu ") {
            cpu_line = line;
            break;
        }
    }
    let parts = cpu_line.split(" ");
    // parts[0] = "cpu", parts[1] may be empty (extra space), values follow
    let nums = [];
    for p in parts {
        if p != "cpu" && p != "" {
            nums += [parse_int(p)];
        }
    }
    // [user, nice, system, idle, iowait, irq, softirq, ...]
    nums
}

fn get_PLUGIN_CPU() {
    let cur = read_cpu_fields();
    if cur.len() < 4 {
        return "?%";
    }

    let idle_now  = cur[3];
    let total_now = 0;
    for v in cur { total_now += v; }

    let prev_raw = read_file(env_var("HOME") + "/.cache/meh2/plugin_sysinfo_cpu_prev");
    let parts    = prev_raw.split(",");
    let result   = "0%";

    if parts.len() >= 2 {
        let idle_prev  = parse_int(parts[0]);
        let total_prev = parse_int(parts[1]);
        let d_idle  = idle_now  - idle_prev;
        let d_total = total_now - total_prev;
        if d_total > 0 {
            let pct = (100 * (d_total - d_idle)) / d_total;
            result  = pct + "%";
        }
    }

    write_cache("plugin_sysinfo_cpu_prev", idle_now + "," + total_now);
    result
}

// ── RAM ───────────────────────────────────────────────────────────────────────

fn get_PLUGIN_RAM() {
    let raw  = read_file("/proc/meminfo");
    let lines = raw.split("\n");
    let total = 0;
    let avail = 0;
    for line in lines {
        if line.starts_with("MemTotal:") {
            let parts = line.split(" ");
            for p in parts {
                if p != "MemTotal:" && p != "kB" && p != "" {
                    total = parse_int(p);
                }
            }
        }
        if line.starts_with("MemAvailable:") {
            let parts = line.split(" ");
            for p in parts {
                if p != "MemAvailable:" && p != "kB" && p != "" {
                    avail = parse_int(p);
                }
            }
        }
    }
    let used_mb = (total - avail) / 1024;
    used_mb + " MB"
}

// ── Widget: sysinfo-pill ──────────────────────────────────────────────────────
//
// Returns a map-based widget tree. PLUGIN_CPU and PLUGIN_RAM are in scope
// because they are listed in [[widgets]] default_watch; the daemon injects
// their current values before calling this function.
//
// The caller can pass :watch to override which vars trigger a rebuild:
//   (sysinfo-pill :watch "PLUGIN_CPU PLUGIN_RAM")  ; default
//   (sysinfo-pill :watch "PLUGIN_CPU")             ; RAM changes won't rebuild

fn render_sysinfo_pill() {
    let cpu_class = if parse_int(PLUGIN_CPU) >= 80 { "sysinfo-cpu high" } else { "sysinfo-cpu" };

    #{
        type:         "box",
        class:        "sysinfo-pill",
        orientation:  "h",
        spacing:      "4",
        "space-evenly": "false",
        children: [
            #{
                type:  "label",
                class: cpu_class,
                text:  " " + PLUGIN_CPU,
            },
            #{
                type:  "label",
                class: "sysinfo-sep",
                text:  "│",
            },
            #{
                type:  "label",
                class: "sysinfo-ram",
                text:  " " + PLUGIN_RAM,
            },
        ]
    }
}
