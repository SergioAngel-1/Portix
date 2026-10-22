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
    // The error is typed so the engine can tell this normal race apart from a
    // permission/format failure without matching on a message.
    let stat = match read_stat(proc_root, &pid_dir) {
        Some(stat) => stat,
        None => return Err(anyhow::Error::new(crate::platform::ProcessGone { pid })),
    };

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
///
/// `State:` and `PPid:` are deliberately not parsed: `/proc/<pid>/stat`
/// already provides both, and the copies here were never read.
fn read_status(pid_dir: &Path) -> Option<StatusInfo> {
    let content = std::fs::read_to_string(pid_dir.join("status")).ok()?;
    let mut info = StatusInfo::default();

    for line in content.lines() {
        if let Some(value) = line.strip_prefix("Name:") {
            info.name = value.trim().to_string();
        } else if let Some(_value) = line.strip_prefix("Pid:") {
            // We already know the PID
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

#[derive(Default)]
struct StatusInfo {
    name: String,
    rss_bytes: Option<u64>,
    virt_bytes: Option<u64>,
    uid: Option<u32>,
    threads: u32,
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

/// Effective UID of the current process.
///
/// `/proc/<pid>/fd` is `dr-x------`: a socket owned by another UID can never be
/// resolved, no matter how many times the engine retries. Knowing our own UID
/// lets the engine mark those inodes as permanently unresolved instead of
/// paying a full fd-table walk for them on every refresh.
pub fn effective_uid() -> u32 {
    // SAFETY: `geteuid` has no preconditions, takes no pointers, and cannot
    // fail. It only reads the kernel's cached credential for this process.
    #[allow(unsafe_code)]
    unsafe {
        libc::geteuid()
    }
}

/// Ceiling for the growable passwd-lookup buffer.
///
/// `ERANGE` means the entry did not fit; doubling from the `sysconf` hint
/// clears any real NIS/LDAP entry long before this. The cap keeps a bogus
/// `_SC_GETPW_R_SIZE_MAX` (or a pathological NSS module) from allocating
/// without bound on a hot path.
const MAX_PASSWD_BUF: usize = 1024 * 1024;

/// Initial buffer size for a `getpw*_r` lookup.
///
/// `sysconf(_SC_GETPW_R_SIZE_MAX)` is a hint, not a guarantee: it can return
/// -1 (indeterminate) or a value still too small for the entry, which is why
/// the callers grow on `ERANGE`.
fn initial_passwd_buf_size() -> usize {
    // SAFETY: `sysconf` takes a constant selector, has no pointer arguments,
    // and only reads kernel configuration for this process.
    #[allow(unsafe_code)]
    let hint = unsafe { libc::sysconf(libc::_SC_GETPW_R_SIZE_MAX) };
    if hint > 0 {
        (hint as usize).min(MAX_PASSWD_BUF)
    } else {
        1024
    }
}

/// Run one `getpw*_r` lookup with a growable buffer.
///
/// Both lookups share the same contract and the same traps: the callee fills
/// the `passwd` in place, and every string field points *into* the caller's
/// buffer, so the buffer must outlive `extract`. On `ERANGE` the entry did not
/// fit — a fixed 4 KiB buffer used to give up and hide the user entirely,
/// which on NIS/LDAP boxes means a blank USER column — so grow up to 1 MiB
/// before returning `None`.
///
/// The lookup is a closure so `getpwuid_r` and `getpwnam_r` share this
/// buffer/`MaybeUninit` handling instead of each carrying its own copy of the
/// unsafe dance.
fn with_passwd<T>(
    lookup: impl Fn(*mut libc::passwd, *mut libc::c_char, usize, *mut *mut libc::passwd) -> libc::c_int,
    extract: impl FnOnce(&libc::passwd) -> Option<T>,
) -> Option<T> {
    let mut size = initial_passwd_buf_size();
    loop {
        let mut buf = vec![0u8; size];
        let mut pwd = std::mem::MaybeUninit::<libc::passwd>::zeroed();
        let mut result: *mut libc::passwd = std::ptr::null_mut();

        // `c_char` is `i8` on x86_64 but `u8` on aarch64/ppc64le/riscv; the
        // old `as *mut i8` cast did not even compile on those targets. `cast`
        // lets the compiler pick the right pointer type. The closure, not this
        // loop, owns the `unsafe` call; it receives a live `size`-byte buffer
        // and an out-parameter pointing at `pwd`.
        let rc = lookup(
            pwd.as_mut_ptr(),
            buf.as_mut_ptr().cast::<libc::c_char>(),
            size,
            &mut result,
        );

        if rc == libc::ERANGE {
            if size >= MAX_PASSWD_BUF {
                return None;
            }
            size = (size * 2).min(MAX_PASSWD_BUF);
            continue;
        }
        if rc != 0 || result.is_null() {
            return None;
        }

        // SAFETY: a zero return with a non-null `result` means the lookup
        // filled the struct it points at, and that every string field points
        // into `buf`, which is still alive on this iteration.
        #[allow(unsafe_code)]
        let pw = unsafe { &*result };
        return extract(pw);
    }
}

/// Resolve a numeric UID to a username via the POSIX `getpwuid_r` API.
///
/// `pub(crate)`: the engine calls this through the L4 cache
/// (`CacheStore::get_user`/`insert_user`); platform exposes the primitive,
/// core decides how often it runs.
pub(crate) fn uid_to_username(uid: u32) -> Option<String> {
    with_passwd(
        |pwd, buf, len, result| {
            // SAFETY: the helper passes a live buffer of `len` bytes, a valid
            // `passwd` slot, and a valid out-pointer.
            #[allow(unsafe_code)]
            unsafe {
                libc::getpwuid_r(uid, pwd, buf, len, result)
            }
        },
        |pw| {
            if pw.pw_name.is_null() {
                return None;
            }
            // SAFETY: `pw_name` is NUL-terminated and points into the lookup
            // buffer, which outlives this closure.
            #[allow(unsafe_code)]
            let name = unsafe { CStr::from_ptr(pw.pw_name) };
            name.to_str().ok().map(|s| s.to_string())
        },
    )
}

/// Home directory of another user, resolved via the POSIX `getpwnam_r` API.
///
/// Used by config loading under `sudo portix` (PRD §13.3): sudo resets
/// `$HOME` to the root user's, so the invoking user's config would otherwise
/// be ignored — and a theme save would land in `/root/.config`. `platform`
/// exposes the primitive; `core` decides when to consult it.
pub(crate) fn home_dir_for_user(name: &str) -> Option<PathBuf> {
    let c_name = std::ffi::CString::new(name).ok()?;
    with_passwd(
        |pwd, buf, len, result| {
            // SAFETY: `c_name` is a live NUL-terminated string, and the helper
            // passes a live buffer of `len` bytes, a valid `passwd` slot, and
            // a valid out-pointer.
            #[allow(unsafe_code)]
            unsafe {
                libc::getpwnam_r(c_name.as_ptr(), pwd, buf, len, result)
            }
        },
        |pw| {
            if pw.pw_dir.is_null() {
                return None;
            }
            // SAFETY: `pw_dir` is NUL-terminated and points into the lookup
            // buffer, which outlives this closure.
            #[allow(unsafe_code)]
            let dir = unsafe { CStr::from_ptr(pw.pw_dir) };
            dir.to_str().ok().map(PathBuf::from)
        },
    )
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

    /// The engine classifies a vanished owner by downcasting, so the error must
    /// stay a typed `ProcessGone` — never just a formatted message.
    #[test]
    fn test_missing_process_reports_process_gone() {
        let dir = fake_proc(12345, HOSTILE_COMM_STAT);
        let err = process_metadata(dir.path(), 999).unwrap_err();
        let gone = err
            .downcast_ref::<crate::platform::ProcessGone>()
            .expect("a vanished pid must surface as ProcessGone");
        assert_eq!(gone.pid, 999);
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

    /// UID 0 is `root` on every system that has a passwd database at all: the
    /// shared lookup must return the name, not just the home directory.
    #[test]
    fn test_uid_to_username_resolves_root() {
        assert_eq!(uid_to_username(0), Some("root".to_string()));
    }
}
