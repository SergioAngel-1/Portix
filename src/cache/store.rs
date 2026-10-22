use crate::model::{Framework, ProcessKey, Project};
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

/// A CPU sample: the process and system jiffy counters, read at the same moment.
#[derive(Debug, Clone, Copy)]
pub struct CpuSample {
    pub proc_jiffies: u64,
    pub total_jiffies: u64,
}

/// Multi-level cache for process data.
#[derive(Default)]
pub struct CacheStore {
    /// L2: Project, keyed by `(inode of the cwd, mtime of the cwd)` — not the
    /// path. The inode survives renames; the mtime invalidates the entry the
    /// moment a manifest appears in or disappears from a previously
    /// manifest-less cwd, so a freshly `pnpm init`-ed directory stops serving
    /// the cached "no project" answer immediately instead of for the whole TTL.
    l2_project: HashMap<(u64, u64), CacheEntry<Option<Project>>>,
    /// L3: Framework, keyed by (inode of project root, mtime of manifest, and a
    /// discriminator over the process inputs detection also depends on).
    l3_framework: HashMap<(u64, u64, u64), CacheEntry<Option<Framework>>>,
    /// L4: uid -> username.
    l4_users: HashMap<u32, CacheEntry<String>>,
    /// Previous CPU counters, keyed by (pid, starttime).
    cpu_samples: HashMap<ProcessKey, CpuSample>,
}

impl CacheStore {
    pub fn new() -> Self {
        Self::default()
    }

    /// Get a project from L2 cache. The outer Option is "cached?", the inner is
    /// "is there a project?" — negative results are cached too, otherwise every
    /// scan re-walks the directory tree for sshd and systemd-resolved.
    ///
    /// `cwd_mtime` is the modification time of the cwd directory at the moment
    /// of the lookup; a mismatch with the key the entry was inserted under
    /// means the directory's contents changed (a manifest appeared or
    /// vanished), so the cached answer is stale.
    pub fn get_project(&self, cwd_inode: u64, cwd_mtime: u64) -> Option<&Option<Project>> {
        self.l2_project
            .get(&(cwd_inode, cwd_mtime))
            .filter(|e| e.is_valid())
            .map(|e| &e.value)
    }

    pub fn insert_project(&mut self, cwd_inode: u64, cwd_mtime: u64, project: Option<Project>) {
        self.l2_project.insert(
            (cwd_inode, cwd_mtime),
            CacheEntry::new(project, Duration::from_secs(60)),
        );
    }

    /// Get a framework.
    ///
    /// The key is `(root_inode, manifest_mtime, disc)`: the manifest mtime
    /// invalidates on `pnpm add`, and `disc` is a hash of the process inputs
    /// (port, exe, cmdline) so two processes sharing a project root but running
    /// different commands do not collide on one entry. A TTL bounds growth,
    /// since a new mtime or disc otherwise leaks the old key forever.
    pub fn get_framework(
        &self,
        root_inode: u64,
        manifest_mtime: u64,
        disc: u64,
    ) -> Option<&Option<Framework>> {
        self.l3_framework
            .get(&(root_inode, manifest_mtime, disc))
            .filter(|e| e.is_valid())
            .map(|e| &e.value)
    }

