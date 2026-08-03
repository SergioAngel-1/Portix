use crate::cache::store::CpuSample;
use crate::cache::CacheStore;
use crate::core::docker_proxy::rewrite_docker_proxy;
use crate::core::Config;
use crate::detect::project::{detect_project, manifest_mtime};
use crate::detect::DetectorRegistry;
use crate::model::{
    Degradation, Family, Process, ProcessKey, Service, ServiceId, SharedSnapshot, Signal, Snapshot,
    Socket,
};
use crate::platform::{LinuxProbe, PlatformProbe};
use anyhow::Result;
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::Arc;
use std::time::Instant;

/// The core engine that orchestrates scanning, detection, and caching.
pub struct Engine {
    platform: Box<dyn PlatformProbe>,
    cache: CacheStore,
    registry: DetectorRegistry,
    config: Config,
    current_uid: u32,
    num_cpus: usize,
}

impl Engine {
    /// Create a new engine with the given proc root.
    pub fn new(proc_root: Option<&Path>, config: Config) -> Result<Self> {
        let platform = Box::new(LinuxProbe::new(proc_root));
        let current_uid = platform.current_uid();

        Ok(Self {
            platform,
            cache: CacheStore::new(),
            registry: DetectorRegistry::new(),
            config,
            current_uid,
            num_cpus: crate::platform::linux::metrics::get_num_cpus(),
        })
    }

    /// Signal the process behind a service, refusing if the PID was recycled.
    pub fn signal_service(&self, service: &Service, sig: Signal) -> Result<()> {
        let process = service
            .process
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("no process information for this service"))?;

