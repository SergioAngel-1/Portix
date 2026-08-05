use crate::model::{Container, Runtime};
use std::path::Path;

/// Detect if a process is running in a container by reading its cgroup.
pub fn container_of(proc_root: &Path, pid: i32) -> Option<Container> {
    let cgroup_path = proc_root.join(pid.to_string()).join("cgroup");
    let content = std::fs::read_to_string(&cgroup_path).ok()?;
    content.lines().find_map(parse_cgroup_line)
}

/// Parse a single line of `/proc/<pid>/cgroup` into a container, if any.
///
/// Handles both forms:
/// - v2: `0::/system.slice/docker-<id>.scope`
/// - v1: `1:cpu,cpuacct:/docker/<id>`
///
/// `pub(crate)` so the container warning heuristic (`core/container_warning.rs`)
/// reuses the exact same structured parsing instead of substring matching.
pub(crate) fn parse_cgroup_line(line: &str) -> Option<Container> {
    // Try v2 format: "0::/system.slice/docker-..."
    if let Some(rest) = line.strip_prefix("0::") {
        if let Some(container) = parse_cgroup_path(rest) {
            return Some(container);
        }
    }

    // Try v1 format: "hierarchy:subsystem-list:cgroup-path"
    if let Some(rest) = line.splitn(3, ':').nth(2) {
        if let Some(container) = parse_cgroup_path(rest) {
            return Some(container);
        }
    }

    None
}

/// Parse a cgroup path to extract container info.
///
/// Real paths carry a leading slash on both v1 (`/docker/<id>`) and v2
/// (`/system.slice/docker-<id>.scope`) lines — verified against live kernels.
/// Normalize once so every branch below matches either spelling.
fn parse_cgroup_path(path: &str) -> Option<Container> {
    let path = path.strip_prefix('/').unwrap_or(path);

    // Docker v2 (systemd): docker-<64hex>.scope
    if let Some(rest) = path.strip_prefix("system.slice/docker-") {
        if let Some(id) = rest.strip_suffix(".scope").filter(|id| is_hex_id(id)) {
            return Some(Container::new(Runtime::Docker, id.to_string()));
        }
    }

    // Docker v1: /docker/<64hex>
    if let Some(rest) = path.strip_prefix("docker/") {
        let id = rest.trim_end_matches('/');
        if is_hex_id(id) {
            return Some(Container::new(Runtime::Docker, id.to_string()));
        }
    }

    // Podman v1: /libpod/<64hex>
    if let Some(rest) = path.strip_prefix("libpod/") {
        let id = rest.trim_end_matches('/');
        if is_hex_id(id) {
            return Some(Container::new(Runtime::Podman, id.to_string()));
        }
    }

    // containerd v1: /cri-containerd/<64hex>
    if let Some(rest) = path.strip_prefix("cri-containerd/") {
        let id = rest.trim_end_matches('/');
        if is_hex_id(id) {
            return Some(Container::new(Runtime::Containerd, id.to_string()));
        }
    }

    // cgroup v2 scope forms. Matching the leaf scope name covers every
    // hierarchy the runtime drivers use: `system.slice/` (rootful systemd),
    // `machine.slice/` (podman rootful v2), `user.slice/user-<uid>.slice/…`
    // (rootless docker and podman), and `kubepods.slice/…` (Kubernetes).
    if let Some(leaf) = path.rsplit('/').next() {
        if let Some(id) = leaf
            .strip_prefix("docker-")
            .and_then(|r| r.strip_suffix(".scope"))
            .filter(|id| is_hex_id(id))
        {
            return Some(Container::new(Runtime::Docker, id.to_string()));
        }
        if let Some(id) = leaf
            .strip_prefix("libpod-")
            .and_then(|r| r.strip_suffix(".scope"))
            .filter(|id| is_hex_id(id))
        {
            return Some(Container::new(Runtime::Podman, id.to_string()));
        }
        if let Some(id) = leaf
            .strip_prefix("cri-containerd-")
            .and_then(|r| r.strip_suffix(".scope"))
            .filter(|id| is_hex_id(id))
        {
            return Some(Container::new(Runtime::Containerd, id.to_string()));
        }
        // systemd-nspawn: machine-<name>.scope under machine.slice. Machine
        // names are arbitrary (no hex guard), but restricted to the
        // machine.slice hierarchy to avoid false positives elsewhere.
        if path.starts_with("machine.slice/") {
            if let Some(id) = leaf
                .strip_prefix("machine-")
                .and_then(|r| r.strip_suffix(".scope"))
                .filter(|id| !id.is_empty())
            {
                return Some(Container::new(Runtime::SystemdNspawn, id.to_string()));
            }
        }
    }

    None
}

