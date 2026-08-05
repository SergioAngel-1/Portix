use crate::model::{Process, ProcessMetrics, ProcessState};
use anyhow::{Context, Result};
use std::ffi::CStr;
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

    // A process can vanish between readdir and open. Its stat file is the one
    // read we cannot degrade past: without starttime there is no safe identity.
    let stat = read_stat(proc_root, &pid_dir).context("process disappeared")?;

    let cmdline = read_cmdline(&pid_dir).unwrap_or_default();
    let exe = read_link_safe(&pid_dir.join("exe")).ok();
    let cwd = read_link_safe(&pid_dir.join("cwd")).ok().map(|p| {
        if p.is_absolute() {
            p
        } else {
            pid_dir.join(&p)
        }
    });
    let status = read_status(&pid_dir).unwrap_or_default();
    // Username resolution is deliberately left to the engine: it is an NSS
    // call (`getpwuid_r`) and goes through the L4 cache so LDAP/SSSD boxes do
    // not pay one lookup per process per scan. The uid rides along on the
    // Process for the engine to resolve.
    let user = None;

    Ok(Process {
        pid,
        ppid: stat.ppid,
        name: status.name,
        cmdline,
        exe,
        cwd,
        uid: status.uid,
        user,
        started_at: stat.started_at,
        starttime_jiffies: stat.starttime_jiffies,
        cpu_jiffies: stat.utime + stat.stime,
        metrics: ProcessMetrics {
            cpu_percent: None, // Filled in by the engine from two samples
            rss_bytes: status.rss_bytes,
            virt_bytes: status.virt_bytes,
            uptime: None,
        },
        state: stat.state,
        threads: status.threads,
    })
}

/// Read only the starttime of a process, for identity re-verification.
///
/// Returns `None` if the process is gone. Cheap enough to call right before
/// sending a signal.
pub fn read_starttime_jiffies(proc_root: &Path, pid: i32) -> Option<u64> {
    let pid_dir = proc_root.join(pid.to_string());
    read_stat(proc_root, &pid_dir).map(|s| s.starttime_jiffies)
}

/// Read the UID of the calling process, honoring the injected proc root.
pub fn current_uid(proc_root: &Path) -> Option<u32> {
    let content = std::fs::read_to_string(proc_root.join("self").join("status")).ok()?;
    content
        .lines()
        .find_map(|l| l.strip_prefix("Uid:"))
        .and_then(|v| v.split_whitespace().next())
        .and_then(|s| s.parse().ok())
}

/// Real UID of the calling process via the `getuid(2)` syscall.
///
/// The fallback for when the injected proc root has no readable `self/status`
/// (the normal case in tests). The `unsafe` lives here, in `platform::linux`,
/// the only layer permitted to touch raw syscalls.
pub fn real_uid() -> u32 {
    #[allow(unsafe_code)]
    unsafe {
        libc::getuid()
    }
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
        } else if let Some(_value) = line.strip_prefix("Pid:") {
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
        } else if let Some(value) = line.strip_prefix("Uid:") {
            // Format: "Uid:\t1000\t1000\t1000\t1000"
            if let Some(first) = value.split_whitespace().next() {
                info.uid = first.parse::<u32>().ok();
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
    uid: Option<u32>,
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
            uid: None,
            threads: 0,
        }
    }
}

