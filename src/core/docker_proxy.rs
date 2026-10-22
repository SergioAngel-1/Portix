use crate::model::{Container, Runtime, Service};
use serde::Deserialize;
use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};

/// Docker Desktop's published-port metadata, obtained from its local Docker
/// Engine API. The API is optional: if the socket is absent, inaccessible, or
/// returns malformed data, the normal `/proc` row is preserved.
#[derive(Debug, Clone, Default)]
pub struct DockerDesktopData {
    pub containers: Vec<DockerDesktopContainer>,
    /// Host process name used when `/proc` cannot resolve the owner.
    pub host_process: String,
}

#[derive(Debug, Clone)]
pub struct DockerDesktopContainer {
    pub id: String,
    pub name: Option<String>,
    pub compose_project: Option<String>,
    pub command: Option<String>,
    pub published_ports: Vec<DockerDesktopPublishedPort>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DockerDesktopPublishedPort {
    pub host_ip: Option<String>,
    pub host_port: u16,
    pub container_port: u16,
    pub protocol: String,
}

#[derive(Debug, Deserialize)]
struct DockerApiContainer {
    #[serde(rename = "Id")]
    id: String,
    #[serde(rename = "Names", default)]
    names: Vec<String>,
    #[serde(rename = "Command", default)]
    command: Option<String>,
    #[serde(rename = "Labels", default)]
    labels: std::collections::HashMap<String, String>,
    #[serde(rename = "Ports", default)]
    ports: Vec<DockerApiPort>,
}

#[derive(Debug, Deserialize)]
struct DockerApiPort {
    #[serde(rename = "IP")]
    ip: Option<String>,
    #[serde(rename = "PrivatePort")]
    private_port: u16,
    #[serde(rename = "PublicPort")]
    public_port: Option<u16>,
    #[serde(rename = "Type")]
    protocol: String,
}

/// Per-operation timeout for the Docker Desktop API.
///
/// This call sits on the scan path: a hung Docker Desktop must not stall the
/// collector for a quarter of a second on every refresh. The engine also
/// caches both hits and misses, so this timeout is only paid on a cache miss.
const DOCKER_API_TIMEOUT: std::time::Duration = std::time::Duration::from_millis(100);

/// Hard cap on a Docker API response. A `DOCKER_HOST` pointing at any Unix
/// socket can return an arbitrarily large body; the collector must not grow
/// memory with it.
const DOCKER_API_MAX_BYTES: usize = 4 * 1024 * 1024;

/// Overall deadline for reading a Docker API response. Per-operation read
/// timeouts only bound a single `read`; a peer that drips one byte just under
/// the timeout would otherwise keep the collector busy forever.
const DOCKER_API_DEADLINE: std::time::Duration = std::time::Duration::from_millis(500);

/// Read Docker Desktop's local API endpoint and list running containers with
/// published ports. This uses only a Unix socket; it never opens a TCP socket.
pub fn read_docker_desktop(socket: Option<&Path>) -> Option<DockerDesktopData> {
    let socket = socket
        .map(PathBuf::from)
        .or_else(default_docker_desktop_socket)?;
    // A missing socket is the common native-Linux case. Checking first avoids
    // building a connection attempt (and an error) on every refresh.
    if !socket.exists() {
        return None;
    }
    let mut stream = UnixStream::connect(socket).ok()?;
    stream.set_read_timeout(Some(DOCKER_API_TIMEOUT)).ok()?;
    stream.set_write_timeout(Some(DOCKER_API_TIMEOUT)).ok()?;

    let request = b"GET /v1.41/containers/json?all=0 HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n";
    stream.write_all(request).ok()?;
    let response = read_bounded(&mut stream, DOCKER_API_MAX_BYTES, DOCKER_API_DEADLINE)?;
    parse_container_list_response(&response)
}

/// Read a response with both a byte cap and an overall deadline.
///
/// The caller's per-operation timeouts still bound each individual `read`, but
/// only this loop bounds the whole response: without it, a socket that drips
/// bytes (any `DOCKER_HOST` Unix socket) stalls the scan and grows `response`
/// without limit. Returns `None` on any read error, once the cap would be
/// exceeded, or when the deadline passes.
fn read_bounded<R: Read>(mut r: R, cap: usize, deadline: std::time::Duration) -> Option<Vec<u8>> {
    let started = std::time::Instant::now();
    let mut response = Vec::new();
    let mut chunk = [0u8; 8192];
    loop {
        if started.elapsed() >= deadline {
            return None;
        }
        match r.read(&mut chunk) {
            Ok(0) => return Some(response),
            Ok(n) => {
                if response.len() + n > cap {
                    return None;
                }
                response.extend_from_slice(&chunk[..n]);
            }
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
            Err(_) => return None,
        }
    }
}

fn parse_container_list_response(response: &[u8]) -> Option<DockerDesktopData> {
    let body = http_body(response)?;
    let decoded = decode_chunked_body(body);
    let api: Vec<DockerApiContainer> = match decoded {
        Some(decoded) => serde_json::from_slice(&decoded).ok()?,
        None => serde_json::from_slice(body).ok()?,
    };

    Some(DockerDesktopData {
        containers: api
            .into_iter()
            .map(|container| DockerDesktopContainer {
                id: container.id.chars().take(12).collect(),
                name: container
                    .names
                    .first()
                    .map(|name| name.trim_start_matches('/').to_string()),
                compose_project: container.labels.get("com.docker.compose.project").cloned(),
                command: container.command,
                published_ports: container
                    .ports
                    .into_iter()
                    .filter_map(|port| {
                        Some(DockerDesktopPublishedPort {
                            host_ip: port.ip.filter(|ip| !ip.is_empty()),
                            host_port: port.public_port?,
                            container_port: port.private_port,
                            protocol: port.protocol,
                        })
                    })
                    .collect(),
            })
            .collect(),
        host_process: "com.docker.backend".to_string(),
    })
}

fn http_body(response: &[u8]) -> Option<&[u8]> {
    response
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .map(|index| &response[index + 4..])
        .or_else(|| {
            response
                .windows(2)
                .position(|window| window == b"\n\n")
                .map(|index| &response[index + 2..])
        })
}

fn decode_chunked_body(body: &[u8]) -> Option<Vec<u8>> {
    let mut cursor = 0;
    let mut decoded = Vec::new();
    loop {
        let line_end = body
            .get(cursor..)?
            .windows(2)
            .position(|window| window == b"\r\n")?;
        let size = usize::from_str_radix(
            std::str::from_utf8(&body[cursor..cursor + line_end])
                .ok()?
                .trim(),
            16,
        )
        .ok()?;
        cursor += line_end + 2;
        if size == 0 {
            return Some(decoded);
        }
        let chunk = body.get(cursor..cursor + size)?;
        decoded.extend_from_slice(chunk);
        cursor += size;
        if body.get(cursor..cursor + 2)? != b"\r\n" {
            return None;
        }
        cursor += 2;
    }
}

fn default_docker_desktop_socket() -> Option<PathBuf> {
    if let Some(host) = std::env::var_os("DOCKER_HOST") {
        let host = host.to_string_lossy();
        return host.strip_prefix("unix://").map(PathBuf::from);
    }
    crate::core::config::display_home().map(|home| home.join(".docker/desktop/docker.sock"))
}

/// Parse the legacy `docker-proxy` command line.
#[derive(Debug, Clone, Default)]
pub struct DockerProxyInfo {
    pub container_ip: Option<String>,
}

impl DockerProxyInfo {
    pub fn from_cmdline(cmdline: &[String]) -> Option<Self> {
        let binary = Path::new(cmdline.first()?).file_name()?.to_str()?;
        if binary != "docker-proxy" {
            return None;
        }

        let mut info = Self::default();
        let args = cmdline.iter().map(String::as_str).collect::<Vec<_>>();
        for (i, arg) in args.iter().enumerate() {
            let rest = arg.trim_start_matches('-');
            if rest == "container-ip"
                || rest.starts_with("container-ip=")
                || rest.starts_with("container-ip ")
            {
                let value = rest
                    .strip_prefix("container-ip=")
                    .or_else(|| rest.strip_prefix("container-ip ").map(str::trim))
                    .or_else(|| args.get(i + 1).copied())
                    .filter(|value| !value.is_empty() && !value.starts_with('-'))
                    .unwrap_or("unknown");
                info.container_ip = Some(value.to_string());
            }
        }

        info.container_ip.as_ref()?;
        Some(info)
    }
}

/// Enrich a row from Docker Desktop's published-port table. The socket port is
/// the host port visible in `/proc/net`; the API supplies the destination
/// container, name and Compose project.
pub fn rewrite_docker_desktop(service: &mut Service, data: &DockerDesktopData) {
    let protocol = match service.socket.protocol {
        crate::model::Protocol::Tcp | crate::model::Protocol::Tcp6 => "tcp",
        crate::model::Protocol::Udp | crate::model::Protocol::Udp6 => "udp",
    };
    let host_ip = service.socket.local_addr.to_string();

    let Some(container) = data.containers.iter().find(|container| {
        container.published_ports.iter().any(|port| {
            if port.host_port != service.socket.port || port.protocol != protocol {
                return false;
            }
            port.host_ip
                .as_deref()
                .map_or(true, |ip| ip == "0.0.0.0" || ip == "::" || ip == host_ip)
        })
    }) else {
        return;
    };

    service.container = Some(Container {
        runtime: Runtime::Docker,
        id: container.id.clone(),
        name: container.name.clone(),
        compose_project: container.compose_project.clone(),
        command: container.command.clone(),
        host_process: Some(data.host_process.clone()),
    });
    // The container replaces the host-side process row semantically. Its name
    // is displayed in PROJECT, while Docker's runtime marker is displayed in
    // FRAMEWORK; the host process is retained separately for COMMAND.
    service.project = None;
    service.framework = None;
    service.docker_proxy_rewritten = true;
}

/// Best-effort rewrite for the legacy docker-proxy command line.
pub fn rewrite_docker_proxy(service: &mut Service) {
    let Some(process) = service.process.as_ref() else {
        return;
    };
    let Some(info) = DockerProxyInfo::from_cmdline(&process.cmdline) else {
        return;
    };
    service.container = Some(Container {
        runtime: Runtime::Docker,
        id: info.container_ip.unwrap_or_else(|| "unknown".to_string()),
        name: None,
        compose_project: None,
        command: None,
        host_process: None,
    });
    service.project = None;
    service.framework = None;
    service.docker_proxy_rewritten = true;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{BindScope, Process, ProcessState, Protocol, ServiceId, Socket, TcpState};

    fn service(port: u16, protocol: Protocol) -> Service {
        let socket = Socket {
            protocol,
            local_addr: std::net::Ipv4Addr::UNSPECIFIED.into(),
            port,
            state: TcpState::Listen,
            bind_scope: BindScope::AllInterfaces,
            inode: 1,
            uid: 1000,
            dual_stack: false,
        };
        Service {
            id: ServiceId::from_socket(&socket),
            socket,
            process: Some(Process {
                pid: 2324,
                ppid: 1,
                name: "com.docker.back".into(),
                cmdline: vec![
                    "/opt/docker-desktop/bin/com.docker.backend".into(),
                    "services".into(),
                ],
                exe: Some("/opt/docker-desktop/bin/com.docker.backend".into()),
                cwd: None,
                uid: Some(1000),
                user: Some("sergi".into()),
                started_at: None,
                starttime_jiffies: 1,
                cpu_jiffies: 0,
                metrics: Default::default(),
                state: ProcessState::Sleeping,
                threads: 1,
            }),
            project: Some(crate::model::Project {
                root: PathBuf::from("/tmp/project"),
                workspace_root: None,
                name: "wrong-project".into(),
                technical_name: None,
                language: None,
                package_manager: None,
            }),
            framework: None,
            container: None,
            docker_proxy_rewritten: false,
        }
    }

    fn data() -> DockerDesktopData {
        DockerDesktopData {
            containers: vec![DockerDesktopContainer {
                id: "b27bd000cd6f".into(),
                name: Some("hwe_wordpress".into()),
                compose_project: Some("siu-tutuava".into()),
                command: Some("docker-entrypoint.sh apache2-foreground".into()),
                published_ports: vec![DockerDesktopPublishedPort {
                    host_ip: Some("0.0.0.0".into()),
                    host_port: 8081,
                    container_port: 80,
                    protocol: "tcp".into(),
                }],
            }],
            host_process: "com.docker.backend".into(),
        }
    }

    #[test]
    fn read_bounded_returns_a_response_under_the_cap() {
        let payload = b"HTTP/1.1 200 OK\r\n\r\n[]";
        assert_eq!(
            read_bounded(&payload[..], 1024, std::time::Duration::from_secs(1)),
            Some(payload.to_vec())
        );
    }

    #[test]
    fn read_bounded_rejects_a_response_over_the_cap() {
        let payload = [b'x'; 64];
        assert_eq!(
            read_bounded(&payload[..], 32, std::time::Duration::from_secs(1)),
            None
        );
    }

    #[test]
    fn read_bounded_stops_once_the_deadline_has_passed() {
        let payload = b"[]";
        assert_eq!(
            read_bounded(&payload[..], 1024, std::time::Duration::ZERO),
            None
        );
    }

    #[test]
    fn docker_desktop_rewrites_backend_row_from_published_port_data() {
        let mut service = service(8081, Protocol::Tcp);
        rewrite_docker_desktop(&mut service, &data());
        assert!(service.docker_proxy_rewritten);
        assert!(service.project.is_none());
        assert_eq!(service.container.as_ref().unwrap().id, "b27bd000cd6f");
        assert_eq!(
            service.container.as_ref().unwrap().name.as_deref(),
            Some("hwe_wordpress")
        );
        assert_eq!(
            service
                .container
                .as_ref()
                .unwrap()
                .compose_project
                .as_deref(),
            Some("siu-tutuava")
        );
        assert_eq!(service.project_display(), "hwe_wordpress");
        assert_eq!(service.runtime_display().as_deref(), Some("🐳 Docker"));
        assert_eq!(service.command_display(), "com.docker.backend");
        assert_eq!(
            service.container.as_ref().unwrap().command.as_deref(),
            Some("docker-entrypoint.sh apache2-foreground")
        );
    }

    #[test]
    fn docker_desktop_does_not_match_unpublished_or_wrong_protocol_ports() {
        let mut service = service(5433, Protocol::Udp);
        rewrite_docker_desktop(&mut service, &data());
        assert!(!service.docker_proxy_rewritten);
        assert!(service.container.is_none());
    }

    #[test]
    fn docker_desktop_parser_accepts_real_http_response_headers() {
        let response = b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n\r\n[{\"Id\":\"abcdef0123456789\",\"Names\":[\"/db\"],\"Labels\":{},\"Ports\":[{\"IP\":\"0.0.0.0\",\"PrivatePort\":5432,\"PublicPort\":5433,\"Type\":\"tcp\"}]}]";
        let parsed = parse_container_list_response(response).expect("valid Docker response");
        assert_eq!(parsed.containers[0].id, "abcdef012345");
        assert_eq!(parsed.containers[0].published_ports[0].host_port, 5433);
    }

    #[test]
    fn docker_desktop_parser_decodes_chunked_response() {
        let response = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n11\r\n[{\"Id\":\"abcdef012\r\n24\r\n3456789\",\"Names\":[\"/db\"],\"Labels\":{}\r\n4F\r\n,\"Ports\":[{\"IP\":\"0.0.0.0\",\"PrivatePort\":5432,\"PublicPort\":5433,\"Type\":\"tcp\"}]}]\r\n0\r\n\r\n";
        assert!(parse_container_list_response(response).is_some());
    }

    #[test]
    fn legacy_docker_proxy_parser_still_handles_container_ip() {
        let cmdline = vec![
            "/usr/bin/docker-proxy".into(),
            "--container-ip=172.17.0.2".into(),
        ];
        assert_eq!(
            DockerProxyInfo::from_cmdline(&cmdline)
                .unwrap()
                .container_ip
                .as_deref(),
            Some("172.17.0.2")
        );
    }
}
