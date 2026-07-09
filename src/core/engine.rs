use crate::cache::CacheStore;
use crate::detect::DetectorRegistry;
use crate::detect::project::detect_project;
use crate::detect::framework::detect_framework;
use crate::model::{Service, ServiceId, Snapshot, SharedSnapshot, Degradation};
use crate::platform::{LinuxProbe, PlatformProbe};
use anyhow::Result;
use std::collections::HashSet;
use std::path::Path;
use std::sync::Arc;
use std::time::Instant;

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

        // Get current user ID safely
        let current_uid = std::fs::read_to_string("/proc/self/status")
            .ok()
            .and_then(|s| {
                s.lines()
                    .find(|l| l.starts_with("Uid:"))
                    .and_then(|l| l.split_whitespace().nth(1))
                    .and_then(|s| s.parse::<u32>().ok())
            })
            .unwrap_or(1000);

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
            Err(_) => {
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
        let interest_inodes: HashSet<u64> = sockets.iter().map(|s| s.inode).collect();
        let inode_map = match crate::platform::linux::fd::build_inode_map(
            self.platform.proc_root(),
            &interest_inodes,
            self.current_uid,
        ) {
            Ok(map) => map,
            Err(_) => {
                degredations.push(Degradation::SocketsWithoutPid(sockets.len()));
                std::collections::HashMap::new()
            }
        };

        // Step 3: For each socket, resolve process and detect project/framework
        for socket in &sockets {
            let mut service = Service {
                id: ServiceId::from_socket(socket),
                socket: socket.clone(),
                process: None,
                project: None,
                framework: None,
                container: None,
            };

            // Resolve PID from inode
            if let Some(pids) = inode_map.get(&socket.inode) {
                if let Some(&(pid, _fd)) = pids.first() {
                    // Get process metadata
                    match self.platform.process_metadata(pid) {
                        Ok(mut process) => {
                            // Calculate uptime if we have started_at
                            if let Some(started) = process.started_at {
                                process.metrics.uptime = started.elapsed().ok();
                            }

                            // Detect container
                            service.container = self.platform.container_of(pid);

                            // Detect project from cwd
                            if let Some(ref cwd) = process.cwd {
                                // Check cache first
                                let cwd_inode = crate::cache::keys::inode(cwd);
                                if let Some(inode) = cwd_inode {
                                    if let Some(cached) = self.cache.get_project(inode) {
                                        service.project = cached.clone();
                                    } else {
                                        let project = detect_project(cwd);
                                        self.cache.insert_project(inode, project.clone());
                                        service.project = project;
                                    }
                                } else {
                                    service.project = detect_project(cwd);
                                }

                                // Detect framework from project and process
                                service.framework = detect_framework(
                                    service.project.as_ref().map(|p| p.root.as_path()),
                                    process.exe.as_deref(),
                                    &process.cmdline,
                                );
                            }

                            service.process = Some(process);
                        }
                        Err(_) => {
                            // Process may have exited between readdir and open
                            // This is normal, don't add degradation
                        }
                    }
                }
            }

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
