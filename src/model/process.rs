use serde::Serialize;
use std::path::PathBuf;
use std::time::SystemTime;

/// Process state from /proc/<pid>/stat.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum ProcessState {
    Running,
    Sleeping,
    DiskSleep,
    Zombie,
    Stopped,
    TracingStop,
    Dead,
}

impl ProcessState {
    pub fn from_char(c: char) -> Option<Self> {
        match c {
            'R' => Some(ProcessState::Running),
            'S' => Some(ProcessState::Sleeping),
            'D' => Some(ProcessState::DiskSleep),
            'Z' => Some(ProcessState::Zombie),
            'T' => Some(ProcessState::Stopped),
            't' => Some(ProcessState::TracingStop),
            'X' | 'x' => Some(ProcessState::Dead),
            _ => None,
        }
    }

    pub fn display(&self) -> &str {
        match self {
            ProcessState::Running => "Running",
            ProcessState::Sleeping => "Sleeping",
            ProcessState::DiskSleep => "Disk sleep",
            ProcessState::Zombie => "Zombie",
            ProcessState::Stopped => "Stopped",
            ProcessState::TracingStop => "Tracing stop",
            ProcessState::Dead => "Dead",
        }
    }
}

/// Process resource usage metrics.
#[derive(Debug, Clone, Default, Serialize)]
pub struct ProcessMetrics {
    /// CPU usage percentage (0.0 - 100.0+, can exceed 100% with multiple cores)
    pub cpu_percent: Option<f64>,
    /// Resident Set Size in bytes
    pub rss_bytes: Option<u64>,
    /// Virtual memory size in bytes
    pub virt_bytes: Option<u64>,
    /// Uptime of the process
    #[serde(rename = "uptime_secs", serialize_with = "duration_as_secs")]
    pub uptime: Option<std::time::Duration>,
}

fn duration_as_secs<S: serde::Serializer>(
    d: &Option<std::time::Duration>,
    s: S,
) -> Result<S::Ok, S::Error> {
    match d {
        Some(d) => s.serialize_some(&d.as_secs()),
        None => s.serialize_none(),
    }
}

fn path_as_str<S: serde::Serializer>(p: &Option<PathBuf>, s: S) -> Result<S::Ok, S::Error> {
    match p {
        Some(p) => s.serialize_some(&p.to_string_lossy()),
        None => s.serialize_none(),
    }
}

/// A process with its metadata.
#[derive(Debug, Clone, Serialize)]
pub struct Process {
    pub pid: i32,
    pub ppid: i32,
    pub name: String,
    pub cmdline: Vec<String>,
    #[serde(serialize_with = "path_as_str")]
    pub exe: Option<PathBuf>,
    #[serde(serialize_with = "path_as_str")]
    pub cwd: Option<PathBuf>,
    /// Numeric uid owning the process. `user` is its (cached) name; the engine
    /// resolves names through the L4 cache so LDAP/SSSD boxes do not pay one
    /// `getpwuid_r` per process per scan.
    pub uid: Option<u32>,
    pub user: Option<String>,
    #[serde(skip)]
    pub started_at: Option<SystemTime>,
    /// Start time in jiffies since boot. Together with the PID this is the only
    /// stable identity a process has: PIDs alone are recycled.
    #[serde(skip)]
    pub starttime_jiffies: u64,
    /// utime + stime, in jiffies. Meaningless alone; differenced across scans.
    #[serde(skip)]
    pub cpu_jiffies: u64,
    pub metrics: ProcessMetrics,
    pub state: ProcessState,
    pub threads: u32,
}

impl Process {
    /// Get the command string for display.
    pub fn command_display(&self) -> String {
        if self.cmdline.is_empty() {
            return self.name.clone();
        }
        self.cmdline.join(" ")
    }

    /// Best-effort name of the running binary.
    ///
    /// `cmdline[0]` lies in three different ways, all of them common:
    ///
    /// - Anything that re-execs itself reports `/proc/self/exe`.
    /// - Chrome and Electron pack their whole command line into a single argv[0]
    ///   blob, so the last path component is a trailing flag, not a program.
    /// - gunicorn rewrites argv outright.
    ///
    /// `comm` is the kernel's own answer and cannot be spoofed by argv games, so
    /// it wins whenever cmdline[0] resolves to something implausible. It is
    /// truncated to 15 bytes, which is why it is the fallback and not the
    /// primary source.
    pub fn binary_name(&self) -> &str {
        let from_cmdline = self
            .cmdline
            .first()
            // A program path never contains a space; a packed argv blob always does.
            .and_then(|s| s.split_whitespace().next())
            .and_then(|first| std::path::Path::new(first).file_name())
            .and_then(|n| n.to_str())
            .filter(|n| !n.is_empty() && *n != "exe");

        from_cmdline.unwrap_or(&self.name)
    }

    /// Identity of this process: the pair that survives PID reuse.
    pub fn key(&self) -> ProcessKey {
        ProcessKey {
            pid: self.pid,
            starttime_jiffies: self.starttime_jiffies,
        }
    }
}

/// Key for process cache: (pid, starttime).
/// Using starttime prevents PID reuse from contaminating the cache.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ProcessKey {
    pub pid: i32,
    pub starttime_jiffies: u64,
}

/// Signal to send to a process.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum Signal {
    Term,
    Kill,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn process(comm: &str, cmdline: &[&str]) -> Process {
        Process {
            pid: 1,
            ppid: 1,
            name: comm.to_string(),
            cmdline: cmdline.iter().map(|s| s.to_string()).collect(),
            exe: None,
            cwd: None,
            uid: None,
            user: None,
            started_at: None,
            starttime_jiffies: 1,
            cpu_jiffies: 0,
            metrics: Default::default(),
            state: ProcessState::Sleeping,
            threads: 1,
        }
    }

    #[test]
    fn test_binary_name_from_a_normal_argv() {
        assert_eq!(
            process("node", &["node", "server.js"]).binary_name(),
            "node"
        );
        assert_eq!(
            process("node", &["/usr/bin/node", "server.js"]).binary_name(),
            "node"
        );
    }

    /// Chrome packs its entire command line into argv[0]. Splitting on `/` alone
    /// yields the trailing flag: "renderD...uuid=3190708990060038890".
    #[test]
    fn test_binary_name_survives_a_packed_argv_blob() {
        let argv0 = "/opt/google/chrome/chrome --type=utility \
                     --render-node-override=/dev/dri/renderD128 \
                     --trace-process-track-uuid=3190708989122997041";
        assert_eq!(process("chrome", &[argv0]).binary_name(), "chrome");
    }

    /// Anything that re-execs reports /proc/self/exe; comm is the real answer.
    #[test]
    fn test_binary_name_falls_back_to_comm_for_proc_self_exe() {
        assert_eq!(
            process("Discord", &["/proc/self/exe", "--type=renderer"]).binary_name(),
            "Discord"
        );
    }

    #[test]
    fn test_binary_name_falls_back_to_comm_for_empty_cmdline() {
        assert_eq!(process("kworker", &[]).binary_name(), "kworker");
    }
}
