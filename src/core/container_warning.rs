use crate::model::Degradation;
use std::path::Path;

/// Heuristic for I3: detect whether a container runtime is active, and whether
/// its userland-proxy is likely disabled, so its published ports may be hidden
/// from `/proc/net`.
///
/// This is intentionally lightweight and best-effort:
/// - inspect `/proc/<pid>/cgroup` for docker/podman/containerd scopes
/// - check for common proxy binaries via `cmdline`
/// - when we find runtime processes but no `docker-proxy` sibling, emit a
///   `Degradation::ContainerPortsHidden`
///
/// We do not read `/proc/<pid>/environ` or call out to any daemon.
pub fn detect_container_warning(proc_root: &Path) -> Option<Degradation> {
    let has_docker_proxy = running_docker_proxies(proc_root);
    let runtimes = active_runtimes(proc_root);
    if runtimes.is_empty() {
        return None;
    }

    let msg = if runtimes.len() == 1 && runtimes[0] == "docker" && !has_docker_proxy {
        format!(
            "{} active without docker-proxy; published ports may be hidden.",
            runtimes.join(", ")
        )
    } else {
        format!(
            "{} active; userland-proxy may be disabled for some published ports.",
            runtimes.join(", ")
        )
    };

    Some(Degradation::ContainerPortsHidden(msg))
}

/// Best-effort detection of common container runtimes.
///
/// This is a static snapshot at scan time. It only needs to be right often
/// enough to show a useful status-bar hint; a false negative is acceptable.
fn active_runtimes(proc_root: &Path) -> Vec<&'static str> {
    let mut seen = Vec::new();

    if let Ok(entries) = std::fs::read_dir(proc_root) {
        for entry in entries.flatten() {
            let pid_str = entry.file_name().to_string_lossy().into_owned();
            let Ok(pid) = pid_str.parse::<i32>() else {
                continue;
            };

            let cgroup = proc_root.join(pid.to_string()).join("cgroup");
            let Ok(data) = std::fs::read_to_string(cgroup) else {
                continue;
            };

            for line in data.lines() {
                let lowered = line.to_ascii_lowercase();
                if lowered.contains("docker") && !seen.contains(&"docker") {
                    seen.push("docker");
                }
                if lowered.contains("libpod")
                    && !seen.contains(&"podman")
                    && !seen.contains(&"docker")
                {
                    seen.push("podman");
                }
                if lowered.contains("cri-containerd")
                    && !seen.contains(&"containerd")
                    && !seen.contains(&"docker")
                {
                    seen.push("containerd");
                }
            }
        }
    }

    seen
}

/// Return true if any `docker-proxy` process is currently visible.
fn running_docker_proxies(proc_root: &Path) -> bool {
    let Ok(entries) = std::fs::read_dir(proc_root) else {
        return false;
    };
    for entry in entries.flatten() {
        let Ok(pid) = entry.file_name().to_string_lossy().parse::<i32>() else {
            continue;
        };

        let cmdline = proc_root.join(pid.to_string()).join("cmdline");
        let Ok(data) = std::fs::read(cmdline) else {
            continue;
        };
        if data
            .split(|b| *b == 0)
            .any(|arg| arg == b"docker-proxy" || arg == b"/usr/bin/docker-proxy")
        {
            return true;
        }
    }

    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn write_file(dir: &std::path::Path, rel: &str, bytes: &[u8]) {
        let p = dir.join(rel);
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent).ok();
        }
        std::fs::write(p, bytes).ok();
    }

    #[test]
    fn test_no_runtimes_returns_none() {
        let tmp = TempDir::new().unwrap();
        assert!(detect_container_warning(tmp.path()).is_none());
    }

    #[test]
    fn test_docker_without_proxy_returns_degradation() {
        let tmp = TempDir::new().unwrap();
        write_file(
            tmp.path(),
            "1234/cmdline",
            b"/usr/bin/dockerd\x00--host=unix:///var/run/docker.sock\x00",
        );
        write_file(
            tmp.path(),
            "1234/cgroup",
            b"0::/system.slice/docker-abc123.scope\n",
        );
        let deg = detect_container_warning(tmp.path());
        assert!(matches!(deg, Some(Degradation::ContainerPortsHidden(_))));
        let msg = deg.unwrap().message();
        assert!(msg.contains("docker"));
        assert!(msg.contains("hidden"));
    }

    #[test]
    fn test_docker_with_proxy_returns_degradation_without_hidden() {
        let tmp = TempDir::new().unwrap();
        write_file(tmp.path(), "1234/cmdline", b"/usr/bin/dockerd\x00");
        write_file(
            tmp.path(),
            "1234/cgroup",
            b"0::/system.slice/docker-abc123.scope\n",
        );
        write_file(
            tmp.path(),
            "2000/cmdline",
            b"/usr/bin/docker-proxy\x00-proto tcp\x00-host-port 5432\x00",
        );
        write_file(
            tmp.path(),
            "2000/cgroup",
            b"0::/system.slice/docker-proxy-abc123.scope\n",
        );
        let deg = detect_container_warning(tmp.path());
        assert!(matches!(deg, Some(Degradation::ContainerPortsHidden(_))));
        let msg = deg.unwrap().message();
        assert!(msg.contains("docker"));
        assert!(msg.contains("userland-proxy"));
    }

    #[test]
    fn test_podman_without_proxy_returns_degradation() {
        let tmp = TempDir::new().unwrap();
        write_file(
            tmp.path(),
            "1234/cmdline",
            b"/usr/bin/conmon\x00--namespace container-abc123\x00",
        );
        write_file(
            tmp.path(),
            "1234/cgroup",
            b"0::/system.slice/libpod-abc123.scope\n",
        );
        let deg = detect_container_warning(tmp.path());
        assert!(matches!(deg, Some(Degradation::ContainerPortsHidden(_))));
        let msg = deg.unwrap().message();
        assert!(msg.contains("podman"));
    }

    #[test]
    fn test_containerd_without_proxy_returns_degradation() {
        let tmp = TempDir::new().unwrap();
        write_file(tmp.path(), "1234/cmdline", b"/usr/bin/containerd\x00");
        write_file(
            tmp.path(),
            "1234/cgroup",
            b"0::/system.slice/cri-containerd-abc123.scope\n",
        );
        let deg = detect_container_warning(tmp.path());
        assert!(matches!(deg, Some(Degradation::ContainerPortsHidden(_))));
        let msg = deg.unwrap().message();
        assert!(msg.contains("containerd"));
    }
}
