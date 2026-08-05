//! Stable cache keys based on inodes, not paths.
//!
//! Using inodes instead of paths ensures:
//! - A `git mv` or rename doesn't invalidate the cache
//! - No stale entries when a directory name is reused

/// Get the inode for a path.
pub fn inode(path: &std::path::Path) -> Option<u64> {
    std::fs::metadata(path).ok().map(|m| {
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            m.ino()
        }
        #[cfg(not(unix))]
        {
            None
        }
    })
}

/// Inode and modification time of a path, from a **single** `stat`.
///
/// L2 keys on the pair `(inode, mtime)`: the inode survives renames (a
/// `git mv` of the project directory does not invalidate), and the mtime
/// catches a manifest *appearing in* or *disappearing from* a directory that
/// previously had none — the "project just got `pnpm init`-ed under a running
/// server" case, which a bare inode key served stale negatives for up to the
/// L2 TTL. Querying both halves in one call keeps the hot path at one
/// syscall per process per scan.
///
/// The mtime is the *directory's*, not the manifest's: it changes exactly
/// when a marker file is created or deleted (the events that change the
/// project-detection answer). Content edits to an existing manifest do not
/// change it, but those change framework/version, which the L3 key already
/// invalidates via `mtime(manifest)`.
pub fn inode_and_mtime(path: &std::path::Path) -> Option<(u64, u64)> {
    let meta = std::fs::metadata(path).ok()?;
    #[cfg(unix)]
    let ino = {
        use std::os::unix::fs::MetadataExt;
        meta.ino()
    };
    #[cfg(not(unix))]
    let ino = 0;
    let mtime = meta
        .modified()
        .ok()?
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .and_then(|d| u64::try_from(d.as_nanos()).ok())?;
    Some((ino, mtime))
}

/// Get the modification time as a u64 for cache invalidation.
///
/// Nanosecond resolution: the L3 framework key carries the manifest mtime, and
/// second granularity lets two edits within the same second (`pnpm add` in
/// quick succession) serve a stale cached framework for up to the L3 TTL.
pub fn mtime(path: &std::path::Path) -> Option<u64> {
    std::fs::metadata(path)
        .ok()
        .and_then(|m| m.modified().ok())
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .and_then(|d| u64::try_from(d.as_nanos()).ok())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    /// Two writes within the same second must still invalidate: the key
    /// resolution is sub-second on every Linux filesystem.
    #[test]
    fn test_mtime_changes_within_the_same_second() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("package.json");
        std::fs::write(&file, r#"{"name":"a"}"#).unwrap();
        let first = mtime(&file).expect("first mtime");
        std::thread::sleep(Duration::from_millis(2));
        std::fs::write(&file, r#"{"name":"b"}"#).unwrap();
        let second = mtime(&file).expect("second mtime");
        assert_ne!(first, second, "sub-second edits must produce distinct keys");
    }

    #[test]
    fn test_mtime_missing_file_is_none() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(mtime(&dir.path().join("nope.json")), None);
    }
}
