use crate::model::{Framework, Process, Project};
use std::collections::HashMap;
use std::time::{Duration, Instant};

/// Cache entry with TTL.
struct CacheEntry<T> {
    value: T,
    created_at: Instant,
    ttl: Duration,
}

impl<T> CacheEntry<T> {
    fn new(value: T, ttl: Duration) -> Self {
        Self {
            value,
            created_at: Instant::now(),
            ttl,
        }
    }

    fn is_valid(&self) -> bool {
        self.created_at.elapsed() < self.ttl
    }
}

/// Multi-level cache for process data.
pub struct CacheStore {
    /// L1: Process metadata (pid, starttime) -> Process
    l1_process: HashMap<(i32, u64), CacheEntry<Process>>,
    /// L2: Project (inode of cwd) -> Project
    l2_project: HashMap<u64, CacheEntry<Option<Project>>>,
    /// L3: Framework (inode of root, mtime of manifest) -> Framework
    l3_framework: HashMap<(u64, u64), CacheEntry<Option<Framework>>>,
    /// L4: Users (uid) -> username
    l4_users: HashMap<u32, CacheEntry<String>>,
}

impl CacheStore {
    pub fn new() -> Self {
        Self {
            l1_process: HashMap::new(),
            l2_project: HashMap::new(),
            l3_framework: HashMap::new(),
            l4_users: HashMap::new(),
        }
    }

    /// Get a process from L1 cache.
    pub fn get_process(&self, pid: i32, starttime: u64) -> Option<&Process> {
        self.l1_process
            .get(&(pid, starttime))
            .filter(|e| e.is_valid())
            .map(|e| &e.value)
    }

    /// Insert a process into L1 cache.
    pub fn insert_process(&mut self, pid: i32, starttime: u64, process: Process) {
        self.l1_process.insert(
            (pid, starttime),
            CacheEntry::new(process, Duration::from_secs(60)),
        );
    }

    /// Get a project from L2 cache.
    pub fn get_project(&self, cwd_inode: u64) -> Option<&Option<Project>> {
        self.l2_project
            .get(&cwd_inode)
            .filter(|e| e.is_valid())
            .map(|e| &e.value)
    }

    /// Insert a project into L2 cache.
    pub fn insert_project(&mut self, cwd_inode: u64, project: Option<Project>) {
        self.l2_project.insert(
            cwd_inode,
            CacheEntry::new(project, Duration::from_secs(60)),
        );
    }

    /// Get a framework from L3 cache.
    pub fn get_framework(&self, root_inode: u64, manifest_mtime: u64) -> Option<&Option<Framework>> {
        self.l3_framework
            .get(&(root_inode, manifest_mtime))
            .filter(|e| e.is_valid())
            .map(|e| &e.value)
    }

    /// Insert a framework into L3 cache.
    pub fn insert_framework(&mut self, root_inode: u64, manifest_mtime: u64, framework: Option<Framework>) {
        self.l3_framework.insert(
            (root_inode, manifest_mtime),
            CacheEntry::new(framework, Duration::from_secs(60)),
        );
    }

    /// Get a username from L4 cache.
    pub fn get_user(&self, uid: u32) -> Option<&String> {
        self.l4_users
            .get(&uid)
            .filter(|e| e.is_valid())
            .map(|e| &e.value)
    }

    /// Insert a username into L4 cache.
    pub fn insert_user(&mut self, uid: u32, username: String) {
        self.l4_users.insert(
            uid,
            CacheEntry::new(username, Duration::from_secs(300)),
        );
    }

    /// Invalidate a process entry.
    pub fn invalidate_process(&mut self, pid: i32, starttime: u64) {
        self.l1_process.remove(&(pid, starttime));
    }

    /// Clear all caches.
    pub fn clear(&mut self) {
        self.l1_process.clear();
        self.l2_project.clear();
        self.l3_framework.clear();
        self.l4_users.clear();
    }

    /// Remove expired entries.
    pub fn cleanup(&mut self) {
        self.l1_process.retain(|_, e| e.is_valid());
        self.l2_project.retain(|_, e| e.is_valid());
        self.l3_framework.retain(|_, e| e.is_valid());
        self.l4_users.retain(|_, e| e.is_valid());
    }
}