/// Parse /proc/<pid>/stat.
/// IMPORTANT: The comm field (field 2) can contain spaces and parentheses.
/// We must parse from the LAST ')' to handle malicious comm values.
fn read_stat(proc_root: &Path, pid_dir: &Path) -> Option<StatInfo> {
    let content = std::fs::read_to_string(pid_dir.join("stat")).ok()?;

    // Find the last ')' to handle comm fields with spaces/parens
    let last_paren = content.rfind(')')?;
    let after_comm = content.get(last_paren + 2..)?; // Skip ") "

    let fields: Vec<&str> = after_comm.split_whitespace().collect();
    if fields.len() < 20 {
        return None;
    }

    // Indices are relative to the field after comm: state=0, ppid=1,
    // utime=11, stime=12, starttime=19.
    let state = fields[0].chars().next().and_then(ProcessState::from_char)?;
    let ppid = fields[1].parse::<i32>().ok()?;
    let utime = fields[11].parse::<u64>().ok()?;
    let stime = fields[12].parse::<u64>().ok()?;
    let starttime_jiffies = fields[19].parse::<u64>().ok()?;

    let hz = super::metrics::clock_ticks_per_sec();
    let btime = boot_time_secs(proc_root).unwrap_or(0);
    let started_at = if btime == 0 {
        None
    } else {
        Some(std::time::UNIX_EPOCH + std::time::Duration::from_secs(btime + starttime_jiffies / hz))
    };

    Some(StatInfo {
        ppid,
        state,
        started_at,
        starttime_jiffies,
        utime,
        stime,
    })
}

/// Boot time in epoch seconds, from the `btime` line of <proc_root>/stat.
///
/// `btime` is constant until reboot, but `read_stat` runs once per process per
/// scan — reading the whole `/proc/stat` every time is hundreds of extra file
/// reads with no information gain. Cached per proc root (single entry: one
/// scan uses one root, and tests use unique temp dirs, so entries never
/// collide).
fn boot_time_secs(proc_root: &Path) -> Option<u64> {
    use std::sync::Mutex;
    static CACHE: Mutex<Option<(PathBuf, Option<u64>)>> = Mutex::new(None);
    let mut cache = CACHE.lock().unwrap_or_else(|p| p.into_inner());
    if let Some((root, btime)) = cache.as_ref() {
        if root == proc_root {
            return *btime;
        }
    }
    let btime = read_btime(proc_root);
    *cache = Some((proc_root.to_path_buf(), btime));
    btime
}

/// Read the `btime` line of <proc_root>/stat, uncached.
fn read_btime(proc_root: &Path) -> Option<u64> {
    let content = std::fs::read_to_string(proc_root.join("stat")).ok()?;
    content
        .lines()
        .find(|l| l.starts_with("btime"))
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|s| s.parse().ok())
}

struct StatInfo {
    ppid: i32,
    state: ProcessState,
    started_at: Option<std::time::SystemTime>,
    starttime_jiffies: u64,
    utime: u64,
    stime: u64,
}

/// Resolve a numeric UID to a username via the POSIX `getpwuid_r` API.
///
/// `pub(crate)`: the engine calls this through the L4 cache
/// (`CacheStore::get_user`/`insert_user`); platform exposes the primitive,
/// core decides how often it runs.
///
/// The buffer is sized at 4 KiB, which covers every username in active use
/// (Linux caps user names at 32 bytes; even NIS/LDAP entries fit comfortably).
/// A too-small buffer returns None and the caller degrades gracefully.
pub(crate) fn uid_to_username(uid: u32) -> Option<String> {
    let buf_size = 4096;
    let mut buf = vec![0u8; buf_size];
    let mut pwd = std::mem::MaybeUninit::<libc::passwd>::zeroed();
    let mut result: *mut libc::passwd = std::ptr::null_mut();

    #[allow(unsafe_code)]
    let rc = unsafe {
        libc::getpwuid_r(
            uid,
            pwd.as_mut_ptr(),
            buf.as_mut_ptr() as *mut i8,
            buf_size,
            &mut result,
        )
    };

    if rc != 0 || result.is_null() {
        return None;
    }

    #[allow(unsafe_code)]
    let pw = unsafe { &*result };

    if pw.pw_name.is_null() {
        return None;
    }

    #[allow(unsafe_code)]
    let name = unsafe { CStr::from_ptr(pw.pw_name) };
    name.to_str().ok().map(|s| s.to_string())
}

