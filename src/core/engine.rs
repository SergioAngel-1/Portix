use crate::cache::store::CpuSample;
use crate::cache::CacheStore;
use crate::core::docker_proxy::{rewrite_docker_proxy, DockerProxyInfo};
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
    num_cpus: usize,
    /// Last container-warning result, recomputed at most once per 10 s: the
    /// heuristic walks all of `/proc` (cgroup + cmdline of every pid) and is
    /// far too expensive to run on every refresh.
    container_warning: Option<(Instant, Option<Degradation>)>,
}

impl Engine {
    /// Create a new engine with the given proc root.
    ///
    /// Infallible: the probe construction and uid resolution never fail on the
    /// Linux backend, so callers get an `Engine` straight away.
    pub fn new(proc_root: Option<&Path>, config: Config) -> Self {
        let platform = Box::new(LinuxProbe::new(proc_root));
        let num_cpus = platform.num_cpus();

        Self {
            platform,
            cache: CacheStore::new(),
            registry: DetectorRegistry::new(),
            config,
            num_cpus,
            container_warning: None,
        }
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

    /// Resolve the service currently listening on `port` from a fresh scan.
    ///
    /// This is the only safe way to turn a port into a kill target: the
    /// snapshot the UI holds can be seconds old, and the process behind a port
    /// may have exited and been replaced in that window.
    ///
    /// The scan ignores the display filters (`show_system_ports`,
    /// `show_loopback_only`, `show_udp`): this is the CLI kill path, which
    /// asks a direct question about a port — a filter meant for the table must
    /// not hide the kill target. (The TUI kill path goes through
    /// [`Self::service_on_inode`] instead, which keeps the filters: a row the
    /// user can select is by definition visible.)
    pub fn service_on_port(&mut self, port: u16) -> Option<Service> {
        let snapshot = self.scan_unfiltered();
        snapshot.by_port(port).cloned()
    }

    /// Resolve the service owning a specific socket inode from a fresh scan.
    ///
    /// The inode is the identity of the exact row the user selected: with
    /// several processes on one port (SO_REUSEPORT workers, containers), a
    /// port alone cannot say *which* row. The inode exists only while that
    /// socket does — a rebind opens a new inode and resolves to None, which
    /// is precisely the safe direction for a kill.
    pub fn service_on_inode(&mut self, inode: u64) -> Option<Service> {
        let snapshot = self.scan();
        snapshot.by_inode(inode).cloned()
    }

    /// Perform a full scan and return a snapshot.
    ///
    /// Infallible by design: every failure mode (unreadable `/proc/net`,
    /// vanishing processes, permission errors) degrades into the snapshot's
    /// `degradations` instead of erroring, so the UI always has a frame to
    /// draw. `Degradation::ScanFailed` is the "the scan itself failed" signal.
    pub fn scan(&mut self) -> SharedSnapshot {
        self.scan_inner(true)
    }

    /// Like [`Self::scan`], but ignoring the display filters
    /// (`show_system_ports`, `show_loopback_only`, `show_udp`).
    ///
    /// Used by the CLI query paths (`portix 80`, `portix --json 80`,
    /// `portix --kill 80`): those answer a direct question about a specific
    /// port, and a filter that exists to keep the table readable must not make
    /// the tool report "no service" for a port that is actually listening
    /// (which would e.g. make `portix :80 || pnpm dev` start a server on a
    /// taken port).
    pub fn scan_unfiltered(&mut self) -> SharedSnapshot {
        self.scan_inner(false)
    }

    fn scan_inner(&mut self, apply_filters: bool) -> SharedSnapshot {
        let start = Instant::now();
        let mut degradations = Vec::new();

        let sockets = match self.platform.list_listening_sockets() {
            Ok(s) => s,
            Err(e) => return Arc::new(self.empty_snapshot(start, e)),
        };

        let sockets: Vec<Socket> = if apply_filters {
            sockets
                .into_iter()
                .filter(|s| self.socket_passes_filters(s))
                .collect()
        } else {
            sockets
        };

        let interest: HashSet<u64> = sockets.iter().map(|s| s.inode).collect();
        let inode_map = self.platform.build_inode_map(&interest).unwrap_or_default();

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

            // A docker-proxy row is rewritten into its destination container
            // moments below, discarding project and framework. The proxy's
            // own cwd is `/` — context detection would walk the tree for
            // nothing, so skip it for proxy processes.
            let is_docker_proxy = process
                .as_ref()
                .is_some_and(|p| DockerProxyInfo::from_cmdline(&p.cmdline).is_some());

            let (project, framework) = match (&process, pid) {
                (Some(p), Some(pid)) if !is_docker_proxy => {
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

            let mut service = Service {
                id: ServiceId::from_socket(&socket),
                socket,
                process,
                project,
                framework,
                container,
                docker_proxy_rewritten: false,
            };
            rewrite_docker_proxy(&mut service);
            services.push(service);
        }

        if let Some(deg) = self.container_warning_cached() {
            degradations.push(deg);
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

        Arc::new(Snapshot {
            services,
            taken_at_epoch_secs: epoch_secs(),
            scan_duration_ms: start.elapsed().as_millis() as u64,
            degradations,
        })
    }

    fn empty_snapshot(&self, start: Instant, error: anyhow::Error) -> Snapshot {
        Snapshot {
            services: Vec::new(),
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

    /// The container warning, recomputed at most once per 10 s.
    ///
    /// `platform::container_warning` walks all of `/proc` (one cgroup and one
    /// cmdline read per pid); running that on every refresh would blow the
    /// hot-refresh budget. Ten seconds is plenty of granularity for a
    /// status-bar hint.
    fn container_warning_cached(&mut self) -> Option<Degradation> {
        const TTL: std::time::Duration = std::time::Duration::from_secs(10);
        if let Some((at, deg)) = &self.container_warning {
            if at.elapsed() < TTL {
                return deg.clone();
            }
        }
        let deg = self.platform.container_warning();
        self.container_warning = Some((Instant::now(), deg.clone()));
        deg
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

        // L4: uid -> username, cached (300 s TTL). `getpwuid_r` is an NSS
        // call; on LDAP/SSSD boxes a per-process-per-scan lookup is real
        // latency, so resolve through the cache and only miss once per uid.
        if let Some(uid) = process.uid {
            process.user = match self.cache.get_user(uid) {
                Some(name) => Some(name.clone()),
                None => {
                    let name = self.platform.username_for_uid(uid);
                    if let Some(ref n) = name {
                        self.cache.insert_user(uid, n.clone());
                    }
                    name
                }
            };
        }

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
                process.metrics.cpu_percent = crate::core::cpu::calculate_cpu_percent(
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
/// services and makes the tool look broken. The pair is (pid, port, family)
/// with one IPv4 and one IPv6 member; the IPv6 socket survives.
///
/// Two sockets of the *same* IP version on one port and pid (SO_REUSEPORT
/// workers, or explicit binds like 127.0.0.1 + 0.0.0.0) are genuinely distinct
/// listeners and are never merged.
///
/// Each key parks a *queue* of rows per IP version, so several v4+v6 pairs on
/// one port (127.0.0.1 + 0.0.0.0 plus [::1] + [::]) each merge independently.
/// The old single-slot parking left the second pair as two orphan rows
/// whenever `/proc` listed both v4s before both v6s — its natural file order.
fn merge_dual_stack(services: Vec<Service>) -> Vec<Service> {
    use std::collections::VecDeque;
    // One queue of parked row indices per identity per IP version: the first
    // of each version waits here to be paired with the other version;
    // same-version duplicates beyond the first keep their own rows.
    let mut v4_queue: HashMap<(i32, u16, Family), VecDeque<usize>> = HashMap::new();
    let mut v6_queue: HashMap<(i32, u16, Family), VecDeque<usize>> = HashMap::new();
    let mut out: Vec<Service> = Vec::with_capacity(services.len());

    for service in services {
        // Sockets we could not attribute to a process are never merged: two
        // unknown listeners on the same port may be genuinely different things.
        let Some(pid) = service.process.as_ref().map(|p| p.pid) else {
            out.push(service);
            continue;
        };
        let key = (pid, service.socket.port, service.socket.protocol.family());

        if service.socket.protocol.is_v6() {
            // Pair with a parked v4 row: the pair becomes one dual-stack row
            // with the v6 as survivor.
            if let Some(idx) = v4_queue.get_mut(&key).and_then(VecDeque::pop_front) {
                out[idx] = service;
                out[idx].socket.dual_stack = true;
            } else {
                v6_queue.entry(key).or_default().push_back(out.len());
                out.push(service);
            }
        } else {
            // v4: attach to a parked v6 row if one is waiting.
            if let Some(idx) = v6_queue.get_mut(&key).and_then(VecDeque::pop_front) {
                out[idx].socket.dual_stack = true;
            } else {
                v4_queue.entry(key).or_default().push_back(out.len());
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
                uid: None,
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

    /// The v6 socket may be listed before the v4 in `/proc/net/*` — order
    /// must not matter: the pair still collapses into one dual-stack row.
    #[test]
    fn test_dual_stack_merges_regardless_of_order() {
        let merged = merge_dual_stack(vec![
            service(Protocol::Tcp6, 3000, Some(1)),
            service(Protocol::Tcp, 3000, Some(1)),
        ]);

        assert_eq!(merged.len(), 1);
        assert!(merged[0].socket.dual_stack);
        assert!(merged[0].socket.protocol.is_v6());
    }

    /// v4 + v6 + another v4 on the same port: the v4+v6 pair collapses, and
    /// the second v4 (a distinct listener, e.g. 127.0.0.1 beside 0.0.0.0)
    /// keeps its own row.
    #[test]
    fn test_third_same_port_socket_stays_separate() {
        let merged = merge_dual_stack(vec![
            service(Protocol::Tcp, 3000, Some(1)),
            service(Protocol::Tcp6, 3000, Some(1)),
            service(Protocol::Tcp, 3000, Some(1)),
        ]);

        assert_eq!(merged.len(), 2);
        let dual = merged.iter().filter(|s| s.socket.dual_stack).count();
        assert_eq!(dual, 1, "exactly one row is the merged pair");
        let v6 = merged.iter().filter(|s| s.socket.protocol.is_v6()).count();
        assert_eq!(v6, 1, "the merged pair's survivor is the v6 socket");
    }

    /// Two sockets of the same IP version on one port and pid are distinct
    /// listeners (SO_REUSEPORT, or 127.0.0.1 + 0.0.0.0 in one process). The
    /// merge exists for v4+v6 dual-stack pairs only.
    #[test]
    fn test_same_version_sockets_on_one_port_are_not_merged() {
        let merged = merge_dual_stack(vec![
            service(Protocol::Tcp, 3000, Some(1)),
            service(Protocol::Tcp, 3000, Some(1)),
        ]);
        assert_eq!(merged.len(), 2);
    }

    /// Four sockets on one port — two v4 (127.0.0.1 + 0.0.0.0) and two v6
    /// ([::1] + [::]) — are two dual-stack pairs, not one pair plus orphans.
    /// `/proc` lists both v4s before both v6s, which is exactly the order that
    /// stranded the second pair under the old single-slot parking.
    #[test]
    fn test_two_dual_stack_pairs_merge_independently() {
        let merged = merge_dual_stack(vec![
            service(Protocol::Tcp, 3000, Some(1)),
            service(Protocol::Tcp, 3000, Some(1)),
            service(Protocol::Tcp6, 3000, Some(1)),
            service(Protocol::Tcp6, 3000, Some(1)),
        ]);

        assert_eq!(merged.len(), 2);
        assert!(
            merged.iter().all(|s| s.socket.dual_stack),
            "both rows must be dual-stack pairs"
        );
        assert!(
            merged.iter().all(|s| s.socket.protocol.is_v6()),
            "the survivor of each pair is the v6 socket"
        );
    }

    fn engine_with(config: Config) -> Engine {
        Engine::new(Some(tempfile::tempdir().unwrap().path()), config)
    }

    #[test]
    fn test_filters_hide_udp_sockets_when_disabled() {
        let engine = engine_with(Config {
            show_udp: false,
            ..Config::default()
        });
        assert!(!engine.socket_passes_filters(&socket(Protocol::Udp, 5353, 1)));
        assert!(engine.socket_passes_filters(&socket(Protocol::Tcp, 5353, 2)));
    }

    #[test]
    fn test_filters_hide_system_ports_unless_requested() {
        let engine = engine_with(Config::default()); // show_system_ports = false
        assert!(!engine.socket_passes_filters(&socket(Protocol::Tcp, 80, 1)));
        assert!(engine.socket_passes_filters(&socket(Protocol::Tcp, 8080, 2)));

        let engine = engine_with(Config {
            show_system_ports: true,
            ..Config::default()
        });
        assert!(engine.socket_passes_filters(&socket(Protocol::Tcp, 80, 3)));
    }

    #[test]
    fn test_loopback_only_filters_non_loopback_binds() {
        let engine = engine_with(Config {
            show_loopback_only: true,
            ..Config::default()
        });
        // The helper socket binds AllInterfaces: filtered out.
        assert!(!engine.socket_passes_filters(&socket(Protocol::Tcp, 3000, 1)));

        let mut loopback = socket(Protocol::Tcp, 3000, 2);
        loopback.local_addr = std::net::Ipv4Addr::LOCALHOST.into();
        loopback.bind_scope = crate::model::BindScope::Loopback;
        assert!(engine.socket_passes_filters(&loopback));
    }
}
