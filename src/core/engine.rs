use crate::cache::store::CpuSample;
use crate::cache::CacheStore;
use crate::core::docker_proxy::{
    read_docker_desktop, rewrite_docker_desktop, rewrite_docker_proxy, DockerDesktopData,
    DockerProxyInfo,
};
use crate::core::{Config, CpuScale};
use crate::detect::project::{detect_project, manifest_mtime};
use crate::detect::DetectorRegistry;
use crate::model::{
    Degradation, Family, Process, ProcessKey, Protocol, Service, ServiceId, SharedSnapshot, Signal,
    Snapshot, Socket,
};
use crate::platform::{LinuxProbe, PlatformProbe, ProcessGone};
use anyhow::Result;
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// How often the inode→process cache is rebuilt from scratch.
///
/// Per-scan validation already catches a dead/recycled PID and a socket whose
/// `(port, protocol, uid)` changed. A full rebuild is the safety net for the
/// rare case where the same inode is reused for an identical-looking socket:
/// it costs one fd-table walk every five minutes, not one per second.
const INODE_REVALIDATE: Duration = Duration::from_secs(300);

/// Backoff for an inode we expected to resolve but could not (a process that
/// vanished mid-scan, or a `/proc` hidden by `hidepid`).
const INODE_MISSING_RETRY: Duration = Duration::from_secs(30);

/// Backoff for sockets owned by another UID. Their `fd` tables are unreadable
/// by this process forever, so retrying them can only burn syscalls.
const INODE_FOREIGN_RETRY: Duration = Duration::from_secs(3600);

/// How long a successful Docker Desktop container list is reused.
const DOCKER_DESKTOP_TTL: Duration = Duration::from_secs(15);

/// How long a failed/absent Docker Desktop API is not retried.
///
/// Native Linux usually has no Docker Desktop socket at all; probing it every
/// second buys nothing. A minute still discovers a Desktop that starts later.
const DOCKER_DESKTOP_NEGATIVE_TTL: Duration = Duration::from_secs(60);

/// Cached owner of a listening socket's inode.
///
/// `(pid, starttime)` is the same identity the kill path pins: a recycled PID
/// carries a different start time. `(port, protocol, uid)` identifies the
/// socket sitting on that inode, so an inode reused by a different listener is
/// detected even when the PID is still alive.
#[derive(Debug, Clone)]
struct InodeOwner {
    pid: i32,
    /// Filled in after the process metadata is read on the scan that resolved
    /// the inode; `None` means "not validated yet" and is treated as stale.
    starttime_jiffies: Option<u64>,
    port: u16,
    protocol: Protocol,
    uid: u32,
}

/// The core engine that orchestrates scanning, detection, and caching.
pub struct Engine {
    platform: Box<dyn PlatformProbe>,
    cache: CacheStore,
    registry: DetectorRegistry,
    config: Config,
    num_cpus: usize,
    /// Last container-warning result, recomputed at most once per 30 s: the
    /// heuristic walks all of `/proc` (cgroup + cmdline of every pid) and is
    /// far too expensive to run on every refresh.
    container_warning: Option<(Instant, Option<Degradation>)>,
    /// Last Docker Desktop published-port mapping, with the `Instant` it was
    /// fetched. The query talks to a Unix socket and may block for the whole
    /// API timeout; it cannot run on every refresh.
    docker_desktop: Option<(Instant, Option<DockerDesktopData>)>,
    /// Cached `socket inode → owning process`. A full `/proc/<pid>/fd` walk is
    /// O(every descriptor on the box); it only has to run when a socket we do
    /// not know about appears (or on the periodic revalidation), not on every
    /// refresh.
    inode_owners: HashMap<u64, InodeOwner>,
    /// Inodes whose resolution failed, with the `Instant` before which they
    /// must not be retried.
    inode_retry: HashMap<u64, Instant>,
    /// When the last full rebuild of `inode_owners` happened.
    last_inode_revalidate: Instant,
}

impl Engine {
    /// Create a new engine with the given proc root.
    ///
    /// Infallible: the probe construction and uid resolution never fail on the
    /// Linux backend, so callers get an `Engine` straight away.
    pub fn new(proc_root: Option<&Path>, config: Config) -> Self {
        Self::from_platform(Box::new(LinuxProbe::new(proc_root)), config)
    }

