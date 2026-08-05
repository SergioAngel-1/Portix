use std::path::Path;

/// Get system-wide CPU jiffies from /proc/stat.
pub fn get_total_jiffies(proc_root: &Path) -> Option<u64> {
    let content = std::fs::read_to_string(proc_root.join("stat")).ok()?;
    let first_line = content.lines().next()?;
    let rest = first_line.strip_prefix("cpu ")?;

    let total: u64 = rest
        .split_whitespace()
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

/// Calculate CPU percentage from two samples of (process jiffies, system jiffies).
///
/// `num_cpus` scales the result to the "core" convention (100% per core, like
/// top); pass 1 for the "system" convention (100% = the whole machine).
/// Returns None when the two samples are identical — there is no delta to
/// divide by, which is exactly the state on the very first scan.
pub fn calculate_cpu_percent(
    proc_jiffies_before: u64,
    total_jiffies_before: u64,
    proc_jiffies_after: u64,
    total_jiffies_after: u64,
    num_cpus: usize,
) -> Option<f64> {
    let process_delta = proc_jiffies_after.saturating_sub(proc_jiffies_before);
    let total_delta = total_jiffies_after.saturating_sub(total_jiffies_before);

    if total_delta == 0 {
        return None;
    }

    Some((process_delta as f64 / total_delta as f64) * 100.0 * num_cpus as f64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_first_sample_has_no_delta() {
        assert_eq!(calculate_cpu_percent(500, 10_000, 500, 10_000, 8), None);
    }

    #[test]
    fn test_fully_busy_single_core_on_eight_cores() {
        // Process burned 100 jiffies while the system burned 800 across 8 cores:
        // exactly one core saturated => 100% in the "core" scale.
        let pct = calculate_cpu_percent(0, 0, 100, 800, 8).unwrap();
        assert!((pct - 100.0).abs() < 0.001, "got {pct}");
    }

    #[test]
    fn test_counter_reset_does_not_underflow() {
        // PID reuse can make the new sample smaller than the old one.
        assert_eq!(calculate_cpu_percent(900, 10_000, 5, 10_800, 1), Some(0.0));
    }
}
