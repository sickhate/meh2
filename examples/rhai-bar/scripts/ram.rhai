// RAM usage percentage — reads /proc/meminfo directly, no subprocess.
// Uses split+parse_int instead of replace()/trim() (Rhai: in-place, return ()).

let raw      = read_file("/proc/meminfo");
let total_kb = 0;
let avail_kb = 0;

for line in raw.split("\n") {
    if line.starts_with("MemTotal:") {
        for tok in line.split(" ") {
            let n = parse_int(tok);
            if n > 0 { total_kb = n; break; }
        }
    }
    if line.starts_with("MemAvailable:") {
        for tok in line.split(" ") {
            let n = parse_int(tok);
            if n > 0 { avail_kb = n; break; }
        }
    }
}

let used_kb = total_kb - avail_kb;
let pct = if total_kb > 0 { (used_kb * 100) / total_kb } else { 0 };
`${pct}`