        self.platform
            .signal(process.pid, process.starttime_jiffies, sig)
    }

    /// Perform a full scan and return a snapshot.
    pub fn scan(&mut self) -> Result<SharedSnapshot> {
        let start = Instant::now();
        let mut degradations = Vec::new();

        let sockets = match self.platform.list_listening_sockets() {
            Ok(s) => s,
            Err(e) => return Ok(Arc::new(self.empty_snapshot(start, e))),
        };

        let sockets: Vec<Socket> = sockets
            .into_iter()
            .filter(|s| self.socket_passes_filters(s))
            .collect();

        let interest: HashSet<u64> = sockets.iter().map(|s| s.inode).collect();
        let inode_map = crate::platform::linux::fd::build_inode_map(
            self.platform.proc_root(),
            &interest,
            self.current_uid,
        )
        .unwrap_or_default();

        let total_jiffies = self.platform.total_jiffies();
        let mut live = HashSet::new();
        let mut services = Vec::new();
        let mut unresolved = 0usize;
        // Sockets whose PID we found but whose /proc metadata we could not read
        // (another user's process, or it exited mid-scan). Counted per distinct
        // PID via the `seen` memo below, not per socket.
        let mut partial_reads = 0usize;

        // One process can own several sockets (dual-stack, or tcp+udp like
        // kdeconnectd). Resolve it once per scan: reading /proc twice is waste,
        // and sampling CPU twice compares a counter against itself and yields no
        // delta at all for the second socket.
        let mut seen: HashMap<i32, Option<Process>> = HashMap::new();
        // Container membership is per-process; project/framework is per (pid,
        // port). A dual-stack service is two sockets on one port, so without
        // these memos it re-reads /proc/<pid>/cgroup and re-walks manifests twice.
        let mut seen_container: HashMap<i32, Option<crate::model::Container>> = HashMap::new();
        type Context = (
            Option<crate::model::Project>,
            Option<crate::model::Framework>,
        );
        let mut seen_context: HashMap<(i32, u16), Context> = HashMap::new();

        for socket in sockets {
            let pid = inode_map
                .get(&socket.inode)
                .and_then(|v| v.first())
                .map(|&(pid, _)| pid);

            let process = match pid {
                Some(pid) => match seen.get(&pid) {
                    Some(cached) => cached.clone(),
                    None => {
                        let resolved = self.resolve_process(pid, total_jiffies, &mut live);
                        // A known PID we could not read is a partial read, not an
                        // unattributed socket: the difference is worth reporting.
                        if resolved.is_none() {
                            partial_reads += 1;
                        }
                        seen.insert(pid, resolved.clone());
                        resolved
                    }
                },
                None => {
                    unresolved += 1;
                    None
                }
            };

            let container = match pid {
                Some(pid) => match seen_container.get(&pid) {
                    Some(cached) => cached.clone(),
                    None => {
                        let c = self.platform.container_of(pid);
                        seen_container.insert(pid, c.clone());
                        c
                    }
                },
                None => None,
            };

            let (project, framework) = match (&process, pid) {
                (Some(p), Some(pid)) => {
                    let ctx_key = (pid, socket.port);
                    match seen_context.get(&ctx_key) {
                        Some(cached) => cached.clone(),
                        None => {
                            let ctx = self.detect_context(p, socket.port);
                            seen_context.insert(ctx_key, ctx.clone());
                            ctx
                        }
                    }
                }
                _ => (None, None),
            };

            services.push(Service {
                id: ServiceId::from_socket(&socket),
                socket,
                process,
                project,
                framework,
                container,
                docker_proxy_rewritten: false,
            });

            rewrite_docker_proxy(services.last_mut().unwrap());
        }

        if unresolved > 0 {
            degradations.push(Degradation::SocketsWithoutPid(unresolved));
        }
        if partial_reads > 0 {
            degradations.push(Degradation::ProcessesPartialRead(partial_reads));
        }

        self.cache.retain_live(&live);

        let mut services = merge_dual_stack(services);
        services.sort_by_key(|s| s.socket.port);

        Ok(Arc::new(Snapshot {
            services,
            taken_at: start,
            taken_at_epoch_secs: epoch_secs(),
            scan_duration_ms: start.elapsed().as_millis() as u64,
            degradations,
        }))
    }

    fn empty_snapshot(&self, start: Instant, error: anyhow::Error) -> Snapshot {
        Snapshot {
            services: Vec::new(),
            taken_at: start,
            taken_at_epoch_secs: epoch_secs(),
            scan_duration_ms: start.elapsed().as_millis() as u64,
            degradations: vec![Degradation::ScanFailed(format!("{error:#}"))],
        }
    }

    fn socket_passes_filters(&self, socket: &Socket) -> bool {
        if !self.config.show_udp && socket.protocol.family() == Family::Udp {
            return false;
        }
        if !self.config.show_system_ports && socket.is_system_port() {
            return false;
        }
        if self.config.show_loopback_only && !socket.is_loopback() {
            return false;
        }
        true
    }

    /// Read a process fresh from `/proc`, then attach its CPU delta from the
    /// previous sample. Metadata is not cached: `cmdline`/`exe`/`cwd` change on
    /// `exec`/`chdir` while `(pid, starttime)` stays the same, so a cached copy
    /// would freeze to stale values for the process's whole life.
    fn resolve_process(
        &mut self,
        pid: i32,
        total_jiffies: Option<u64>,
        live: &mut HashSet<ProcessKey>,
    ) -> Option<Process> {
        // A process can exit between the readdir that found it and this read.
        // That is the normal case, not an error.
        let mut process = self.platform.process_metadata(pid).ok()?;
        let key = process.key();
        live.insert(key);

        if let Some(started) = process.started_at {
            process.metrics.uptime = started.elapsed().ok();
        }

        if let Some(total) = total_jiffies {
            let current = CpuSample {
                proc_jiffies: process.cpu_jiffies,
                total_jiffies: total,
            };
            // First sight of a process yields None: there is no delta yet.
            if let Some(prev) = self.cache.swap_cpu_sample(key, current) {
                process.metrics.cpu_percent =
                    crate::platform::linux::metrics::calculate_cpu_percent(
                        prev.proc_jiffies,
                        prev.total_jiffies,
                        current.proc_jiffies,
                        current.total_jiffies,
                        self.cpu_scale_factor(),
                    );
            }
        }

        Some(process)
    }

    fn cpu_scale_factor(&self) -> usize {
        if self.config.cpu_scale == "core" {
            self.num_cpus
        } else {
            1
        }
    }

    /// Resolve project and framework for a process, memoizing both.
    fn detect_context(
        &mut self,
        process: &Process,
        port: u16,
    ) -> (
        Option<crate::model::Project>,
        Option<crate::model::Framework>,
    ) {
        let Some(cwd) = process.cwd.as_deref() else {
            return (None, None);
        };

        let project = match crate::cache::keys::inode(cwd) {
            Some(cwd_inode) => match self.cache.get_project(cwd_inode) {
                Some(cached) => cached.clone(),
                None => {
                    let project = detect_project(cwd);
                    self.cache.insert_project(cwd_inode, project.clone());
                    project
                }
            },
            None => detect_project(cwd),
        };

        let root = project.as_ref().map(|p| p.root.clone());
        let framework = self.detect_framework_cached(root.as_deref(), process, port);

        (project, framework)
    }

    fn detect_framework_cached(
        &mut self,
        root: Option<&Path>,
        process: &Process,
        port: u16,
    ) -> Option<crate::model::Framework> {
        let detect = |registry: &DetectorRegistry| {
            registry.detect(root, process.exe.as_deref(), &process.cmdline, port)
        };

        // Only project-rooted detection is cacheable: it is the half that reads
        // manifests. The key carries the manifest mtime, so `pnpm add` invalidates.
        let Some(root) = root else {
            return detect(&self.registry);
        };
        let (Some(root_inode), Some(mtime)) =
            (crate::cache::keys::inode(root), manifest_mtime(root))
        else {
            return detect(&self.registry);
        };

        // Detection also depends on the process inputs, not just the manifest.
        // Fold them into the key so two processes under one root but with
        // different ports/commands don't read each other's cached framework.
        let disc = {
            use std::hash::{Hash, Hasher};
            let mut hasher = std::collections::hash_map::DefaultHasher::new();
            port.hash(&mut hasher);
            process.exe.hash(&mut hasher);
            process.cmdline.hash(&mut hasher);
            hasher.finish()
        };

        if let Some(cached) = self.cache.get_framework(root_inode, mtime, disc) {
            return cached.clone();
        }

        let framework = detect(&self.registry);
        self.cache
            .insert_framework(root_inode, mtime, disc, framework.clone());
        framework
    }
}

