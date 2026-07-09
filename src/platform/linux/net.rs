use crate::model::{Protocol, Socket, TcpState};
use anyhow::Result;
use std::path::Path;

/// Parse /proc/net/tcp and /proc/net/tcp6 to find listening sockets.
pub fn list_listening_sockets(proc_root: &Path) -> Result<Vec<Socket>> {
    let mut sockets = Vec::new();

    // Read TCP sockets
    if let Ok(tcp) = read_proc_net(proc_root, "tcp", Protocol::Tcp) {
        sockets.extend(tcp);
    }
    if let Ok(tcp6) = read_proc_net(proc_root, "tcp6", Protocol::Tcp6) {
        sockets.extend(tcp6);
    }

    // Read UDP sockets
    if let Ok(udp) = read_proc_net(proc_root, "udp", Protocol::Udp) {
        sockets.extend(udp);
    }
    if let Ok(udp6) = read_proc_net(proc_root, "udp6", Protocol::Udp6) {
        sockets.extend(udp6);
    }

    Ok(sockets)
}

fn read_proc_net(proc_root: &Path, file: &str, protocol: Protocol) -> Result<Vec<Socket>> {
    let path = proc_root.join("net").join(file);
    let content = std::fs::read_to_string(&path)?;
    let mut sockets = Vec::new();

    for line in content.lines().skip(1) {
        // Skip header line
        if let Some(socket) = parse_proc_net_line(line, protocol) {
            if is_serving(&socket, line, protocol) {
                sockets.push(socket);
            }
        }
    }

    Ok(sockets)
}

/// Decide whether a parsed socket represents a service *offering* a port.
///
/// TCP is easy: only LISTEN. UDP has no connection state, so a socket only
/// counts when it has a local port and no peer — otherwise every outbound
/// mDNS/QUIC socket a browser opens shows up as a "service" on an ephemeral
/// port. See PRD §7.1, trap 5.
fn is_serving(socket: &Socket, line: &str, protocol: Protocol) -> bool {
    match protocol.family() {
        crate::model::Family::Tcp => socket.state == TcpState::Listen,
        crate::model::Family::Udp => {
            if socket.port == 0 {
                return false;
            }
            match remote_endpoint(line) {
                Some((addr_hex, port)) => port == 0 && addr_hex.bytes().all(|b| b == b'0'),
                None => false,
            }
        }
    }
}

/// Extract the remote `ADDR:PORT` (field index 2) as (hex_addr, port).
fn remote_endpoint(line: &str) -> Option<(&str, u16)> {
    let field = line.split_whitespace().nth(2)?;
    let (addr_hex, port_hex) = field.rsplit_once(':')?;
    let port = u16::from_str_radix(port_hex, 16).ok()?;
    Some((addr_hex, port))
}

