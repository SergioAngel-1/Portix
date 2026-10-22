use anyhow::Result;
use std::collections::{HashMap, HashSet};
use std::path::Path;

/// Parse the inode out of an fd symlink target.
///
/// Only `socket:[N]` counts: spelling the whole shape out keeps pipes,
/// eventfds and truncated targets (`socket:[1`) from ever looking like sockets.
fn parse_socket_inode(target: &Path) -> Option<u64> {
    target
        .to_str()?
        .strip_prefix("socket:[")?
        .strip_suffix(']')?
        .parse()
        .ok()
}

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

    // Walk PIDs in DESCENDING order. Between full rebuilds the engine only
    // asks for newly appeared sockets, and those almost always belong to
    // recently started processes — the highest PIDs. Resolving those first
    // lets the "all interest resolved" exit below fire after a few fd tables
    // instead of after the whole box; a full rebuild has to visit every pid
    // anyway, so it costs exactly what it did before.
    let mut pids = super::proc::list_pids(proc_root)?;
    pids.sort_unstable_by(|a, b| b.cmp(a));

    for pid in pids {
        let fd_dir = proc_root.join(pid.to_string()).join("fd");
        if let Ok(entries) = std::fs::read_dir(&fd_dir) {
            for entry in entries.flatten() {
                // Parse the fd number before touching the link target: a
                // non-numeric entry (a stray file in a test fixture, a race
                // with a closed fd) must not cost a readlink syscall.
                let Some(fd_num) = entry
                    .file_name()
                    .to_str()
                    .and_then(|s| s.parse::<i32>().ok())
                else {
                    continue;
                };
                let Ok(target) = std::fs::read_link(entry.path()) else {
                    continue;
                };
                let Some(inode) = parse_socket_inode(&target) else {
                    continue;
                };
                if interest_inodes.contains(&inode) {
                    map.entry(inode).or_default().push((pid, fd_num));
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

    /// Only `socket:[N]` is a socket. Pipes, eventfds, a truncated target and
    /// a non-numeric inode must all be rejected without pretending to be one.
    #[test]
    fn test_parse_socket_inode_accepts_sockets_only() {
        use std::path::PathBuf;
        assert_eq!(
            parse_socket_inode(&PathBuf::from("socket:[123]")),
            Some(123)
        );
        for target in [
            "pipe:[1]",
            "socket:[x]",
            "socket:[1",
            "socket:1]",
            "anon_inode:[eventfd]",
            "/run/socket:[7]",
        ] {
            assert_eq!(
                parse_socket_inode(&PathBuf::from(target)),
                None,
                "{target} must not parse as a socket inode"
            );
        }
    }

    /// The descending walk must not lose sockets owned by low PIDs: a full
    /// rebuild sees every pid, so both ends of the range resolve.
    #[test]
    fn test_low_and_high_pid_inodes_both_resolve() {
        let temp_dir = tempfile::tempdir().unwrap();
        let proc_root = temp_dir.path();

        for (pid, inode) in [(100u32, 111u64), (9000, 222)] {
            let fd_dir = proc_root.join(pid.to_string()).join("fd");
            std::fs::create_dir_all(&fd_dir).unwrap();
            std::os::unix::fs::symlink(format!("socket:[{inode}]"), fd_dir.join("3")).unwrap();
        }

        let interest = HashSet::from([111, 222]);
        let map = build_inode_map(proc_root, &interest).unwrap();

        assert_eq!(map[&111][0].0, 100, "low-pid socket must resolve");
        assert_eq!(map[&222][0].0, 9000, "high-pid socket must resolve");
        assert_eq!(map.len(), 2);
    }
}