fn epoch_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Collapse the two kernel sockets a dual-stack service produces into one row.
///
/// A server that binds `0.0.0.0` and `::` (or a `::` socket with `IPV6_V6ONLY=0`)
/// appears twice in `/proc/net/*`. Showing both makes `next dev` look like two
/// services and makes the tool look broken. Identity is (pid, port, family).
fn merge_dual_stack(services: Vec<Service>) -> Vec<Service> {
    let mut by_identity: HashMap<(Option<i32>, u16, Family), usize> = HashMap::new();
    let mut out: Vec<Service> = Vec::with_capacity(services.len());

    for service in services {
        let pid = service.process.as_ref().map(|p| p.pid);
        let identity = (pid, service.socket.port, service.socket.protocol.family());

        // Sockets we could not attribute to a process are never merged: two
        // unknown listeners on the same port may be genuinely different things.
        if pid.is_none() {
            out.push(service);
            continue;
        }

        match by_identity.get(&identity) {
            Some(&existing) => {
                let kept: &mut Service = &mut out[existing];
                if kept.socket.protocol.is_v6() != service.socket.protocol.is_v6() {
                    kept.socket.dual_stack = true;
                }
                // Prefer the IPv6 socket as the survivor: it is the one a modern
                // runtime actually binds, and the one whose scope is widest.
                if service.socket.protocol.is_v6() && !kept.socket.protocol.is_v6() {
                    let dual = kept.socket.dual_stack;
                    *kept = service;
                    kept.socket.dual_stack = dual;
                }
            }
            None => {
                by_identity.insert(identity, out.len());
                out.push(service);
            }
        }
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{BindScope, Protocol, TcpState};

    fn socket(protocol: Protocol, port: u16, inode: u64) -> Socket {
        Socket {
            protocol,
            local_addr: if protocol.is_v6() {
                std::net::Ipv6Addr::UNSPECIFIED.into()
            } else {
                std::net::Ipv4Addr::UNSPECIFIED.into()
            },
            port,
            state: TcpState::Listen,
            bind_scope: BindScope::AllInterfaces,
            inode,
            uid: 1000,
            dual_stack: false,
        }
    }

    fn service(protocol: Protocol, port: u16, pid: Option<i32>) -> Service {
        let sock = socket(protocol, port, port as u64);
        Service {
            id: ServiceId::from_socket(&sock),
            socket: sock,
            process: pid.map(|pid| Process {
                pid,
                ppid: 1,
                name: "node".into(),
                cmdline: vec!["node".into()],
                exe: None,
                cwd: None,
                user: None,
                started_at: None,
                starttime_jiffies: 1,
                cpu_jiffies: 0,
                metrics: Default::default(),
                state: crate::model::ProcessState::Sleeping,
                threads: 1,
            }),
            project: None,
            framework: None,
            container: None,
            docker_proxy_rewritten: false,
        }
    }

    #[test]
    fn test_dual_stack_sockets_collapse_into_one_row() {
        let merged = merge_dual_stack(vec![
            service(Protocol::Tcp, 3000, Some(48213)),
            service(Protocol::Tcp6, 3000, Some(48213)),
        ]);

        assert_eq!(merged.len(), 1);
        assert!(merged[0].socket.dual_stack);
        assert!(merged[0].socket.protocol.is_v6());
    }

    #[test]
    fn test_same_port_different_processes_are_not_merged() {
        // SO_REUSEPORT workers, or two containers: genuinely different services.
        let merged = merge_dual_stack(vec![
            service(Protocol::Tcp, 3000, Some(1)),
            service(Protocol::Tcp, 3000, Some(2)),
        ]);
        assert_eq!(merged.len(), 2);
    }

    #[test]
    fn test_tcp_and_udp_on_same_port_are_not_merged() {
        let merged = merge_dual_stack(vec![
            service(Protocol::Tcp, 1716, Some(1575)),
            service(Protocol::Udp6, 1716, Some(1575)),
        ]);
        assert_eq!(merged.len(), 2);
    }

    #[test]
    fn test_unattributed_sockets_are_never_merged() {
        let merged = merge_dual_stack(vec![
            service(Protocol::Tcp, 53, None),
            service(Protocol::Tcp, 53, None),
        ]);
        assert_eq!(merged.len(), 2);
    }
}
