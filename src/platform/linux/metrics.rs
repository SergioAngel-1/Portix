use std::path::Path;

/// Get system-wide CPU jiffies from /proc/stat.
pub fn get_total_jiffies(proc_root: &Path) -> Option<u64> {
    let content = std::fs::read_to_string(proc_root.join("stat")).ok()?;
    let first_line = content.lines().next()?;
    let rest = first_line.strip_prefix("cpu ")?;

    // Sum only the first eight counters (user nice system idle iowait irq
    // softirq steal). The ninth and tenth columns (guest, guest_nice) are
    // already included in user/nice, so summing them again double-counts VM
    // time and understates every process's CPU share while guests run.
    let total: u64 = rest
        .split_whitespace()
        .take(8)
        .filter_map(|s| s.parse::<u64>().ok())
        .sum();
    Some(total)
}

/// Get the number of CPU cores.
pub fn get_num_cpus() -> usize {
    std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1)
}

/// Jiffies per second, as the kernel reports them to userspace.
///
/// `/proc/<pid>/stat` expresses utime/stime/starttime in these units. Hardcoding
/// 100 is right on most builds and silently wrong on the rest. `sysconf` is
/// constant for the lifetime of the kernel, so it is queried once and memoized.
pub fn clock_ticks_per_sec() -> u64 {
    static HZ: std::sync::OnceLock<u64> = std::sync::OnceLock::new();
    *HZ.get_or_init(|| {
        #[allow(unsafe_code)]
        let hz = unsafe { libc::sysconf(libc::_SC_CLK_TCK) };
        if hz > 0 {
            hz as u64
        } else {
            100
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// guest/guest_nice (columns 9-10 of the cpu line) are already included
    /// in user/nice; the total must not count them twice, or every process's
    /// CPU share gets understated while VMs run.
    #[test]
    fn test_total_jiffies_excludes_guest_double_count() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("stat"),
            "cpu  100 200 300 400 500 600 700 800 900 1000\n",
        )
        .unwrap();
        // 100+200+300+400+500+600+700+800 = 3600; the guest columns (900,
        // 1000) must not inflate the total.
        assert_eq!(get_total_jiffies(dir.path()), Some(3600));
    }
}
