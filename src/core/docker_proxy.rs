use crate::model::{Container, Service};
use std::path::Path;

/// Parse the two `docker-proxy` flags we care about from a cmdline.
///
/// Canonical form seen in the wild:
/// `/usr/bin/docker-proxy
///  -proto tcp
///  -host-ip 0.0.0.0
///  -host-port 5432
///  -container-ip 172.17.0.2
///  -container-port 5432`
///
/// We do not depend on every flag being present; we extract what we can and
/// leave the rest blank. Only `-container-ip` is consumed today — the socket's
/// host port is what the UI displays, so `-container-port` is not needed.
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
            let mut rest = *arg;
            // docker-proxy flags may be single-dash (`-proto`) or double-dash
            // (`--container-ip`); strip all leading dashes so the name matches
            // regardless of how many the binary emits.
            rest = rest.trim_start_matches('-');

            if rest == "container-ip"
                || rest.starts_with("container-ip=")
                || rest.starts_with("container-ip ")
            {
                let value = rest
                    .strip_prefix("container-ip=")
                    .or_else(|| rest.strip_prefix("container-ip ").map(|s| s.trim()))
                    .or_else(|| args.get(i + 1).copied())
                    .unwrap_or("unknown");
                info.container_ip = Some(value.to_string());
            }
        }

        if info.container_ip.is_none() {
            None
        } else {
            Some(info)
        }
    }
}

/// Best-effort container attribution for a `docker-proxy` service.
///
/// We do not call out to the Docker API. Instead, we synthesize a container
/// record from the cmdline metadata we already have.
pub fn rewrite_docker_proxy(service: &mut Service) {
    let Some(process) = service.process.as_ref() else {
        return;
    };

    let Some(info) = DockerProxyInfo::from_cmdline(&process.cmdline) else {
        return;
    };

    let container = Some(Container {
        runtime: crate::model::Runtime::Docker,
        id: info.container_ip.unwrap_or_else(|| "unknown".to_string()),
        name: None,
        compose_project: None,
    });

    service.container = container;
    service.project = None;
    service.framework = None;
    service.docker_proxy_rewritten = true;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Process, ProcessState, Protocol, Socket, TcpState};

    fn docker_proxy_service(cmdline: &[&str]) -> Service {
        let socket = Socket {
            protocol: Protocol::Tcp,
            local_addr: std::net::Ipv4Addr::LOCALHOST.into(),
            port: 5432,
            state: TcpState::Listen,
            bind_scope: crate::model::BindScope::AllInterfaces,
            inode: 1,
            uid: 1000,
            dual_stack: false,
        };
        Service {
            id: crate::model::ServiceId::from_socket(&socket),
            socket,
            process: Some(Process {
                pid: 1,
                ppid: 0,
                name: "docker-proxy".into(),
                cmdline: cmdline.iter().map(|s| s.to_string()).collect(),
                exe: None,
                cwd: None,
                uid: None,
                user: None,
                started_at: None,
                starttime_jiffies: 1,
                cpu_jiffies: 0,
                metrics: Default::default(),
                state: ProcessState::Sleeping,
                threads: 1,
            }),
            project: Some(crate::model::Project {
                root: std::path::PathBuf::from("/usr/bin"),
                workspace_root: None,
                name: "docker-proxy".into(),
                technical_name: None,
                language: None,
                package_manager: None,
            }),
            framework: None,
            container: None,
            docker_proxy_rewritten: false,
        }
    }

    #[test]
    fn test_docker_proxy_rewrite_attaches_container() {
        let mut service = docker_proxy_service(&[
            "/usr/bin/docker-proxy",
            "-proto tcp",
            "-host-ip 0.0.0.0",
            "-host-port 5432",
            "-container-ip 172.17.0.2",
            "-container-port 5432",
        ]);

        rewrite_docker_proxy(&mut service);

        assert!(service.docker_proxy_rewritten);
        assert!(service.project.is_none());
        assert!(service.framework.is_none());
        assert_eq!(
            service.container.as_ref().unwrap().runtime,
            crate::model::Runtime::Docker
        );
        assert_eq!(service.container.as_ref().unwrap().id, "172.17.0.2");
        assert_eq!(service.socket.port, 5432);
    }

    #[test]
    fn test_non_docker_proxy_is_ignored() {
        let mut service = docker_proxy_service(&["/usr/bin/node", "server.js"]);
        rewrite_docker_proxy(&mut service);
        assert!(!service.docker_proxy_rewritten);
        assert!(service.project.is_some());
    }

    #[test]
    fn test_docker_proxy_double_dash_equals_form() {
        let mut service = docker_proxy_service(&[
            "/usr/bin/docker-proxy",
            "--proto",
            "tcp",
            "--host-ip=0.0.0.0",
            "--host-port=5432",
            "--container-ip=172.17.0.2",
            "--container-port=5432",
        ]);
        rewrite_docker_proxy(&mut service);
        assert!(service.docker_proxy_rewritten);
        assert_eq!(service.container.as_ref().unwrap().id, "172.17.0.2");
        assert_eq!(
            service.container.as_ref().unwrap().runtime,
            crate::model::Runtime::Docker
        );
    }

    #[test]
    fn test_docker_proxy_double_dash_separated_form() {
        let mut service = docker_proxy_service(&[
            "/usr/bin/docker-proxy",
            "--proto",
            "tcp",
            "--host-ip",
            "0.0.0.0",
            "--host-port",
            "5432",
            "--container-ip",
            "172.17.0.2",
            "--container-port",
            "5432",
        ]);
        rewrite_docker_proxy(&mut service);
        assert!(service.docker_proxy_rewritten);
        assert_eq!(service.container.as_ref().unwrap().id, "172.17.0.2");
    }

    #[test]
    fn test_docker_proxy_without_process_is_ignored() {
        let mut service = docker_proxy_service(&["/usr/bin/docker-proxy"]);
        service.process = None;
        rewrite_docker_proxy(&mut service);
        assert!(!service.docker_proxy_rewritten);
        assert!(service.project.is_some());
    }
}
