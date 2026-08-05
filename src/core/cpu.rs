//! CPU accounting helpers — pure math, no I/O.
//!
//! Lives in `core`, not `platform`, because it is pure arithmetic: the
//! platform layer's job is reading counters from the OS, and this function
//! only combines two samples that were already read. Keeping it here is what
//! lets `core` honour the layering rule ("never reaches into
//! `platform::linux::*` directly") while still scaling CPU percentages.

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
