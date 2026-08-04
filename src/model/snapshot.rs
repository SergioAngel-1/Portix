use crate::model::Service;
use serde::Serialize;
use std::sync::Arc;

/// Reasons why some data couldn't be obtained.
#[derive(Debug, Clone, Serialize)]
pub enum Degradation {
    /// N sockets couldn't be resolved to a PID (permission issue)
    SocketsWithoutPid(usize),
    /// N processes were found by PID but couldn't be read from /proc
    ProcessesPartialRead(usize),
    /// The scan itself failed before any socket could be listed.
    ScanFailed(String),
    /// A container runtime appears active, but its published ports may be
    /// hidden from `/proc/net` because its userland-proxy is disabled.
    ContainerPortsHidden(String),
}

impl Degradation {
    pub fn message(&self) -> String {
        match self {
            Degradation::SocketsWithoutPid(n) => {
                format!(
                    "{} sockets belong to other users. Run with sudo to see them.",
                    n
                )
            }
            Degradation::ProcessesPartialRead(n) => {
                format!("{} processes couldn't be fully read.", n)
            }
            Degradation::ScanFailed(msg) => format!("Scan failed: {msg}"),
            Degradation::ContainerPortsHidden(msg) => {
                format!("Container ports may be hidden: {msg}")
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_partial_read_message_is_distinct_from_unresolved() {
        assert_eq!(
            Degradation::ProcessesPartialRead(3).message(),
            "3 processes couldn't be fully read."
        );
        assert!(Degradation::SocketsWithoutPid(3)
            .message()
            .contains("other users"));
    }
}

/// Immutable snapshot of the world at a point in time.
///
/// Produced by `Engine::scan()` — which runs synchronously on the caller's
/// thread in the current MVP (the collector-thread design of PRD §10.3 is not
/// implemented) — and passed to the UI as an `Arc` so snapshot updates are a
/// cheap pointer swap.
#[derive(Debug, Clone, Serialize)]
pub struct Snapshot {
    /// All detected services
    pub services: Vec<Service>,
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
