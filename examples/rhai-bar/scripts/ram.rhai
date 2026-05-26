// RAM usage from /proc/meminfo.
// Returns a JSON object: {"used_mb": 1234, "total_mb": 16000, "pct": 7}

let raw = read_file("/proc/meminfo");
let total_kb = 0;
let avail_kb = 0;

for line in raw.split('\n') {
    if line.starts_with("MemTotal:") {
        let parts = line.split_whitespace();
        total_kb = parse_int(parts[1]);
    }
    if line.starts_with("MemAvailable:") {
        let parts = line.split_whitespace();
        avail_kb = parse_int(parts[1]);
    }
}

let used_kb  = total_kb - avail_kb;
let used_mb  = used_kb  / 1024;
let total_mb = total_kb / 1024;
let pct      = if total_kb > 0 { (used_kb * 100) / total_kb } else { 0 };

`{"used_mb": ${used_mb}, "total_mb": ${total_mb}, "pct": ${pct}}`
