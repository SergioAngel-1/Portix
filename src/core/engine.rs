use crate::cache::CacheStore;
use crate::detect::DetectorRegistry;
use crate::model::{Service, ServiceId, Snapshot, SharedSnapshot, Degradation};
use crate::platform::{LinuxProbe, PlatformProbe};
use anyhow::Result;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// The core engine that orchestrates scanning, detection, and caching.
pub struct Engine {
    platform: Box<dyn PlatformProbe>,
    cache: CacheStore,
    registry: DetectorRegistry,
    current_uid: u32,
}

impl Engine {
    /// Create a new engine with the given proc root.
    pub fn new(proc_root: Option<&Path>) -> Result<Self> {
        let platform = Box::new(LinuxProbe::new(proc_root));
        let current_uid = unsafe { libc::getuid() };

        Ok(Self {
            platform,
            cache: CacheStore::new(),
            registry: DetectorRegistry::new(),
            current_uid,
        })
    }

    /// Perform a full scan and return a snapshot.
    pub fn scan(&mut self) -> Result<SharedSnapshot> {
        let start = Instant::now();
        let mut services = Vec::new();
        let mut degredations = Vec::new();

        // Step 1: List all listening sockets
        let sockets = match self.platform.list_listening_sockets() {
            Ok(s) => s,
            Err(e) => {
                degredations.push(Degradation::SocketsWithoutPid(0));
                return Ok(Arc::new(Snapshot {
                    services: Vec::new(),
                    taken_at: start,
                    scan_duration_ms: start.elapsed().as_millis() as u64,
                    degredations,
                }));
            }
        };

        // Step 2: Build inode -> PID map
        let interest_inodes: std::collections::HashSet<u64> =
            sockets.iter().map(|s| s.inode).collect();

        // For now, we'll skip the full inode mapping and just use what we can get
        // TODO: Implement the full inode mapping with rayon parallelism

        // Step 3: For each socket, create a service
        for socket in &sockets {
            let service = Service {
                id: ServiceId::from_socket(socket),
                socket: socket.clone(),
                process: None, // TODO: Resolve from inode map
                project: None, // TODO: Detect from cwd
                framework: None, // TODO: Detect from context
                container: None, // TODO: Detect from cgroup
            };
            services.push(service);
        }

        let snapshot = Snapshot {
            services,
            taken_at: start,
            scan_duration_ms: start.elapsed().as_millis() as u64,
            degredations,
        };

        Ok(Arc::new(snapshot))
    }
}
