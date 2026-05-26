// CPU usage percentage from /proc/stat.
// Returns a string like "42" (integer percent, 0–100).

let stat1 = read_file("/proc/stat");
let line1 = stat1.split('\n')[0];
let parts1 = line1.split(' ');

// /proc/stat cpu line: "cpu  user nice system idle iowait irq softirq steal"
// parts[0] = "cpu", then one or two spaces before user — filter empties
let nums1 = [];
for p in parts1 {
    if p != "" && p != "cpu" {
        nums1 += p;
    }
}

let user1    = parse_int(nums1[0]);
let nice1    = parse_int(nums1[1]);
let system1  = parse_int(nums1[2]);
let idle1    = parse_int(nums1[3]);
let iowait1  = parse_int(nums1[4]);

let total1   = user1 + nice1 + system1 + idle1 + iowait1;
let active1  = total1 - idle1 - iowait1;

// Second sample after 200ms
run_shell("sleep 0.2");

let stat2 = read_file("/proc/stat");
let line2 = stat2.split('\n')[0];
let parts2 = line2.split(' ');

let nums2 = [];
for p in parts2 {
    if p != "" && p != "cpu" {
        nums2 += p;
    }
}

let user2    = parse_int(nums2[0]);
let nice2    = parse_int(nums2[1]);
let system2  = parse_int(nums2[2]);
let idle2    = parse_int(nums2[3]);
let iowait2  = parse_int(nums2[4]);

let total2   = user2 + nice2 + system2 + idle2 + iowait2;
let active2  = total2 - idle2 - iowait2;

let dtotal  = total2  - total1;
let dactive = active2 - active1;

if dtotal == 0 {
    "0"
} else {
    let pct = (dactive * 100) / dtotal;
    `${pct}`
}
