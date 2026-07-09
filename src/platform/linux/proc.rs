use crate::model::{Process, ProcessMetrics, ProcessState};
use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

/// List all PIDs visible to the current user.
pub fn list_pids(proc_root: &Path) -> Result<Vec<i32>> {
    let mut pids = Vec::new();
    for entry in std::fs::read_dir(proc_root)? {
        let entry = entry?;
        let name = entry.file_name();
        if let Some(name_str) = name.to_str() {
            if let Ok(pid) = name_str.parse::<i32>() {
                pids.push(pid);
            }
        }
    }
    Ok(pids)
}

/// Get metadata for a specific process.
pub fn process_metadata(proc_root: &Path, pid: i32) -> Result<Process> {
    let pid_dir = proc_root.join(pid.to_string());

    // Read cmdline
    let cmdline = read_cmdline(&pid_dir).unwrap_or_default();

    // Read exe (readlink)
    let exe = read_link_safe(&pid_dir.join("exe")).ok();

    // Read cwd (readlink)
    let cwd = read_link_safe(&pid_dir.join("cwd")).ok();

    // Read status
    let status = read_status(&pid_dir).unwrap_or_default();

    // Read stat
    let stat = read_stat(&pid_dir).unwrap_or_default();

    // Read cgroup
    let _cgroup = read_cgroup(&pid_dir).unwrap_or_default();

    Ok(Process {
        pid,
        ppid: stat.ppid,
        name: status.name,
        cmdline,
        exe,
        cwd,
        user: None, // TODO: Resolve uid to username
        started_at: stat.started_at,
        metrics: ProcessMetrics {
            cpu_percent: None, // Requires delta calculation
            rss_bytes: status.rss_bytes,
            virt_bytes: status.virt_bytes,
            uptime: None,
        },
        state: stat.state,
        threads: status.threads,
    })
}

/// Read /proc/<pid>/cmdline (null-separated).
fn read_cmdline(pid_dir: &Path) -> Option<Vec<String>> {
    let bytes = std::fs::read(pid_dir.join("cmdline")).ok()?;
    if bytes.is_empty() {
        return None;
    }

    // Split by null bytes
    let args: Vec<String> = bytes
        .split(|&b| b == 0)
        .filter(|b| !b.is_empty())
        .map(|b| String::from_utf8_lossy(b).to_string())
        .collect();

    if args.is_empty() {
        None
    } else {
        Some(args)
    }
}

/// Read a symlink safely (handles deleted targets).
fn read_link_safe(path: &Path) -> Result<PathBuf> {
    let target = std::fs::read_link(path)
        .with_context(|| format!("Failed to read link: {}", path.display()))?;
    Ok(target)
}

/// Parse /proc/<pid>/status.
fn read_status(pid_dir: &Path) -> Option<StatusInfo> {
    let content = std::fs::read_to_string(pid_dir.join("status")).ok()?;
    let mut info = StatusInfo::default();

    for line in content.lines() {
        if let Some(value) = line.strip_prefix("Name:") {
            info.name = value.trim().to_string();
        } else if let Some(value) = line.strip_prefix("State:") {
            // State line looks like: "State:  S (sleeping)"
            if let Some(state_char) = value.trim().chars().next() {
                info.state = ProcessState::from_char(state_char).unwrap_or(ProcessState::Sleeping);
            }
        } else if let Some(value) = line.strip_prefix("Pid:") {
            // We already know the PID
        } else if let Some(value) = line.strip_prefix("PPid:") {
            if let Ok(ppid) = value.trim().parse::<i32>() {
                info.ppid = ppid;
            }
        } else if let Some(value) = line.strip_prefix("VmRSS:") {
            // Format: "VmRSS:    12345 kB"
            if let Some(kb_str) = value.trim().strip_suffix(" kB") {
                if let Ok(kb) = kb_str.trim().parse::<u64>() {
                    info.rss_bytes = Some(kb * 1024);
                }
            }
        } else if let Some(value) = line.strip_prefix("VmSize:") {
            if let Some(kb_str) = value.trim().strip_suffix(" kB") {
                if let Ok(kb) = kb_str.trim().parse::<u64>() {
                    info.virt_bytes = Some(kb * 1024);
                }
            }
        } else if let Some(value) = line.strip_prefix("Threads:") {
            if let Ok(threads) = value.trim().parse::<u32>() {
                info.threads = threads;
            }
        }
    }

    Some(info)
}