/// Container IDs are 64 hex chars (abbreviated in tests). Requiring hex
/// rejects host-level lookalikes such as `system.slice/docker.service` (the
/// daemon's own unit) or a hypothetical `docker-proxy-…` scope, which must
/// not be attributed to a container.
fn is_hex_id(id: &str) -> bool {
    !id.is_empty() && id.chars().all(|c| c.is_ascii_hexdigit())
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

    /// cgroup v1 line: "1:cpu,cpuacct:/docker/abc123"
    /// `splitn(3, ':').nth(2)` must extract "/docker/abc123", not "cpu,cpuacct:/docker/...".
    #[test]
    fn test_parse_docker_v1_full_line() {
        let line = "1:cpu,cpuacct:/docker/3f2a9c1b8e7d6f5a4b3c2d1e0f9a8b7c6d5e4f3a2b1c0d9e8f7a6b5c4d3e2f1a";
        let rest = line.splitn(3, ':').nth(2).unwrap();
        assert!(rest.starts_with("/docker/"));
        let container = parse_cgroup_path(rest).unwrap();
        assert_eq!(container.runtime, Runtime::Docker);
    }

    /// cgroup v1 line: "2:cpu:/libpod/abc123"
    #[test]
    fn test_parse_podman_v1() {
        let line = "2:cpu:/libpod/abc123";
        let rest = line.splitn(3, ':').nth(2).unwrap();
        assert!(rest.starts_with("/libpod/"));
        let container = parse_cgroup_path(rest).unwrap();
        assert_eq!(container.runtime, Runtime::Podman);
    }

    /// cgroup v1 line: "3:cpu,cpuacct:/cri-containerd/abc123"
    #[test]
    fn test_parse_containerd_v1() {
        let line = "3:cpu,cpuacct:/cri-containerd/abc123";
        let rest = line.splitn(3, ':').nth(2).unwrap();
        assert!(rest.starts_with("/cri-containerd/"));
        let container = parse_cgroup_path(rest).unwrap();
        assert_eq!(container.runtime, Runtime::Containerd);
    }

    #[test]
    fn test_parse_podman() {
        let path = "system.slice/libpod-abc123.scope";
        let container = parse_cgroup_path(path).unwrap();
        assert_eq!(container.runtime, Runtime::Podman);
    }

    /// Real v2 lines carry a leading slash ("0::/system.slice/..."), unlike the
    /// bare-path unit tests above. This is the shape a live kernel produces.
    #[test]
    fn test_parse_v2_full_line_with_leading_slash() {
        let line = "0::/system.slice/docker-3f2a9c1b8e7d6f5a4b3c2d1e0f9a8b7c6d5e4f3a2b1c0d9e8f7a6b5c4d3e2f1a.scope";
        let container = parse_cgroup_line(line).unwrap();
        assert_eq!(container.runtime, Runtime::Docker);

        let line = "0::/machine.slice/libpod-abc123.scope";
        let container = parse_cgroup_line(line).unwrap();
        assert_eq!(container.runtime, Runtime::Podman);
    }

    /// The daemon's own systemd unit is not a container. A substring match on
    /// "docker" would fire here and warn on every box that merely runs dockerd.
    #[test]
    fn test_daemon_unit_is_not_a_container() {
        let line = "0::/system.slice/docker.service";
        assert!(parse_cgroup_line(line).is_none());
    }

    /// docker-proxy runs on the host; its scope must not be attributed to a
    /// container either (the id after `docker-` is not hex).
    #[test]
    fn test_docker_proxy_scope_is_not_a_container() {
        let line = "0::/system.slice/docker-proxy-abc123.scope";
        assert!(parse_cgroup_line(line).is_none());
    }

    /// Rootless podman (and docker) live deep under user.slice; leaf matching
    /// must still find the libpod scope.
    #[test]
    fn test_rootless_podman_v2() {
        let line = "0::/user.slice/user-1000.slice/user@1000.service/app.slice/libpod-abc123.scope";
        let container = parse_cgroup_line(line).unwrap();
        assert_eq!(container.runtime, Runtime::Podman);
    }

    /// systemd-nspawn machines live in machine.slice as machine-<name>.scope;
    /// names are arbitrary, so the branch must not demand a hex id.
    #[test]
    fn test_systemd_nspawn_machine_scope() {
        let line = "0::/machine.slice/machine-web1.scope";
        let container = parse_cgroup_line(line).unwrap();
        assert_eq!(container.runtime, Runtime::SystemdNspawn);
        assert_eq!(container.id, "web1");
    }
}
