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
    use crate::model::{BindScope, Protocol, ServiceId, Socket, TcpState};

    fn service(inode: u64, port: u16) -> Service {
        let socket = Socket {
            protocol: Protocol::Tcp,
            local_addr: std::net::Ipv4Addr::LOCALHOST.into(),
            port,
            state: TcpState::Listen,
            bind_scope: BindScope::Loopback,
            inode,
            uid: 1000,
            dual_stack: false,
        };
        Service {
            id: ServiceId::from_socket(&socket),
            socket,
            process: None,
            project: None,
            framework: None,
            container: None,
            docker_proxy_rewritten: false,
        }
    }

    /// Two processes on one port (SO_REUSEPORT): `by_port` answers the first
    /// row, `by_inode` the exact one — the identity a kill must carry.
    #[test]
    fn test_by_inode_distinguishes_same_port_rows() {
        let snap = Snapshot {
            services: vec![service(11, 3000), service(22, 3000)],
            taken_at_epoch_secs: 0,
            scan_duration_ms: 0,
            degradations: vec![],
        };
        assert_eq!(snap.by_port(3000).unwrap().socket.inode, 11);
        assert_eq!(snap.by_inode(22).unwrap().socket.inode, 22);
        assert!(snap.by_inode(99).is_none());
    }

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

    /// Look up the service owning a specific socket inode, if any.
    ///
    /// The inode is the stable identity of a *specific* row: `by_port` returns
    /// the first of several listeners that can share one port (SO_REUSEPORT
    /// workers, containers publishing the same port), while the inode pins the
    /// exact socket the user selected. A rebind closes the old socket and
    /// reopens a new inode, so an inode lookup misses — which is the safe
    /// direction for a kill.
    pub fn by_inode(&self, inode: u64) -> Option<&Service> {
        self.services.iter().find(|s| s.socket.inode == inode)
    }
}

/// Type alias for shared immutable snapshots.
pub type SharedSnapshot = Arc<Snapshot>;
