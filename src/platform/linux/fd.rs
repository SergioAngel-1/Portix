use anyhow::Result;
use std::collections::{HashMap, HashSet};
use std::path::Path;

/// Build a map from inode to (pid, fd) by scanning /proc/<pid>/fd/*.
///
/// This is the critical optimization: instead of O(sockets × processes × fds),
/// we do a single pass through all processes and build the reverse map.
///
/// No uid filter is applied — and none is needed: `/proc/<pid>/fd` is
/// `dr-x------`, readable only by the process owner (and root), so a
/// non-owner's fd table simply fails `read_dir` with EACCES and is skipped by
/// the `if let Ok` below. That is what keeps sockets of other users
/// unresolved (→ `Degradation::SocketsWithoutPid`) without the extra
/// `/proc/<pid>/status` read per process the old ownership filter paid — one
/// more read per pid per scan was doubling the status I/O on the hot path.
pub fn build_inode_map(
    proc_root: &Path,
    interest_inodes: &HashSet<u64>,
) -> Result<HashMap<u64, Vec<(i32, i32)>>> {
    let mut map: HashMap<u64, Vec<(i32, i32)>> = HashMap::new();
    let total_interest = interest_inodes.len();

    // An empty interest set means every socket was filtered out (or none
    // existed): there is nothing to resolve, so skip the pid listing and the
    // fd-table walk entirely. The per-inode early-exit below can never fire
    // for an empty set (map.len() >= 0 is always true but the old
    // `!interest_inodes.is_empty()` guard made the walk run anyway — ~12k
    // readlinks for nothing on a typical box).
    if total_interest == 0 {
        return Ok(map);
    }

    // Read all PIDs
    let pids = super::proc::list_pids(proc_root)?;

    for pid in pids {
        let fd_dir = proc_root.join(pid.to_string()).join("fd");
        if let Ok(entries) = std::fs::read_dir(&fd_dir) {
            for entry in entries.flatten() {
                if let Ok(target) = std::fs::read_link(entry.path()) {
                    if let Some(target_str) = target.to_str() {
                        if let Some(inode_str) = target_str.strip_prefix("socket:[") {
                            if let Some(inode_str) = inode_str.strip_suffix(']') {
                                if let Ok(inode) = inode_str.parse::<u64>() {
                                    if interest_inodes.contains(&inode) {
                                        let Some(fd_num) = entry
                                            .file_name()
                                            .to_str()
                                            .and_then(|s| s.parse::<i32>().ok())
                                        else {
                                            continue;
                                        };
                                        map.entry(inode).or_default().push((pid, fd_num));
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

        // Stop once every inode we care about has an owner. Count distinct
        // inodes, not hits: one socket can be held by several fds (dup, forked
        // workers under SO_REUSEPORT), and counting hits stops the scan early
        // with inodes still unresolved.
        if map.len() >= total_interest {
            break;
        }
    }

    Ok(map)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_inode_map_simple() {
        let temp_dir = tempfile::tempdir().unwrap();
        let proc_root = temp_dir.path();

        // Create fake /proc structure
        let pid_dir = proc_root.join("1234");
        let fd_dir = pid_dir.join("fd");
        std::fs::create_dir_all(&fd_dir).unwrap();

        // Create a symlink that looks like socket:[486312]
        std::os::unix::fs::symlink("socket:[486312]", fd_dir.join("23")).unwrap();

        // Create status file with our UID
        std::fs::write(
            pid_dir.join("status"),
            "Name:\tnode\nUid:\t1000\t1000\t1000\t1000\n",
        )
        .unwrap();

        let mut interest = HashSet::new();
        interest.insert(486312);

        let map = build_inode_map(proc_root, &interest).unwrap();
        assert!(map.contains_key(&486312));
        assert_eq!(map[&486312][0].0, 1234);
    }

    /// One process holding the same socket on two fds must not convince the
    /// scan it has resolved two inodes and stop early.
    #[test]
    fn test_duplicate_fds_do_not_end_the_scan_early() {
        let temp_dir = tempfile::tempdir().unwrap();
        let proc_root = temp_dir.path();

        for (pid, inode, fds) in [(1, 100u64, vec!["3", "4"]), (2, 200u64, vec!["5"])] {
            let pid_dir = proc_root.join(pid.to_string());
            let fd_dir = pid_dir.join("fd");
            std::fs::create_dir_all(&fd_dir).unwrap();
            std::fs::write(
                pid_dir.join("status"),
                "Name:\tsvc\nUid:\t1000\t1000\t1000\t1000\n",
            )
            .unwrap();
            for fd in fds {
                std::os::unix::fs::symlink(format!("socket:[{inode}]"), fd_dir.join(fd)).unwrap();
            }
        }

        let interest = HashSet::from([100, 200]);
        let map = build_inode_map(proc_root, &interest).unwrap();

        assert!(map.contains_key(&100), "inode 100 missing");
        assert!(map.contains_key(&200), "scan stopped before reaching pid 2");
    }

    /// An empty interest set must short-circuit before any filesystem access:
    /// even a nonexistent proc root resolves to an empty map instead of an
    /// error, because there is nothing to look up. (The old guard only broke
    /// the loop *after* the first fd-table walk, so a fully filtered scan
    /// still paid ~12k readlinks for nothing.)
    #[test]
    fn test_empty_interest_skips_the_fs_walk() {
        let nowhere = tempfile::tempdir().unwrap().path().join("does-not-exist");
        let map = build_inode_map(&nowhere, &HashSet::new()).unwrap();
        assert!(map.is_empty());
    }
}