/// Parse a single line from /proc/net/tcp* or /proc/net/udp*.
pub fn parse_proc_net_line(line: &str, protocol: Protocol) -> Option<Socket> {
    let parts: Vec<&str> = line.split_whitespace().collect();
    if parts.len() < 10 {
        return None;
    }

    // The format is: "sl local_address rem_address st tx_queue rx_queue tr tm->when retrnsmt uid timeout inode"
    // After splitting by whitespace:
    // [0] = "0:" (sl)
    // [1] = "0100007F:1F90" (local_address)
    // [2] = "00000000:0000" (rem_address)
    // [3] = "0A" (st - state)
    // [4] = "00000000:00000000" (tx_queue)
    // [5] = "00:00000000" (tr:tm->when)
    // [6] = "00000000" (retrnsmt)
    // [7] = "1000" (uid)
    // [8] = "0" (timeout)
    // [9] = "486312" (inode)

    // Parse local_address (index 1): "ADDR:PORT" in hex
    let local_addr_str = parts.get(1)?;
    let (addr_hex, port_hex) = local_addr_str.rsplit_once(':')?;

    // Parse state (index 3)
    let state_hex = parts.get(3)?;
    let state = TcpState::from_hex(state_hex)?;

    // Parse uid (index 7)
    let uid = parts.get(7)?.parse::<u32>().ok()?;

    // Parse inode (index 9)
    let inode = parts.get(9)?.parse::<u64>().ok()?;

    // Parse port (big-endian hex)
    let port = u16::from_str_radix(port_hex, 16).ok()?;

    // Parse address (little-endian for each 4-byte group)
    let local_addr = match protocol {
        Protocol::Tcp | Protocol::Udp => {
            let addr_u32 = u32::from_str_radix(addr_hex, 16).ok()?;
            let bytes = addr_u32.to_le_bytes();
            std::net::Ipv4Addr::new(bytes[0], bytes[1], bytes[2], bytes[3]).into()
        }
        Protocol::Tcp6 | Protocol::Udp6 => {
            if addr_hex.len() != 32 {
                return None;
            }
            let mut bytes = [0u8; 16];
            for i in 0..4 {
                let start = i * 8;
                let group_hex = &addr_hex[start..start + 8];
                let group_u32 = u32::from_str_radix(group_hex, 16).ok()?;
                let group_bytes = group_u32.to_le_bytes();
                bytes[i * 4] = group_bytes[0];
                bytes[i * 4 + 1] = group_bytes[1];
                bytes[i * 4 + 2] = group_bytes[2];
                bytes[i * 4 + 3] = group_bytes[3];
            }
            std::net::Ipv6Addr::from(bytes).into()
        }
    };

    let bind_scope = crate::model::BindScope::from_ip(local_addr);

    Some(Socket {
        protocol,
        local_addr,
        port,
        state,
        bind_scope,
        inode,
        uid,
        dual_stack: false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_tcp_listen() {
        let line = "   0: 0100007F:1F90 00000000:0000 0A 00000000:00000000 00:00000000 00000000  1000        0 486312 1 ffff888101234567 100 0 0 10 0";
        // Debug: print the parts
        let parts: Vec<&str> = line.split_whitespace().collect();
        eprintln!("Parts count: {}", parts.len());
        for (i, part) in parts.iter().enumerate() {
            eprintln!("  [{}]: {:?}", i, part);
        }
        let socket = parse_proc_net_line(line, Protocol::Tcp).unwrap();
        assert_eq!(socket.port, 8080);
        assert_eq!(socket.state, TcpState::Listen);
        assert_eq!(socket.inode, 486312);
        assert_eq!(socket.local_addr, std::net::IpAddr::V4(std::net::Ipv4Addr::new(127, 0, 0, 1)));
    }

    #[test]
    fn test_parse_tcp_all_interfaces() {
        let line = "   1: 00000000:0BB8 00000000:0000 0A 00000000:00000000 00:00000000 00000000  1000        0 486401 1 ffff888101234567 100 0 0 10 0";
        let socket = parse_proc_net_line(line, Protocol::Tcp).unwrap();
        assert_eq!(socket.port, 3000);
        assert_eq!(socket.bind_scope, crate::model::BindScope::AllInterfaces);
    }

    #[test]
    fn test_parse_ipv6_unspecified_is_not_byte_reversed_across_groups() {
        // [::]:3000 — every group is zero, so this only checks the shape.
        let line = "   0: 00000000000000000000000000000000:0BB8 00000000000000000000000000000000:0000 0A 00000000:00000000 00:00000000 00000000  1000        0 486401 1 0000 100 0 0 10 0";
        let socket = parse_proc_net_line(line, Protocol::Tcp6).unwrap();
        assert_eq!(socket.port, 3000);
        assert_eq!(socket.bind_scope, crate::model::BindScope::AllInterfaces);
    }

    #[test]
    fn test_parse_ipv6_loopback() {
        // ::1 — the last u32 group is 0x01000000 in host order.
        let line = "   0: 00000000000000000000000001000000:1F90 00000000000000000000000000000000:0000 0A 00000000:00000000 00:00000000 00000000  1000        0 486401 1 0000 100 0 0 10 0";
        let socket = parse_proc_net_line(line, Protocol::Tcp6).unwrap();
        assert_eq!(socket.local_addr, std::net::IpAddr::V6(std::net::Ipv6Addr::LOCALHOST));
    }

    #[test]
    fn test_tcp_non_listen_is_not_serving() {
        // st = 01 (ESTABLISHED)
        let line = "   0: 0100007F:1F90 0100007F:C000 01 00000000:00000000 00:00000000 00000000  1000        0 486312 1 0000 100 0 0 10 0";
        let socket = parse_proc_net_line(line, Protocol::Tcp).unwrap();
        assert!(!is_serving(&socket, line, Protocol::Tcp));
    }

    #[test]
    fn test_bound_udp_socket_is_serving() {
        // Bound, no peer: a real UDP service.
        let line = "   0: 00000000:14E9 00000000:0000 07 00000000:00000000 00:00000000 00000000  1000        0 486312 2 0000 0";
        let socket = parse_proc_net_line(line, Protocol::Udp).unwrap();
        assert!(is_serving(&socket, line, Protocol::Udp));
    }

    #[test]
    fn test_connected_udp_socket_is_not_serving() {
        // Peer address set: this is an outbound socket (Chrome mDNS/QUIC), not a service.
        let line = "   0: 0100007F:C1A3 0800080A:0035 07 00000000:00000000 00:00000000 00000000  1000        0 486313 2 0000 0";
        let socket = parse_proc_net_line(line, Protocol::Udp).unwrap();
        assert!(!is_serving(&socket, line, Protocol::Udp));
    }
}
