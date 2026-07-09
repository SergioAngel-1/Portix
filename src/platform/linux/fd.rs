use anyhow::Result;
use std::collections::{HashMap, HashSet};
use std::path::Path;

/// Build a map from inode to (pid, fd) by scanning /proc/<pid>/fd/*.
///
/// This is the critical optimization: instead of O(sockets × processes × fds),
/// we do a single pass through all processes and build the reverse map.
pub fn build_inode_map(
    proc_root: &Path,
    interest_inodes: &HashSet<u64>,
    current_uid: u32,
) -> Result<HashMap<u64, Vec<(i32, i32)>>> {
    let mut map: HashMap<u64, Vec<(i32, i32)>> = HashMap::new();
    let mut found_count = 0usize;
    let total_interest = interest_inodes.len();

    // Read all PIDs
    let pids = super::proc::list_pids(proc_root)?;

    for pid in pids {
        // Optimization: skip processes not owned by us (unless root)
        if current_uid != 0 && !is_process_owned_by(proc_root, pid, current_uid) {
            continue;
        }

        let fd_dir = proc_root.join(pid.to_string()).join("fd");
        if let Ok(entries) = std::fs::read_dir(&fd_dir) {
            for entry in entries.flatten() {
                if let Ok(target) = std::fs::read_link(entry.path()) {
                    if let Some(target_str) = target.to_str() {
                        if let Some(inode_str) = target_str.strip_prefix("socket:[") {
                            if let Some(inode_str) = inode_str.strip_suffix(']') {
                                if let Ok(inode) = inode_str.parse::<u64>() {
                                    if interest_inodes.contains(&inode) {
                                        let fd_num = entry
                                            .file_name()
                                            .to_str()
                                            .and_then(|s| s.parse::<i32>().ok())
                                            .unwrap_or(0);
                                        map.entry(inode)
                                            .or_insert_with(Vec::new)
                                            .push((pid, fd_num));
                                        found_count += 1;
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

        // Early termination if we found all inodes
        if found_count >= total_interest {
            break;
        }
    }

    Ok(map)
}

/// Check if a process is owned by a specific UID.
fn is_process_owned_by(proc_root: &Path, pid: i32, uid: u32) -> bool {
    let status_path = proc_root.join(pid.to_string()).join("status");
    if let Ok(content) = std::fs::read_to_string(&status_path) {
        for line in content.lines() {
            if let Some(value) = line.strip_prefix("Uid:") {
                // Format: "Uid:\t1000\t1000\t1000\t1000"
                if let Some(first) = value.split_whitespace().next() {
                    if let Ok(file_uid) = first.parse::<u32>() {
                        return file_uid == uid;
                    }
                }
            }
        }
    }
    false
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

        let map = build_inode_map(proc_root, &interest, 1000).unwrap();
        assert!(map.contains_key(&486312));
        assert_eq!(map[&486312][0].0, 1234);
    }
}
