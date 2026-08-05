use serde::Serialize;
use std::net::IpAddr;

/// Transport family of a socket, ignoring the IP version.
///
/// A service that binds both `0.0.0.0` and `::` produces two kernel sockets but
/// is one service; grouping by family is what collapses them into one row.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
pub enum Family {
    Tcp,
    Udp,
}

/// Network protocol of a socket.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
pub enum Protocol {
    Tcp,
    Tcp6,
    Udp,
    Udp6,
}

impl Protocol {
    pub fn family(&self) -> Family {
        match self {
            Protocol::Tcp | Protocol::Tcp6 => Family::Tcp,
            Protocol::Udp | Protocol::Udp6 => Family::Udp,
        }
    }

    pub fn is_v6(&self) -> bool {
        matches!(self, Protocol::Tcp6 | Protocol::Udp6)
    }
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
    /// True when the same process serves this port over both IPv4 and IPv6.
    #[serde(default)]
    pub dual_stack: bool,
}

impl Socket {
    /// Create a URL for this socket.
    ///
    /// A service bound to a specific non-loopback address is opened on that
    /// address; loopback and wildcard binds are reachable via localhost.
    pub fn url(&self) -> String {
        match self.bind_scope {
            BindScope::Specific(ip) => match ip {
                IpAddr::V4(v4) => format!("http://{v4}:{}", self.port),
                IpAddr::V6(v6) => format!("http://[{v6}]:{}", self.port),
            },
            _ => format!("http://localhost:{}", self.port),
        }
    }

    /// Protocol as shown in the PROTO column, collapsing dual-stack pairs.
    pub fn proto_display(&self) -> String {
        if self.dual_stack {
            match self.protocol.family() {
                Family::Tcp => "tcp*".to_string(),
                Family::Udp => "udp*".to_string(),
            }
        } else {
            self.protocol.to_string()
        }
    }

    /// True when the port is in the privileged range reserved for system services.
    pub fn is_system_port(&self) -> bool {
        self.port < 1024
    }

    pub fn is_loopback(&self) -> bool {
        self.bind_scope == BindScope::Loopback
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

#[cfg(test)]
mod tests {
    use super::*;

    fn socket(addr: IpAddr, port: u16) -> Socket {
        Socket {
            protocol: Protocol::Tcp,
            local_addr: addr,
            port,
            state: TcpState::Listen,
            bind_scope: BindScope::from_ip(addr),
            inode: 1,
            uid: 1000,
            dual_stack: false,
        }
    }

    #[test]
    fn test_url_uses_localhost_for_wildcard_and_loopback() {
        let wildcard = socket("0.0.0.0".parse().unwrap(), 3000);
        assert_eq!(wildcard.url(), "http://localhost:3000");
        let loopback = socket("127.0.0.1".parse().unwrap(), 3000);
        assert_eq!(loopback.url(), "http://localhost:3000");
    }

    /// A service bound to a specific non-loopback address is opened on that
    /// address — localhost would point at nothing.
    #[test]
    fn test_url_uses_specific_address() {
        let v4 = socket("192.168.1.5".parse().unwrap(), 3000);
        assert_eq!(v4.url(), "http://192.168.1.5:3000");
        let v6 = socket("2001:db8::1".parse().unwrap(), 3000);
        assert_eq!(v6.url(), "http://[2001:db8::1]:3000");
    }
}
