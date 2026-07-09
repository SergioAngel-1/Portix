use crate::model::Service;
use serde::Serialize;
use std::sync::Arc;
use std::time::Instant;

/// Reasons why some data couldn't be obtained.
#[derive(Debug, Clone, Serialize)]
pub enum Degradation {
    /// N sockets couldn't be resolved to a PID (permission issue)
    SocketsWithoutPid(usize),
    /// N processes couldn't be fully read
    ProcessesPartialRead(usize),
    /// N projects couldn't be detected
    ProjectDetectionFailed(usize),
    /// Network namespace scanning unavailable
    NetnsUnavailable,
}

impl Degradation {
    pub fn message(&self) -> String {
        match self {
            Degradation::SocketsWithoutPid(n) => {
                format!("{} sockets belong to other users. Run with sudo to see them.", n)
            }
            Degradation::ProcessesPartialRead(n) => {
                format!("{} processes couldn't be fully read.", n)
            }
            Degradation::ProjectDetectionFailed(n) => {
                format!("{} projects couldn't be detected.", n)
            }
            Degradation::NetnsUnavailable => {
                "Network namespace scanning unavailable. Containers may not be visible.".to_string()
            }
        }
    }
}

/// Immutable snapshot of the world at a point in time.
///
/// This is produced by the collector thread and consumed by the UI thread.
/// The UI replaces the Arc pointer atomically - no shared state, no locks.
#[derive(Debug, Clone, Serialize)]
pub struct Snapshot {
    /// All detected services
    pub services: Vec<Service>,
    /// Monotonic instant the scan started; not meaningful outside this process.
    #[serde(skip)]
    pub taken_at: Instant,
    /// Wall-clock time of the scan, for JSON consumers.
    pub taken_at_epoch_secs: u64,
    /// How long the scan took (as milliseconds)
    pub scan_duration_ms: u64,
    /// Any degraded data
    pub degradations: Vec<Degradation>,
}

impl Snapshot {
    /// Get services sorted by port.
    pub fn sorted_by_port(&self) -> Vec<&Service> {
        let mut services: Vec<&Service> = self.services.iter().collect();
        services.sort_by_key(|s| s.socket.port);
        services
    }

    /// Look up the service listening on a port, if any.
    pub fn by_port(&self, port: u16) -> Option<&Service> {
        self.services.iter().find(|s| s.socket.port == port)
    }
}

/// Type alias for shared immutable snapshots.
pub type SharedSnapshot = Arc<Snapshot>;
