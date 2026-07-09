use std::path::Path;

/// Get system-wide CPU jiffies from /proc/stat.
pub fn get_total_jiffies(proc_root: &Path) -> Option<u64> {
    let content = std::fs::read_to_string(proc_root.join("stat")).ok()?;
    let first_line = content.lines().next()?;
    let rest = first_line.strip_prefix("cpu ")?;

    let total: u64 = rest.split_whitespace().filter_map(|s| s.parse::<u64>().ok()).sum();
    Some(total)
}

/// Get the number of CPU cores.
pub fn get_num_cpus() -> usize {
    std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1)
}

/// Get the HZ value (jiffies per second) from the system.
/// Falls back to 100 (typical for most Linux systems).
pub fn get_hz() -> u64 {
    // Try to read from /proc/cmdline or use a safe approach
    // Most Linux systems use HZ=100 or HZ=300
    // We'll use 100 as the default which works for most cases
    100
}

/// Calculate CPU percentage from two samples.
/// Returns None if this is the first sample (delta is zero).
pub fn calculate_cpu_percent(
    utime1: u64,
    stime1: u64,
    total1: u64,
    utime2: u64,
    stime2: u64,
    total2: u64,
    num_cpus: usize,
) -> Option<f64> {
    let process_delta = (utime2 + stime2).saturating_sub(utime1 + stime1);
    let total_delta = total2.saturating_sub(total1);

    if total_delta == 0 {
        return None;
    }

    Some((process_delta as f64 / total_delta as f64) * 100.0 * num_cpus as f64)
}
