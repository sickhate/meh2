// GPL-3.0-or-later
// rhai-widget demo — shows map-based widget tree construction.
//
// Usage in yuck:
//   (rhai-widget :src "main.rhai" :fn "status_bar" :watch "CPU MEM")
//
// The function receives no arguments. Read vars via read_file() or run_shell()
// inside the function; `:watch` tells the daemon which vars trigger a rebuild.

// Returns a horizontal box with CPU and memory labels side by side.
// CPU and MEM are injected from :watch vars — no cache file reads needed.
fn status_bar() {
    // CPU and MEM are injected into scope from the :watch attribute.
    // Fall back to "?" if somehow missing.
    let cpu = if CPU == "" { "?" } else { CPU };
    let mem = if MEM == "" { "?" } else { MEM };

    #{
        type: "box",
        orientation: "h",
        spacing: "12",
        class: "rhai-status",
        children: [
            #{
                type: "label",
                text: "󰻠 " + cpu + "%",
                class: "rhai-cpu"
            },
            #{
                type: "label",
                text: "󰍛 " + mem + " MB",
                class: "rhai-mem"
            }
        ]
    }
}

// Returns a single label — minimal example.
fn greeting() #{
    type: "label",
    text: "Hello from Rhai!",
    class: "rhai-greeting"
}

// Dynamic widget: changes structure based on a condition.
fn battery_widget() {
    let cap_raw = read_file("/sys/class/power_supply/BAT0/capacity");
    let status   = read_file("/sys/class/power_supply/BAT0/status");

    if cap_raw == "" {
        return #{
            type: "label",
            text: "no battery",
            class: "rhai-bat-absent"
        };
    }

    let cap = parse_int(cap_raw);
    let icon = if status == "Charging" {
        "󰂄"
    } else if cap > 80 {
        "󰁹"
    } else if cap > 50 {
        "󰁾"
    } else if cap > 20 {
        "󰁼"
    } else {
        "󰁺"
    };

    let cls = if cap <= 20 && status != "Charging" { "rhai-bat-low" } else { "rhai-bat" };

    #{
        type: "label",
        text: icon + " " + cap + "%",
        class: cls
    }
}