    pub fn insert_framework(
        &mut self,
        root_inode: u64,
        manifest_mtime: u64,
        disc: u64,
        framework: Option<Framework>,
    ) {
        self.l3_framework.insert(
            (root_inode, manifest_mtime, disc),
            CacheEntry::new(framework, Duration::from_secs(300)),
        );
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

    /// Drop everything belonging to processes that no longer exist, and any
    /// TTL-expired project/framework/user entries.
    ///
    /// Without this, a long-running Portix accumulates one CPU sample per dev
    /// server the user ever started, and one framework entry per manifest edit.
    pub fn retain_live(&mut self, live: &std::collections::HashSet<ProcessKey>) {
        self.cpu_samples.retain(|k, _| live.contains(k));
        self.l2_project.retain(|_, e| e.is_valid());
        self.l3_framework.retain(|_, e| e.is_valid());
        self.l4_users.retain(|_, e| e.is_valid());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_framework_invalidated_when_manifest_mtime_changes() {
        let mut cache = CacheStore::new();
        cache.insert_framework(7, 1000, 0, None);

        assert!(cache.get_framework(7, 1000, 0).is_some());
        assert!(cache.get_framework(7, 2000, 0).is_none());
    }

    /// The bug this fixes: a directory with no manifest caches a negative L2
    /// answer; when `pnpm init` later creates package.json there, the cached
    /// "no project" must not keep serving for the rest of the 60 s TTL. The
    /// cwd's mtime changes the moment the file appears, so the key misses.
    #[test]
    fn test_l2_negative_invalidated_when_cwd_mtime_changes() {
        let mut cache = CacheStore::new();
        cache.insert_project(7, 1000, None);

        // Same inode, same mtime: the cached negative serves (no re-walk).
        assert!(cache.get_project(7, 1000).is_some());
        assert!(cache.get_project(7, 1000).unwrap().is_none());

        // A manifest appeared: the directory mtime changed, the key misses,
        // and re-detection can cache the fresh answer.
        assert!(cache.get_project(7, 2000).is_none());
        cache.insert_project(
            7,
            2000,
            Some(Project {
                root: std::path::PathBuf::from("/w"),
                manifest: std::path::PathBuf::from("/w/package.json"),
                workspace_root: None,
                name: "web".into(),
                technical_name: None,
                language: None,
                package_manager: None,
            }),
        );
        assert!(cache.get_project(7, 2000).unwrap().is_some());
        // The stale negative is still there under its old mtime but can never
        // be reached: no lookup will present mtime 1000 again.
        assert!(cache.get_project(7, 1000).unwrap().is_none());
    }

    /// Two processes sharing a project root but differing in their inputs (a
    /// different `disc`) must not read each other's cached framework.
    #[test]
    fn test_framework_key_separates_distinct_processes() {
        let mut cache = CacheStore::new();
        cache.insert_framework(7, 1000, 111, None);

        assert!(cache.get_framework(7, 1000, 111).is_some());
        assert!(cache.get_framework(7, 1000, 222).is_none());
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

    /// A dead process's CPU sample must not linger once it is no longer live:
    /// the (pid, starttime) key is what stops a recycled PID reading stale data.
    #[test]
    fn test_retain_live_evicts_dead_cpu_samples() {
        let mut cache = CacheStore::new();
        let alive = ProcessKey {
            pid: 1,
            starttime_jiffies: 1,
        };
        let dead = ProcessKey {
            pid: 2,
            starttime_jiffies: 2,
        };
        cache.swap_cpu_sample(
            alive,
            CpuSample {
                proc_jiffies: 1,
                total_jiffies: 1,
            },
        );
        cache.swap_cpu_sample(
            dead,
            CpuSample {
                proc_jiffies: 1,
                total_jiffies: 1,
            },
        );

        let live = std::collections::HashSet::from([alive]);
        cache.retain_live(&live);

        assert!(cache
            .swap_cpu_sample(
                alive,
                CpuSample {
                    proc_jiffies: 2,
                    total_jiffies: 2
                }
            )
            .is_some());
        assert!(cache
            .swap_cpu_sample(
                dead,
                CpuSample {
                    proc_jiffies: 2,
                    total_jiffies: 2
                }
            )
            .is_none());
    }

    /// L4 (uid → username): a hit returns the cached name; a miss that was
    /// stored becomes a hit. This is the cache the engine routes every
    /// `getpwuid_r` through.
    #[test]
    fn test_user_cache_roundtrip() {
        let mut cache = CacheStore::new();
        assert!(cache.get_user(1000).is_none());

        cache.insert_user(1000, "sergi".to_string());
        assert_eq!(cache.get_user(1000).map(String::as_str), Some("sergi"));
    }
}
