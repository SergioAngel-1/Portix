pub mod linux;

use crate::model::{Container, Process, Signal, Socket};
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
///
/// `Send` is required because the TUI hands the `Engine` (which boxes this
/// trait) to the collector thread that runs the periodic scans off the UI
/// thread; the UI thread locks it back for the rare synchronous kill-path
/// re-scans.
pub trait PlatformProbe: Send {
    /// List all listening sockets on the system.
    fn list_listening_sockets(&self) -> Result<Vec<Socket>>;

    /// Get metadata for a specific process.
    ///
    /// The error is preserved by the engine so a disappearing process can be
    /// distinguished from a permission/format failure in diagnostics.
    fn process_metadata(&self, pid: i32) -> Result<Process>;

    /// Check if a process is running inside a container.
    fn container_of(&self, pid: i32) -> Option<Container>;

    /// System-level container-runtime heuristic for the hidden-ports warning:
    /// `Some(Degradation::ContainerPortsHidden)` when a runtime is active but
    /// no `docker-proxy` sibling is visible (so published ports may be hidden
    /// from `/proc/net`). The caller decides how often to recompute it.
    fn container_warning(&self) -> Option<crate::model::Degradation>;

    /// Send a signal to a process, aborting if `expected_starttime` no longer
    /// matches the live process (PID reuse).
    fn signal(&self, pid: i32, expected_starttime: u64, sig: Signal) -> Result<()>;

    /// System-wide CPU jiffies, for computing per-process CPU shares.
    fn total_jiffies(&self) -> Option<u64>;

    /// Build the socket-inode → (pid, fd) reverse map for the inodes of
    /// interest, by walking every process's fd table. Processes whose fd
    /// tables are unreadable (other users, hidepid) simply never appear.
    fn build_inode_map(
        &self,
        interest: &std::collections::HashSet<u64>,
    ) -> Result<std::collections::HashMap<u64, Vec<(i32, i32)>>>;

    /// Number of CPU cores, for scaling CPU percentages.
    fn num_cpus(&self) -> usize;

    /// Resolve a numeric UID to a username via NSS (`getpwuid_r`), if any.
    fn username_for_uid(&self, uid: u32) -> Option<String>;
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

    fn process_metadata(&self, pid: i32) -> Result<Process> {
        linux::proc::process_metadata(&self.proc_root, pid)
    }

    fn container_of(&self, pid: i32) -> Option<Container> {
        linux::cgroup::container_of(&self.proc_root, pid)
    }

    fn container_warning(&self) -> Option<crate::model::Degradation> {
        linux::container_warning::detect_container_warning(&self.proc_root)
    }

    fn signal(&self, pid: i32, expected_starttime: u64, sig: Signal) -> Result<()> {
        linux::signal::send_signal(&self.proc_root, pid, expected_starttime, sig)
    }

    fn total_jiffies(&self) -> Option<u64> {
        linux::metrics::get_total_jiffies(&self.proc_root)
    }

    fn build_inode_map(
        &self,
        interest: &std::collections::HashSet<u64>,
    ) -> Result<std::collections::HashMap<u64, Vec<(i32, i32)>>> {
        linux::fd::build_inode_map(&self.proc_root, interest)
    }

    fn num_cpus(&self) -> usize {
        linux::metrics::get_num_cpus()
    }

    fn username_for_uid(&self, uid: u32) -> Option<String> {
        linux::proc::uid_to_username(uid)
    }
}
