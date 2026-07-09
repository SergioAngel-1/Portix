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
    #[serde(skip)]
    pub uptime: Option<std::time::Duration>,
}

/// A process with its metadata.
#[derive(Debug, Clone, Serialize)]
pub struct Process {
    pub pid: i32,
    pub ppid: i32,
    pub name: String,
    pub cmdline: Vec<String>,
    #[serde(skip)]
    pub exe: Option<PathBuf>,
    #[serde(skip)]
    pub cwd: Option<PathBuf>,
    pub user: Option<String>,
    #[serde(skip)]
    pub started_at: Option<SystemTime>,
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

    /// Get the binary name from cmdline.
    pub fn binary_name(&self) -> &str {
        self.cmdline
            .first()
            .map(|s| {
                std::path::Path::new(s)
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or(s)
            })
            .unwrap_or(&self.name)
    }

    /// Check if this is a kernel thread.
    pub fn is_kernel_thread(&self) -> bool {
        self.exe.is_none() && self.cwd.is_none()
    }
}

/// Key for process cache: (pid, starttime).
/// Using starttime prevents PID reuse from contaminating the cache.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ProcessKey {
    pub pid: i32,
    pub starttime_jiffies: u64,
}

/// Signal to send to a process.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum Signal {
    Term,
    Kill,
    Interrupt,
}
