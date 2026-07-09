/// Stable cache keys based on inodes, not paths.
///
/// Using inodes instead of paths ensures:
/// - A `git mv` or rename doesn't invalidate the cache
/// - No stale entries when a directory name is reused
pub struct CacheKeys;

impl CacheKeys {
    /// Get the inode for a path.
    pub fn inode(path: &std::path::Path) -> Option<u64> {
        std::fs::metadata(path)
            .ok()
            .and_then(|m| {
                #[cfg(unix)]
                {
                    use std::os::unix::fs::MetadataExt;
                    Some(m.ino())
                }
                #[cfg(not(unix))]
                {
                    None
                }
            })
    }

    /// Get the modification time as a u64 for cache invalidation.
    pub fn mtime(path: &std::path::Path) -> Option<u64> {
        std::fs::metadata(path)
            .ok()
            .and_then(|m| m.modified().ok())
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs())
    }
}
