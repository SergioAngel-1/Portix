use serde::Serialize;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

/// Network protocol of a socket.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
pub enum Protocol {
    Tcp,
    Tcp6,
    Udp,
    Udp6,
}

impl std::fmt::Display for Protocol {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Protocol::Tcp => write!(f, "tcp"),
            Protocol::Tcp6 => write!(f, "tcp6"),
            Protocol::Udp => write!(f, "udp"),
            Protocol::Udp6 => write!(f, "udp6"),
        }
    }
}

/// TCP connection state from /proc/net/tcp.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
pub enum TcpState {
    Established,
    SynSent,
    SynRecv,
    FinWait1,
    FinWait2,
    TimeWait,
    Close,
    CloseWait,
    LastAck,
    Listen,
    Closing,
    NewSynRecv,
}

impl TcpState {
    /// Parse from hex string in /proc/net/tcp.
    pub fn from_hex(hex: &str) -> Option<Self> {
        match hex {
            "01" => Some(TcpState::Established),
            "02" => Some(TcpState::SynSent),
            "03" => Some(TcpState::SynRecv),
            "04" => Some(TcpState::FinWait1),
            "05" => Some(TcpState::FinWait2),
            "06" => Some(TcpState::TimeWait),
            "07" => Some(TcpState::Close),
            "08" => Some(TcpState::CloseWait),
            "09" => Some(TcpState::LastAck),
            "0A" => Some(TcpState::Listen),
            "0B" => Some(TcpState::Closing),
            "0C" => Some(TcpState::NewSynRecv),
            _ => None,
        }
    }
}

/// How the socket is bound.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
pub enum BindScope {
    /// Bound to 127.0.0.1 or ::1
    Loopback,
    /// Bound to 0.0.0.0 or ::
    AllInterfaces,
    /// Bound to a specific non-loopback address
    Specific(IpAddr),
}

impl BindScope {
    pub fn from_ip(addr: IpAddr) -> Self {
        match addr {
            IpAddr::V4(v4) => {
                if v4.is_loopback() {
                    BindScope::Loopback
                } else if v4.is_unspecified() {
                    BindScope::AllInterfaces
                } else {
                    BindScope::Specific(addr)
                }
            }
            IpAddr::V6(v6) => {
                if v6.is_loopback() {
                    BindScope::Loopback
                } else if v6.is_unspecified() {
                    BindScope::AllInterfaces
                } else {
                    BindScope::Specific(addr)
                }
            }
        }
    }
}

/// A listening socket.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
pub struct Socket {
    pub protocol: Protocol,
    pub local_addr: IpAddr,
    pub port: u16,
    pub state: TcpState,
    pub bind_scope: BindScope,
    pub inode: u64,
    pub uid: u32,
}

impl Socket {
    /// Create a URL for this socket.
    pub fn url(&self) -> String {
        format!("http://localhost:{}", self.port)
    }

    /// Check if this socket is a listening TCP socket.
    pub fn is_listening(&self) -> bool {
        self.state == TcpState::Listen
    }

    /// Check if this socket is a UDP socket with a local port.
    pub fn is_udp_with_port(&self) -> bool {
        matches!(self.protocol, Protocol::Udp | Protocol::Udp6)
            && self.port != 0
            && self.local_addr == std::net::IpAddr::V4(Ipv4Addr::UNSPECIFIED)
                || self.local_addr == std::net::IpAddr::V6(Ipv6Addr::UNSPECIFIED)
    }
}

/// Unique identifier for a socket.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
pub struct SocketId {
    pub protocol: Protocol,
    pub addr: IpAddr,
    pub port: u16,
    pub inode: u64,
}

impl From<&Socket> for SocketId {
    fn from(s: &Socket) -> Self {
        Self {
            protocol: s.protocol,
            addr: s.local_addr,
            port: s.port,
            inode: s.inode,
        }
    }
}
