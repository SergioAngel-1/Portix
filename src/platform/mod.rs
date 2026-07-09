pub mod linux;

use crate::model::{Container, Process, Service, Signal, Socket};
use anyhow::Result;
use std::path::Path;

/// Platform abstraction layer.
///
/// This trait defines the interface between Portix and the operating system.
/// The Linux implementation reads from /proc, but other platforms could use
/// different mechanisms (libproc on macOS, GetExtendedTcpTable on Windows).
///
/// No module outside of `platform/` should import /proc, rustix, or system
/// absolute paths.
pub trait PlatformProbe {
    /// List all listening sockets on the system.
    fn list_listening_sockets(&self) -> Result<Vec<Socket>>;

    /// List all PIDs visible to the current user.
    fn list_pids(&self) -> Result<Vec<i32>>;

    /// Get metadata for a specific process.
    fn process_metadata(&self, pid: i32) -> Result<Process>;

    /// Check if a process is running inside a container.
    fn container_of(&self, pid: i32) -> Option<Container>;

    /// Send a signal to a process (or process group).
    fn signal(&self, pid: i32, sig: Signal) -> Result<()>;

    /// Get the root path for /proc (or equivalent).
    fn proc_root(&self) -> &Path;
}

/// Default platform probe using /proc.
pub struct LinuxProbe {
    proc_root: std::path::PathBuf,
}

impl LinuxProbe {
    pub fn new(proc_root: Option<&Path>) -> Self {
        Self {
            proc_root: proc_root
                .unwrap_or_else(|| Path::new("/proc"))
                .to_path_buf(),
        }
    }
}

impl PlatformProbe for LinuxProbe {
    fn list_listening_sockets(&self) -> Result<Vec<Socket>> {
        linux::net::list_listening_sockets(&self.proc_root)
    }

    fn list_pids(&self) -> Result<Vec<i32>> {
        linux::proc::list_pids(&self.proc_root)
    }

    fn process_metadata(&self, pid: i32) -> Result<Process> {
        linux::proc::process_metadata(&self.proc_root, pid)
    }

    fn container_of(&self, pid: i32) -> Option<Container> {
        linux::cgroup::container_of(&self.proc_root, pid)
    }

    fn signal(&self, pid: i32, sig: Signal) -> Result<()> {
        linux::signal::send_signal(pid, sig)
    }

    fn proc_root(&self) -> &Path {
        &self.proc_root
    }
}
