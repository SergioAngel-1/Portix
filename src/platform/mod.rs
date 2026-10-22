pub mod linux;

use crate::model::{Container, Process, Signal, Socket};
use anyhow::Result;
use std::path::Path;

/// A process whose PID was resolved by the inode walk but vanished before its
/// `/proc/<pid>/stat` could be read.
///
/// This is the normal `/proc` race, not an error worth alarming about — but the
/// engine must not confuse it with a permission or format failure. It used to
/// classify on the anyhow context string (`"process disappeared"`), which
/// silently misfiled every vanished process as `ProcessesPartialRead` the day
/// someone reworded the message. A typed error makes that distinction
/// unbreakable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProcessGone {
    pub pid: i32,
}

impl std::fmt::Display for ProcessGone {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "process {} disappeared", self.pid)
    }
}

impl std::error::Error for ProcessGone {}

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
    ///
    /// `include_udp` lets the caller skip `/proc/net/udp*` entirely when UDP
    /// sockets are filtered out of the view: reading two extra proc files just
    /// to discard every row is pure waste on a busy box.
    fn list_listening_sockets(&self, include_udp: bool) -> Result<Vec<Socket>>;

    /// Get metadata for a specific process.
    ///
    /// The error is preserved by the engine so a disappearing process can be
    /// distinguished from a permission/format failure in diagnostics.
    fn process_metadata(&self, pid: i32) -> Result<Process>;

    /// Read only the process start time (in jiffies since boot).
    ///
    /// Cheap identity check: `(pid, starttime)` survives PID reuse, and the
    /// engine validates its cached inode→process map with one read per owner
    /// per scan instead of re-walking every fd table.
    fn process_starttime(&self, pid: i32) -> Option<u64>;

    /// Effective UID of the Portix process.
    ///
    /// `/proc/<pid>/fd` is only readable for the process owner (and root).
    /// Sockets owned by another UID can never be resolved by this process, so
    /// the engine must not retry an impossible fd walk for them on every scan.
    fn effective_uid(&self) -> u32;

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
    fn list_listening_sockets(&self, include_udp: bool) -> Result<Vec<Socket>> {
        linux::net::list_listening_sockets(&self.proc_root, include_udp)
    }

    fn process_metadata(&self, pid: i32) -> Result<Process> {
        linux::proc::process_metadata(&self.proc_root, pid)
    }

    fn process_starttime(&self, pid: i32) -> Option<u64> {
        linux::proc::read_starttime_jiffies(&self.proc_root, pid)
    }

    fn effective_uid(&self) -> u32 {
        linux::proc::effective_uid()
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
