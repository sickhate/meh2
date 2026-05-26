// RAM usage percentage — reads /proc/meminfo directly, no subprocess.

let raw = read_file("/proc/meminfo");
let total_kb = 0;
let avail_kb = 0;

for line in raw.split("\n") {
    let trimmed = line.trim();
    if trimmed.starts_with("MemTotal:") {
        let val = trimmed.replace("MemTotal:", "").trim().split(" ")[0].trim();
        total_kb = parse_int(val);
    }
    if trimmed.starts_with("MemAvailable:") {
        let val = trimmed.replace("MemAvailable:", "").trim().split(" ")[0].trim();
        avail_kb = parse_int(val);
    }
}

let used_kb = total_kb - avail_kb;
let pct = if total_kb > 0 { (used_kb * 100) / total_kb } else { 0 };
`${pct}`
