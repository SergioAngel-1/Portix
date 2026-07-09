use crate::model::Service;
use serde::Serialize;
use std::sync::Arc;
use std::time::{Duration, Instant};

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
    /// When this snapshot was taken (as epoch seconds)
    #[serde(skip)]
    pub taken_at: Instant,
    /// How long the scan took (as milliseconds)
    pub scan_duration_ms: u64,
    /// Any degraded data
    pub degredations: Vec<Degradation>,
}

impl Snapshot {
    /// Get services sorted by port.
    pub fn sorted_by_port(&self) -> Vec<&Service> {
        let mut services: Vec<&Service> = self.services.iter().collect();
        services.sort_by_key(|s| s.socket.port);
        services
    }

    /// Get services matching a filter.
    pub fn filtered(&self, query: &str) -> Vec<&Service> {
        let query_lower = query.to_lowercase();
        self.services
            .iter()
            .filter(|s| {
                // Search in port, project name, framework, command, PID
                let port_match = s.socket.port.to_string().contains(&query_lower);
                let project_match = s
                    .project
                    .as_ref()
                    .map(|p| p.display_name().to_lowercase().contains(&query_lower))
                    .unwrap_or(false);
                let framework_match = s
                    .framework
                    .as_ref()
                    .map(|f| f.display_name.to_lowercase().contains(&query_lower))
                    .unwrap_or(false);
                let cmd_match = s
                    .process
                    .as_ref()
                    .map(|p| p.command_display().to_lowercase().contains(&query_lower))
                    .unwrap_or(false);
                let pid_match = s
                    .process
                    .as_ref()
                    .map(|p| p.pid.to_string().contains(&query_lower))
                    .unwrap_or(false);

                port_match || project_match || framework_match || cmd_match || pid_match
            })
            .collect()
    }
}

/// Type alias for shared immutable snapshots.
pub type SharedSnapshot = Arc<Snapshot>;