struct StatusInfo {
    name: String,
    ppid: i32,
    state: ProcessState,
    rss_bytes: Option<u64>,
    virt_bytes: Option<u64>,
    threads: u32,
}

impl Default for StatusInfo {
    fn default() -> Self {
        Self {
            name: String::new(),
            ppid: 0,
            state: ProcessState::Sleeping,
            rss_bytes: None,
            virt_bytes: None,
            threads: 0,
        }
    }
}

/// Parse /proc/<pid>/stat.
/// IMPORTANT: The comm field (field 2) can contain spaces and parentheses.
/// We must parse from the LAST ')' to handle malicious comm values.
fn read_stat(pid_dir: &Path) -> Option<StatInfo> {
    let content = std::fs::read_to_string(pid_dir.join("stat")).ok()?;

    // Find the last ')' to handle comm fields with spaces/parens
    let last_paren = content.rfind(')')?;
    let after_comm = &content[last_paren + 2..]; // Skip ") "

    let fields: Vec<&str> = after_comm.split_whitespace().collect();
    if fields.len() < 20 {
        return None;
    }

    // Field 0 (after comm): state (index 0)
    let state = fields[0].chars().next().and_then(ProcessState::from_char)?;

    // Field 1: ppid (index 1)
    let ppid = fields[1].parse::<i32>().ok()?;

    // Field 19 (index 19): starttime (jiffies since boot)
    let starttime = fields[19].parse::<u64>().ok();

    // Get boot time from /proc/stat
    let btime = std::fs::read_to_string("/proc/stat")
        .ok()
        .and_then(|s| {
            s.lines()
                .find(|l| l.starts_with("btime"))
                .and_then(|l| l.split_whitespace().nth(1))
                .and_then(|s| s.parse::<u64>().ok())
        })
        .unwrap_or(0);

    Some(StatInfo {
        ppid,
        state,
        started_at: starttime.map(|st| {
            // Convert jiffies to SystemTime
            // starttime is jiffies since boot, btime is epoch seconds of boot
            let jiffies_per_sec = 100u64;
            let process_start_secs = st / jiffies_per_sec;
            std::time::UNIX_EPOCH + std::time::Duration::from_secs(btime + process_start_secs)
        }),
    })
}

struct StatInfo {
    ppid: i32,
    state: ProcessState,
    started_at: Option<std::time::SystemTime>,
}

impl Default for StatInfo {
    fn default() -> Self {
        Self {
            ppid: 0,
            state: ProcessState::Sleeping,
            started_at: None,
        }
    }
}

/// Read /proc/<pid>/cgroup.
fn read_cgroup(pid_dir: &Path) -> Option<String> {
    std::fs::read_to_string(pid_dir.join("cgroup")).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_stat_with_parens() {
        // Process with parentheses in name
        let content = "12345 ((evil) 1 2 3) S 12340 12345 12345 34816 12345 0 0 0 0 0 0 0 0 0 20 0 1 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0";
        let temp_dir = tempfile::tempdir().unwrap();
        let pid_dir = temp_dir.path().join("12345");
        std::fs::create_dir(&pid_dir).unwrap();
        std::fs::write(pid_dir.join("stat"), content).unwrap();

        let stat = read_stat(&pid_dir).unwrap();
        assert_eq!(stat.ppid, 12340);
        assert_eq!(stat.state, ProcessState::Sleeping);
    }
}