/// Home directory of another user, resolved via the POSIX `getpwnam_r` API.
///
/// Used by config loading under `sudo portix` (PRD §13.3): sudo resets
/// `$HOME` to the root user's, so the invoking user's config would otherwise
/// be ignored — and a theme save would land in `/root/.config`. `platform`
/// exposes the primitive; `core` decides when to consult it.
pub(crate) fn home_dir_for_user(name: &str) -> Option<PathBuf> {
    let buf_size = 4096;
    let mut buf = vec![0u8; buf_size];
    let mut pwd = std::mem::MaybeUninit::<libc::passwd>::zeroed();
    let mut result: *mut libc::passwd = std::ptr::null_mut();
    let c_name = std::ffi::CString::new(name).ok()?;

    #[allow(unsafe_code)]
    let rc = unsafe {
        libc::getpwnam_r(
            c_name.as_ptr(),
            pwd.as_mut_ptr(),
            buf.as_mut_ptr() as *mut i8,
            buf_size,
            &mut result,
        )
    };

    if rc != 0 || result.is_null() {
        return None;
    }

    #[allow(unsafe_code)]
    let pw = unsafe { &*result };

    if pw.pw_dir.is_null() {
        return None;
    }

    #[allow(unsafe_code)]
    let dir = unsafe { CStr::from_ptr(pw.pw_dir) };
    dir.to_str().ok().map(PathBuf::from)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a fake proc root containing one pid with the given stat content.
    fn fake_proc(pid: i32, stat: &str) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("stat"), "cpu  1 2 3 4\nbtime 1700000000\n").unwrap();
        let pid_dir = dir.path().join(pid.to_string());
        std::fs::create_dir(&pid_dir).unwrap();
        std::fs::write(pid_dir.join("stat"), stat).unwrap();
        dir
    }

    /// comm is attacker-controlled: a process named "(evil) 1 2 3" must not
    /// shift every subsequent field. Parse from the LAST ')'.
    const HOSTILE_COMM_STAT: &str = "12345 ((evil) 1 2 3) S 12340 12345 12345 34816 12345 0 0 0 0 0 111 222 0 0 20 0 1 0 987654 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0";

    #[test]
    fn test_parse_stat_with_parens() {
        let dir = fake_proc(12345, HOSTILE_COMM_STAT);
        let stat = read_stat(dir.path(), &dir.path().join("12345")).unwrap();
        assert_eq!(stat.ppid, 12340);
        assert_eq!(stat.state, ProcessState::Sleeping);
    }

    #[test]
    fn test_parse_stat_reads_cpu_times_and_starttime() {
        let dir = fake_proc(12345, HOSTILE_COMM_STAT);
        let stat = read_stat(dir.path(), &dir.path().join("12345")).unwrap();
        assert_eq!(stat.utime, 111);
        assert_eq!(stat.stime, 222);
        assert_eq!(stat.starttime_jiffies, 987654);
    }

    #[test]
    fn test_boot_time_comes_from_injected_proc_root() {
        // A hermetic proc root must never fall through to the real /proc.
        let dir = fake_proc(12345, HOSTILE_COMM_STAT);
        assert_eq!(boot_time_secs(dir.path()), Some(1_700_000_000));

        let empty = tempfile::tempdir().unwrap();
        assert_eq!(boot_time_secs(empty.path()), None);
    }

    #[test]
    fn test_read_starttime_of_missing_process_is_none() {
        let dir = fake_proc(12345, HOSTILE_COMM_STAT);
        assert_eq!(read_starttime_jiffies(dir.path(), 12345), Some(987654));
        assert_eq!(read_starttime_jiffies(dir.path(), 999), None);
    }

    /// `root` always exists with a home; a made-up user must not resolve.
    /// These are local passwd lookups, so no LDAP/NIS latency in tests.
    #[test]
    fn test_home_dir_for_user_resolves_real_users_only() {
        assert!(
            home_dir_for_user("root").is_some(),
            "root must resolve to a home directory"
        );
        assert_eq!(home_dir_for_user("no-such-user-portix-test"), None);
    }
}