    /// Build an engine around an arbitrary probe. Kept separate so tests can
    /// inject a probe that counts fd-table walks.
    fn from_platform(platform: Box<dyn PlatformProbe>, config: Config) -> Self {
        let num_cpus = platform.num_cpus();

        Self {
            platform,
            cache: CacheStore::new(),
            registry: DetectorRegistry::new(),
            config,
            num_cpus,
            container_warning: None,
            docker_desktop: None,
            inode_owners: HashMap::new(),
            inode_retry: HashMap::new(),
            last_inode_revalidate: Instant::now(),
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
    /// may have exited and been replaced in that window. The inode cache is
    /// bypassed: a stale owner must never become a signal target.
    ///
    /// The scan ignores the display filters (`show_system_ports`,
    /// `show_loopback_only`, `show_udp`): this is the CLI kill path, which
    /// asks a direct question about a port — a filter meant for the table must
    /// not hide the kill target. (The TUI kill path goes through
    /// [`Self::service_on_inode`] instead, which keeps the filters: a row the
    /// user can select is by definition visible.)
    pub fn service_on_port(&mut self, port: u16) -> Option<Service> {
        let snapshot = self.scan_inner(false, true);
        snapshot.by_port(port).cloned()
    }

    /// Resolve the service owning a specific socket inode from a fresh scan.
    ///
    /// The inode is the identity of the exact row the user selected: with
    /// several processes on one port (SO_REUSEPORT workers, containers), a
    /// port alone cannot say *which* row. The inode exists only while that
    /// socket does — a rebind opens a new inode and resolves to None, which
    /// is precisely the safe direction for a kill.
    ///
    /// The cached inode map is deliberately bypassed: the signal target must
    /// come from a live `/proc/<pid>/fd` walk, never from a mapping that could
    /// be one refresh old.
    pub fn service_on_inode(&mut self, inode: u64) -> Option<Service> {
        let snapshot = self.scan_inner(true, true);
        snapshot.by_inode(inode).cloned()
    }

    /// Perform a full scan and return a snapshot.
    ///
    /// Infallible by design: every failure mode (unreadable `/proc/net`,
    /// vanishing processes, permission errors) degrades into the snapshot's
    /// `degradations` instead of erroring, so the UI always has a frame to
    /// draw. `Degradation::ScanFailed` is the "the scan itself failed" signal.
    pub fn scan(&mut self) -> SharedSnapshot {
        self.scan_inner(true, false)
    }

    /// A scan that rebuilds the inode→process map from scratch.
    ///
    /// Used by the user's explicit refresh action: it is the one moment where
    /// "prove the ownership cache against a live fd walk" is worth the cost,
    /// and it also makes the `r` key a reliable recovery action.
    pub fn scan_fresh(&mut self) -> SharedSnapshot {
        self.scan_inner(true, true)
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
        self.scan_inner(false, false)
    }

    fn scan_inner(&mut self, apply_filters: bool, force_full_inode_map: bool) -> SharedSnapshot {
        let start = Instant::now();
        let mut degradations = Vec::new();

        let include_udp = !apply_filters || self.config.show_udp;
        let sockets = match self.platform.list_listening_sockets(include_udp) {
            Ok(s) => s,
            Err(e) => return Arc::new(self.empty_snapshot(start, e)),
        };

        // Docker Desktop exposes the published-port mapping through its local
        // Unix socket. It is optional and best-effort: a missing/blocked API
        // must never prevent the /proc scan from producing a snapshot.
        let docker_desktop = self.docker_desktop_cached();

        let sockets: Vec<Socket> = if apply_filters {
            sockets
                .into_iter()
                .filter(|s| self.socket_passes_filters(s))
                .collect()
        } else {
            sockets
        };

        let interest: HashSet<u64> = sockets.iter().map(|s| s.inode).collect();
        // The socket's own fields are the cheap half of cache validation: an
        // inode reused by a different listener is caught without walking /proc.
        let socket_meta: HashMap<u64, (u16, Protocol, u32)> = sockets
            .iter()
            .map(|s| (s.inode, (s.port, s.protocol, s.uid)))
            .collect();

        if let Some(error) =
            self.refresh_inode_owners(&interest, &socket_meta, force_full_inode_map)
        {
            degradations.push(Degradation::InodeMapFailed(error));
        }

        let total_jiffies = self.platform.total_jiffies();
        let mut live = HashSet::new();
        let mut services = Vec::new();
        let mut unresolved = 0usize;
        // Sockets whose PID we found but whose /proc metadata we could not read
        // (another user's process, or it exited mid-scan). Counted per distinct
        // PID via the `seen` memo below, not per socket.
        let mut partial_reads = 0usize;
        let mut disappeared = 0usize;

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
            let pid = self.inode_owners.get(&socket.inode).map(|owner| owner.pid);

            let process = match pid {
                Some(pid) => match seen.get(&pid) {
                    Some(cached) => cached.clone(),
                    None => {
                        let resolved = self.resolve_process(pid, total_jiffies, &mut live);
                        // A PID found in the reverse map may disappear before metadata is read.
                        // Preserve that distinction in the degradation report instead
                        // of guessing permission. The check is on the typed error, so
                        // rewording a message cannot reclassify the whole race.
                        if let Err(error) = &resolved {
                            if error.chain().any(|cause| cause.is::<ProcessGone>()) {
                                disappeared += 1;
                            } else {
                                partial_reads += 1;
                            }
                        }
                        let process = resolved.ok();
                        seen.insert(pid, process.clone());
                        process
                    }
                },
                None => {
                    unresolved += 1;
                    None
                }
            };

            // The cache entry was created with `starttime_jiffies: None`
            // because the fd walk only knows the PID. Fill in the identity
            // now that the process metadata has been read; from the next scan
            // on, validation is a single `/proc/<pid>/stat` read.
            if let (Some(pid), Some(process)) = (pid, process.as_ref()) {
                if let Some(owner) = self.inode_owners.get_mut(&socket.inode) {
                    if owner.pid == pid {
                        owner.starttime_jiffies = Some(process.starttime_jiffies);
                    }
                }
            }

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
            if let Some(data) = docker_desktop.as_ref() {
                rewrite_docker_desktop(&mut service, data);
            }
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
        if disappeared > 0 {
            degradations.push(Degradation::ProcessesDisappeared(disappeared));
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

    /// Bring the inode→process cache up to date.
    ///
    /// A full `build_inode_map` is O(every descriptor on the box). The whole
    /// point of this cache is to pay that only when:
    /// - a socket inode we have never seen appears (a server just started),
    /// - a cached owner fails validation (PID died/recycled, socket changed),
    /// - the periodic safety revalidation fires.
    ///
    /// Returns an error string to surface as `Degradation::InodeMapFailed`
    /// when the underlying fd walk itself failed.
    fn refresh_inode_owners(
        &mut self,
        interest: &HashSet<u64>,
        socket_meta: &HashMap<u64, (u16, Protocol, u32)>,
        force_full: bool,
    ) -> Option<String> {
        let now = Instant::now();

        // Sockets that stopped listening must not keep their owner forever;
        // the same for backoff entries whose inode is gone.
        self.inode_owners
            .retain(|inode, _| interest.contains(inode));
        self.inode_retry
            .retain(|inode, until| interest.contains(inode) && *until > now);

        let mut pending: HashSet<u64> = HashSet::new();
        let rebuild_all = force_full || self.last_inode_revalidate.elapsed() >= INODE_REVALIDATE;

        if rebuild_all {
            // Drop every mapping and prove it again against live /proc. This
            // is the safety net for an fd handed to another process or an
            // inode recycled into an identical-looking socket.
            self.inode_owners.clear();
            self.last_inode_revalidate = now;
            pending.extend(interest.iter().copied());
        } else {
            // Cheap validation of every cached owner: PID identity plus the
            // socket tuple. No fd table is touched here.
            let mut invalid = Vec::new();
            // A process with several listeners (a dual-stack pair, tcp+udp,
            // workers) would otherwise re-read `/proc/<pid>/stat` once per
            // socket every scan. Memoize per pid inside this loop.
            let mut starttimes: HashMap<i32, Option<u64>> = HashMap::new();
            for (inode, owner) in &self.inode_owners {
                let Some(&(port, protocol, uid)) = socket_meta.get(inode) else {
                    invalid.push(*inode);
                    continue;
                };
                let socket_ok =
                    owner.port == port && owner.protocol == protocol && owner.uid == uid;
                let pid_ok = owner.starttime_jiffies.is_some_and(|starttime| {
                    *starttimes
                        .entry(owner.pid)
                        .or_insert_with(|| self.platform.process_starttime(owner.pid))
                        == Some(starttime)
                });
                if !socket_ok || !pid_ok {
                    invalid.push(*inode);
                }
            }
            for inode in invalid {
                self.inode_owners.remove(&inode);
                pending.insert(inode);
            }
            for inode in interest {
                if !self.inode_owners.contains_key(inode) {
                    pending.insert(*inode);
                }
            }
        }

        // Sockets owned by another UID can never be resolved by this process:
        // `/proc/<pid>/fd` is `dr-x------`. Give them a long backoff instead
        // of letting them force a full fd walk on every refresh (which is
        // exactly why the old early-exit never fired).
        let euid = self.platform.effective_uid();
        let mut to_resolve = HashSet::new();
        for inode in pending {
            let Some(&(_, _, uid)) = socket_meta.get(&inode) else {
                continue;
            };
            if euid != 0 && uid != euid {
                self.inode_retry.insert(inode, now + INODE_FOREIGN_RETRY);
                continue;
            }
            if self
                .inode_retry
                .get(&inode)
                .is_some_and(|until| *until > now)
            {
                continue;
            }
            to_resolve.insert(inode);
        }

        if to_resolve.is_empty() {
            return None;
        }

        match self.platform.build_inode_map(&to_resolve) {
            Ok(map) => {
                for (inode, owners) in &map {
                    let Some(&(pid, _fd)) = owners.first() else {
                        continue;
                    };
                    let Some(&(port, protocol, uid)) = socket_meta.get(inode) else {
                        continue;
                    };
                    self.inode_owners.insert(
                        *inode,
                        InodeOwner {
                            pid,
                            // Filled in after this scan reads the process
                            // metadata; validation treats None as stale.
                            starttime_jiffies: None,
                            port,
                            protocol,
                            uid,
                        },
                    );
                    self.inode_retry.remove(inode);
                }
                // A resolvable inode the walk did not find is a race or a
                // `hidepid` restriction; do not retry it every tick.
                for inode in &to_resolve {
                    if !map.contains_key(inode) {
                        self.inode_retry.insert(*inode, now + INODE_MISSING_RETRY);
                    }
                }
                None
            }
            Err(error) => {
                for inode in &to_resolve {
                    self.inode_retry.insert(*inode, now + INODE_MISSING_RETRY);
                }
                Some(format!("{error:#}"))
            }
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

    /// The Docker Desktop mapping, reused for [`DOCKER_DESKTOP_TTL`] on
    /// success and [`DOCKER_DESKTOP_NEGATIVE_TTL`] when the API is absent.
    ///
    /// Native Linux boxes usually have no Desktop socket; without the negative
    /// cache every refresh would pay a failed `connect` for nothing. A minute
    /// is short enough to notice a Desktop that starts later.
    fn docker_desktop_cached(&mut self) -> Option<DockerDesktopData> {
        if let Some((at, data)) = &self.docker_desktop {
            let ttl = if data.is_some() {
                DOCKER_DESKTOP_TTL
            } else {
                DOCKER_DESKTOP_NEGATIVE_TTL
            };
            if at.elapsed() < ttl {
                return data.clone();
            }
        }
        let data = read_docker_desktop(None);
        self.docker_desktop = Some((Instant::now(), data.clone()));
        data
    }

    /// The container warning, recomputed at most once per 30 s.
    ///
    /// `platform::container_warning` walks all of `/proc` (one cgroup and one
    /// cmdline read per pid); running that on every refresh would blow the
    /// hot-refresh budget. Thirty seconds is plenty of granularity for a
    /// status-bar hint.
    fn container_warning_cached(&mut self) -> Option<Degradation> {
        const TTL: std::time::Duration = std::time::Duration::from_secs(30);
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
    ) -> Result<Process> {
        // A process can exit between the readdir that found it and this read.
        // That is the normal case, not an error.
        let mut process = self.platform.process_metadata(pid)?;
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

        Ok(process)
    }

    fn cpu_scale_factor(&self) -> usize {
        if self.config.cpu_scale == CpuScale::Core {
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

        let project = match crate::cache::keys::inode_and_mtime(cwd) {
            // One stat yields both halves of the L2 key. The mtime makes a
            // freshly initialized directory (manifest appeared since the last
            // scan) miss the cached "no project" instead of serving it for the
            // whole 60 s TTL.
            Some((cwd_inode, cwd_mtime)) => match self.cache.get_project(cwd_inode, cwd_mtime) {
                Some(cached) => cached.clone(),
                None => {
                    let project = detect_project(cwd);
                    self.cache
                        .insert_project(cwd_inode, cwd_mtime, project.clone());
                    project
                }
            },
            // The cwd vanished (or is unreadable): nothing to key on, and
            // detection will fail anyway — do it uncached.
            None => detect_project(cwd),
        };

        let framework = self.detect_framework_cached(project.as_ref(), process, port);

        (project, framework)
    }

    fn detect_framework_cached(
        &mut self,
        project: Option<&crate::model::Project>,
        process: &Process,
        port: u16,
    ) -> Option<crate::model::Framework> {
        let root = project.map(|p| p.root.as_path());
        let detect = |registry: &DetectorRegistry| {
            registry.detect(root, process.exe.as_deref(), &process.cmdline, port)
        };

        // Only project-rooted detection is cacheable: it is the half that reads
        // manifests. The key carries the mtime of the manifest detection
        // already matched, so the hot path is one `stat` and `pnpm add`
        // invalidates.
        let Some(project) = project else {
            return detect(&self.registry);
        };
        let (Some(root_inode), Some(mtime)) = (
            crate::cache::keys::inode(&project.root),
            manifest_mtime(&project.manifest),
        ) else {
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
    use std::io::Write;
    use std::sync::atomic::{AtomicUsize, Ordering};

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

    /// The engine crosses threads: the TUI's collector thread owns the
    /// periodic scans while the UI thread locks it for kill re-resolutions
    /// (both hold `Arc<Mutex<Engine>>`). This guard fails to compile the day
    /// a field makes `Engine` `!Send`.
    #[test]
    fn engine_is_send() {
        fn assert_send<T: Send>() {}
        assert_send::<Engine>();
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

    /// A probe that delegates to the real Linux backend but counts how many
    /// times the engine walks every fd table.
    struct CountingProbe {
        inner: LinuxProbe,
        walks: Arc<AtomicUsize>,
        starttime_reads: Arc<AtomicUsize>,
    }

    impl CountingProbe {
        fn new(root: &Path) -> Self {
            Self {
                inner: LinuxProbe::new(Some(root)),
                walks: Arc::new(AtomicUsize::new(0)),
                starttime_reads: Arc::new(AtomicUsize::new(0)),
            }
        }
    }

    impl PlatformProbe for CountingProbe {
        fn list_listening_sockets(&self, include_udp: bool) -> Result<Vec<Socket>> {
            self.inner.list_listening_sockets(include_udp)
        }

        fn process_metadata(&self, pid: i32) -> Result<Process> {
            self.inner.process_metadata(pid)
        }

        fn process_starttime(&self, pid: i32) -> Option<u64> {
            self.starttime_reads.fetch_add(1, Ordering::Relaxed);
            self.inner.process_starttime(pid)
        }

        fn effective_uid(&self) -> u32 {
            1000
        }

        fn container_of(&self, pid: i32) -> Option<crate::model::Container> {
            self.inner.container_of(pid)
        }

        fn container_warning(&self) -> Option<Degradation> {
            self.inner.container_warning()
        }

        fn signal(&self, pid: i32, expected_starttime: u64, sig: Signal) -> Result<()> {
            self.inner.signal(pid, expected_starttime, sig)
        }

        fn total_jiffies(&self) -> Option<u64> {
            self.inner.total_jiffies()
        }

        fn build_inode_map(
            &self,
            interest: &HashSet<u64>,
        ) -> Result<HashMap<u64, Vec<(i32, i32)>>> {
            self.walks.fetch_add(1, Ordering::Relaxed);
            self.inner.build_inode_map(interest)
        }

        fn num_cpus(&self) -> usize {
            self.inner.num_cpus()
        }

        fn username_for_uid(&self, uid: u32) -> Option<String> {
            self.inner.username_for_uid(uid)
        }
    }

    /// A synthetic `proc_root` with TCP listeners. `our` sockets belong to the
    /// effective UID; `foreign` sockets belong to another user and therefore
    /// can never be resolved by this process.
    fn synthetic_proc(our: &[(u16, i32, u64)], foreign: &[(u16, u64)]) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::write(root.join("stat"), "cpu  1 2 3 4\nbtime 1700000000\n").unwrap();
        std::fs::create_dir_all(root.join("net")).unwrap();
        std::fs::write(
            root.join("net/tcp"),
            "  sl  local_address rem_address   st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode\n",
        )
        .unwrap();

        for &(port, pid, inode) in our {
            add_listener(root, port, pid, inode, 1000);
        }
        for &(port, inode) in foreign {
            add_listener(root, port, 0, inode, 1001);
        }
        dir
    }

    fn add_listener(root: &Path, port: u16, pid: i32, inode: u64, uid: u32) {
        let line = format!(
            "   0: 0100007F:{port:04X} 00000000:0000 0A 00000000:00000000 00:00000000 00000000  {uid}        0 {inode} 1 0000 100 0 0 10 0\n"
        );
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(root.join("net/tcp"))
            .unwrap();
        file.write_all(line.as_bytes()).unwrap();

        if pid == 0 {
            return;
        }
        let pid_dir = root.join(pid.to_string());
        std::fs::create_dir_all(pid_dir.join("fd")).unwrap();
        std::fs::write(
            pid_dir.join("stat"),
            format!("{pid} (svc) S 1 1 1 0 -1 0 0 0 0 0 10 20 0 0 20 0 1 0 12345 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0"),
        )
        .unwrap();
        std::fs::write(
            pid_dir.join("status"),
            "Name:\tsvc\nUid:\t1000\t1000\t1000\t1000\nVmRSS:\t1024 kB\nThreads:\t1\n",
        )
        .unwrap();
        std::fs::write(pid_dir.join("cmdline"), b"node\x00server.js\x00").unwrap();
        // One process can own several sockets; give each its own fd.
        let fd_dir = pid_dir.join("fd");
        let mut fd = 3;
        // `exists()` follows the link, and `socket:[N]` is dangling; probe the
        // link itself.
        while std::fs::symlink_metadata(fd_dir.join(fd.to_string())).is_ok() {
            fd += 1;
        }
        std::os::unix::fs::symlink(format!("socket:[{inode}]"), fd_dir.join(fd.to_string()))
            .unwrap();
    }

    /// The whole point of P0: a stable socket set must not trigger a second
    /// `/proc/<pid>/fd` walk. `scan_fresh` is the explicit revalidation.
    #[test]
    fn test_inode_map_is_cached_between_scans() {
        let dir = synthetic_proc(&[(8080, 1234, 777001)], &[]);
        let probe = CountingProbe::new(dir.path());
        let walks = Arc::clone(&probe.walks);
        let mut engine = Engine::from_platform(Box::new(probe), Config::default());

        let first = engine.scan();
        assert_eq!(first.services.len(), 1);
        assert_eq!(walks.load(Ordering::Relaxed), 1);

        let second = engine.scan();
        assert_eq!(second.services.len(), 1);
        assert_eq!(
            walks.load(Ordering::Relaxed),
            1,
            "a stable socket set must reuse the cached inode map"
        );

        let _ = engine.scan_fresh();
        assert_eq!(
            walks.load(Ordering::Relaxed),
            2,
            "scan_fresh must revalidate against live /proc"
        );
    }

    /// A process with several listeners (dual-stack pair, tcp+udp, workers)
    /// must validate its identity once per scan, not once per socket: the
    /// per-socket re-read of `/proc/<pid>/stat` was pure waste.
    #[test]
    fn test_starttime_read_once_per_pid_per_scan() {
        let dir = synthetic_proc(&[(8080, 1234, 777001), (8081, 1234, 777002)], &[]);
        let probe = CountingProbe::new(dir.path());
        let starttime_reads = Arc::clone(&probe.starttime_reads);
        let mut engine = Engine::from_platform(Box::new(probe), Config::default());

        let first = engine.scan();
        assert_eq!(first.services.len(), 2, "one process owns two sockets");
        starttime_reads.store(0, Ordering::Relaxed);

        let second = engine.scan();
        assert_eq!(second.services.len(), 2);
        assert_eq!(
            starttime_reads.load(Ordering::Relaxed),
            1,
            "two sockets of one pid must cost one starttime read per scan"
        );
    }

    /// The bug that made the old early-exit unreachable: a socket owned by
    /// another user can never be resolved, and must not force a full fd walk
    /// on every refresh.
    #[test]
    fn test_foreign_socket_does_not_force_repeated_fd_walks() {
        let dir = synthetic_proc(&[(8080, 1234, 777001)], &[(5353, 777999)]);
        let probe = CountingProbe::new(dir.path());
        let walks = Arc::clone(&probe.walks);
        let mut engine = Engine::from_platform(Box::new(probe), Config::default());

        let first = engine.scan();
        assert_eq!(first.services.len(), 2, "both rows are still reported");
        assert_eq!(walks.load(Ordering::Relaxed), 1);

        let _ = engine.scan();
        assert_eq!(
            walks.load(Ordering::Relaxed),
            1,
            "the unreadable foreign inode must be skipped, not re-walked"
        );
    }

    /// A newly started server is the one event that must pay a fresh walk,
    /// and only once.
    #[test]
    fn test_new_socket_triggers_one_incremental_walk() {
        let dir = synthetic_proc(&[(8080, 1234, 777001)], &[]);
        let probe = CountingProbe::new(dir.path());
        let walks = Arc::clone(&probe.walks);
        let mut engine = Engine::from_platform(Box::new(probe), Config::default());

        let _ = engine.scan();
        assert_eq!(walks.load(Ordering::Relaxed), 1);

        add_listener(dir.path(), 9090, 5678, 888001, 1000);
        let snapshot = engine.scan();
        assert_eq!(snapshot.services.len(), 2);
        assert_eq!(
            walks.load(Ordering::Relaxed),
            2,
            "the unknown inode must be resolved exactly once"
        );

        let _ = engine.scan();
        assert_eq!(walks.load(Ordering::Relaxed), 2);
    }

    /// A socket whose owner vanishes between the fd walk and the metadata read
    /// is the normal `/proc` race. It must count as `ProcessesDisappeared`,
    /// not as `ProcessesPartialRead` (a permission/format problem) — the
    /// distinction the typed `ProcessGone` error exists to preserve.
    #[test]
    fn test_vanished_process_counts_as_disappeared() {
        let dir = synthetic_proc(&[(8080, 1234, 777001)], &[]);
        // The fd table still resolves the socket's owner, but its stat file is
        // gone: exactly what a process exiting mid-scan looks like.
        std::fs::remove_file(dir.path().join("1234/stat")).unwrap();
        let mut engine =
            Engine::from_platform(Box::new(CountingProbe::new(dir.path())), Config::default());

        let snapshot = engine.scan();
        assert!(
            snapshot
                .degradations
                .iter()
                .any(|d| matches!(d, Degradation::ProcessesDisappeared(1))),
            "expected one disappeared process, got {:?}",
            snapshot.degradations
        );
        assert!(
            !snapshot
                .degradations
                .iter()
                .any(|d| matches!(d, Degradation::ProcessesPartialRead(_))),
            "a vanished owner is not a partial read: {:?}",
            snapshot.degradations
        );
    }
}
