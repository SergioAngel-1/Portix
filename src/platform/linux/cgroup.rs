use crate::model::{Container, Runtime};
use std::path::Path;

/// Detect if a process is running in a container by reading its cgroup.
pub fn container_of(proc_root: &Path, pid: i32) -> Option<Container> {
    let cgroup_path = proc_root.join(pid.to_string()).join("cgroup");
    let content = std::fs::read_to_string(&cgroup_path).ok()?;

    for line in content.lines() {
        // Try v2 format: "0::/system.slice/docker-..."
        if let Some(rest) = line.strip_prefix("0::") {
            if let Some(container) = parse_cgroup_path(rest) {
                return Some(container);
            }
        }

        // Try v1 format: "n:subsystem:/docker/..."
        if let Some(rest) = line.splitn(2, ':').nth(1) {
            if let Some(container) = parse_cgroup_path(rest) {
                return Some(container);
            }
        }
    }

    None
}

/// Parse a cgroup path to extract container info.
fn parse_cgroup_path(path: &str) -> Option<Container> {
    // Docker v2 (systemd): docker-<64hex>.scope
    if let Some(rest) = path.strip_prefix("system.slice/docker-") {
        if let Some(id) = rest.strip_suffix(".scope") {
            return Some(Container {
                runtime: Runtime::Docker,
                id: id.to_string(),
                name: None,
                compose_project: None,
            });
        }
    }

    // Docker v1: /docker/<64hex>
    if let Some(rest) = path.strip_prefix("/docker/") {
        let id = rest.trim_end_matches('/');
        if !id.is_empty() {
            return Some(Container {
                runtime: Runtime::Docker,
                id: id.to_string(),
                name: None,
                compose_project: None,
            });
        }
    }

    // Podman: libpod-<64hex>.scope
    if let Some(rest) = path.strip_prefix("system.slice/libpod-") {
        if let Some(id) = rest.strip_suffix(".scope") {
            return Some(Container {
                runtime: Runtime::Podman,
                id: id.to_string(),
                name: None,
                compose_project: None,
            });
        }
    }

    // containerd/k8s: cri-containerd-<64hex>.scope
    if let Some(rest) = path.strip_prefix("system.slice/cri-containerd-") {
        if let Some(id) = rest.strip_suffix(".scope") {
            return Some(Container {
                runtime: Runtime::Containerd,
                id: id.to_string(),
                name: None,
                compose_project: None,
            });
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_docker_v2() {
        let path = "system.slice/docker-3f2a9c1b8e7d6f5a4b3c2d1e0f9a8b7c6d5e4f3a2b1c0d9e8f7a6b5c4d3e2f1a.scope";
        let container = parse_cgroup_path(path).unwrap();
        assert_eq!(container.runtime, Runtime::Docker);
        assert!(container.id.starts_with("3f2a9c"));
    }

    #[test]
    fn test_parse_docker_v1() {
        let path = "/docker/3f2a9c1b8e7d6f5a4b3c2d1e0f9a8b7c6d5e4f3a2b1c0d9e8f7a6b5c4d3e2f1a";
        let container = parse_cgroup_path(path).unwrap();
        assert_eq!(container.runtime, Runtime::Docker);
    }

    #[test]
    fn test_parse_podman() {
        let path = "system.slice/libpod-abc123.scope";
        let container = parse_cgroup_path(path).unwrap();
        assert_eq!(container.runtime, Runtime::Podman);
    }
}
