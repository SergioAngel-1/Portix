use crate::model::{Framework, Process, ProcessKey, Project};
use std::collections::HashMap;
use std::path::PathBuf;
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

/// The parts of a process that cannot change while it lives.
///
/// Deliberately excludes metrics: RSS and CPU move every second, and caching
/// them is how a monitor starts lying. Only the immutable half is memoized.
#[derive(Debug, Clone)]
pub struct ProcessStatic {
    pub name: String,
    pub ppid: i32,
    pub cmdline: Vec<String>,
    pub exe: Option<PathBuf>,
    pub cwd: Option<PathBuf>,
    pub started_at: Option<std::time::SystemTime>,
}

impl From<&Process> for ProcessStatic {
    fn from(p: &Process) -> Self {
        Self {
            name: p.name.clone(),
            ppid: p.ppid,
            cmdline: p.cmdline.clone(),
            exe: p.exe.clone(),
            cwd: p.cwd.clone(),
            started_at: p.started_at,
        }
    }
}

/// A CPU sample: the process and system jiffy counters, read at the same moment.
#[derive(Debug, Clone, Copy)]
pub struct CpuSample {
    pub proc_jiffies: u64,
    pub total_jiffies: u64,
}

/// Multi-level cache for process data.
#[derive(Default)]
pub struct CacheStore {
    /// L1: immutable process metadata, keyed by (pid, starttime).
    l1_process: HashMap<ProcessKey, ProcessStatic>,
    /// L2: Project, keyed by the inode of the cwd (not its path).
    l2_project: HashMap<u64, CacheEntry<Option<Project>>>,
    /// L3: Framework, keyed by (inode of project root, mtime of manifest).
    l3_framework: HashMap<(u64, u64), Option<Framework>>,
    /// L4: uid -> username.
    l4_users: HashMap<u32, CacheEntry<String>>,
    /// Previous CPU counters, keyed by (pid, starttime).
    cpu_samples: HashMap<ProcessKey, CpuSample>,
}

impl CacheStore {
    pub fn new() -> Self {
        Self::default()
    }

    /// Get immutable process metadata.
    ///
    /// Keyed on `(pid, starttime)` and never on `pid` alone: PIDs are recycled,
    /// and a hit on a recycled PID would show a dead process's project.
    pub fn get_process(&self, key: ProcessKey) -> Option<&ProcessStatic> {
        self.l1_process.get(&key)
    }

    pub fn insert_process(&mut self, key: ProcessKey, value: ProcessStatic) {
        self.l1_process.insert(key, value);
    }

    /// Get a project from L2 cache. The outer Option is "cached?", the inner is
    /// "is there a project?" — negative results are cached too, otherwise every
    /// scan re-walks the directory tree for sshd and systemd-resolved.
    pub fn get_project(&self, cwd_inode: u64) -> Option<&Option<Project>> {
        self.l2_project
            .get(&cwd_inode)
            .filter(|e| e.is_valid())
            .map(|e| &e.value)
    }

    pub fn insert_project(&mut self, cwd_inode: u64, project: Option<Project>) {
        self.l2_project
            .insert(cwd_inode, CacheEntry::new(project, Duration::from_secs(60)));
    }

    /// Get a framework. No TTL: the manifest mtime is part of the key, so a
    /// `pnpm add next@16` produces a different key and a natural miss.
    pub fn get_framework(&self, root_inode: u64, manifest_mtime: u64) -> Option<&Option<Framework>> {
        self.l3_framework.get(&(root_inode, manifest_mtime))
    }

    pub fn insert_framework(
        &mut self,
        root_inode: u64,
        manifest_mtime: u64,
        framework: Option<Framework>,
    ) {
        self.l3_framework
            .insert((root_inode, manifest_mtime), framework);
    }

    pub fn get_user(&self, uid: u32) -> Option<&String> {
        self.l4_users
            .get(&uid)
            .filter(|e| e.is_valid())
            .map(|e| &e.value)
    }

    pub fn insert_user(&mut self, uid: u32, username: String) {
        self.l4_users
            .insert(uid, CacheEntry::new(username, Duration::from_secs(300)));
    }

    /// Read the previous CPU sample for a process, then store the current one.
    pub fn swap_cpu_sample(&mut self, key: ProcessKey, current: CpuSample) -> Option<CpuSample> {
        self.cpu_samples.insert(key, current)
    }

    /// Drop everything belonging to processes that no longer exist.
    ///
    /// Without this, a long-running Portix accumulates one L1 entry per dev
    /// server the user ever started.
    pub fn retain_live(&mut self, live: &std::collections::HashSet<ProcessKey>) {
        self.l1_process.retain(|k, _| live.contains(k));
        self.cpu_samples.retain(|k, _| live.contains(k));
        self.l2_project.retain(|_, e| e.is_valid());
        self.l4_users.retain(|_, e| e.is_valid());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn a_static(name: &str) -> ProcessStatic {
        ProcessStatic {
            name: name.to_string(),
            ppid: 1,
            cmdline: vec![name.to_string()],
            exe: None,
            cwd: None,
            started_at: None,
        }
    }

    /// The bug this key design exists to prevent: PID 4242 dies, the kernel
    /// hands 4242 to something else, and the cache shows the dead process's data.
    #[test]
    fn test_pid_reuse_does_not_leak_cache() {
        let mut cache = CacheStore::new();
        let old = ProcessKey {
            pid: 4242,
            starttime_jiffies: 111,
        };
        let recycled = ProcessKey {
            pid: 4242,
            starttime_jiffies: 999,
        };

        cache.insert_process(old, a_static("next-dev"));

        assert!(cache.get_process(recycled).is_none());
        assert_eq!(cache.get_process(old).unwrap().name, "next-dev");
    }

    #[test]
    fn test_framework_invalidated_when_manifest_mtime_changes() {
        let mut cache = CacheStore::new();
        cache.insert_framework(7, 1000, None);

        assert!(cache.get_framework(7, 1000).is_some());
        assert!(cache.get_framework(7, 2000).is_none());
    }

    #[test]
    fn test_cpu_sample_swap_returns_previous() {
        let mut cache = CacheStore::new();
        let key = ProcessKey {
            pid: 1,
            starttime_jiffies: 1,
        };

        assert!(cache
            .swap_cpu_sample(
                key,
                CpuSample {
                    proc_jiffies: 10,
                    total_jiffies: 100
                }
            )
            .is_none());

        let prev = cache
            .swap_cpu_sample(
                key,
                CpuSample {
                    proc_jiffies: 20,
                    total_jiffies: 200,
                },
            )
            .unwrap();
        assert_eq!(prev.proc_jiffies, 10);
    }

    #[test]
    fn test_retain_live_evicts_dead_processes() {
        let mut cache = CacheStore::new();
        let alive = ProcessKey {
            pid: 1,
            starttime_jiffies: 1,
        };
        let dead = ProcessKey {
            pid: 2,
            starttime_jiffies: 2,
        };
        cache.insert_process(alive, a_static("alive"));
        cache.insert_process(dead, a_static("dead"));

        let live = std::collections::HashSet::from([alive]);
        cache.retain_live(&live);

        assert!(cache.get_process(alive).is_some());
        assert!(cache.get_process(dead).is_none());
    }
}
