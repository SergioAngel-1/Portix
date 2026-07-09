use crate::model::Signal;
use anyhow::{anyhow, Result};
use std::path::Path;

impl Signal {
    fn as_libc(self) -> i32 {
        match self {
            Signal::Term => libc::SIGTERM,
            Signal::Kill => libc::SIGKILL,
            Signal::Interrupt => libc::SIGINT,
        }
    }
}

/// Send a signal to a single process, after proving it is still the process we
/// meant to signal.
///
/// PIDs are recycled. Between the snapshot the user is looking at and the
/// keystroke that kills, the kernel may have handed this PID to something else.
/// Re-reading `starttime` and comparing it against the value captured in the
/// snapshot closes that TOCTOU window for the price of one file read.
///
/// We signal the process, never the process group: `kill(-pgid, …)` takes down
/// every sibling sharing the group, which for a dev server launched from an
/// interactive shell can include the shell itself.
pub fn send_signal(proc_root: &Path, pid: i32, expected_starttime: u64, sig: Signal) -> Result<()> {
    let actual = super::proc::read_starttime_jiffies(proc_root, pid)
        .ok_or_else(|| anyhow!("process {pid} no longer exists"))?;

    if actual != expected_starttime {
        return Err(anyhow!(
            "refusing to signal PID {pid}: it was replaced by a different process \
             (start time {actual} != {expected_starttime})"
        ));
    }

    // SAFETY: kill(2) with a positive pid and a valid signal number, on a pid we
    // just verified to be the process we intend to signal.
    #[allow(unsafe_code)]
    let rc = unsafe { libc::kill(pid, sig.as_libc()) };

    if rc == -1 {
        return Err(std::io::Error::last_os_error().into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fake_proc(pid: i32, starttime: u64) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("stat"), "cpu  1 2 3\nbtime 1700000000\n").unwrap();
        let pid_dir = dir.path().join(pid.to_string());
        std::fs::create_dir(&pid_dir).unwrap();
        let stat = format!("{pid} (svc) S 1 1 1 0 -1 0 0 0 0 0 10 20 0 0 20 0 1 0 {starttime} 0 0");
        std::fs::write(pid_dir.join("stat"), stat).unwrap();
        dir
    }

    #[test]
    fn test_refuses_to_signal_a_recycled_pid() {
        // The snapshot saw starttime 111; /proc now reports 222 => different process.
        let dir = fake_proc(4242, 222);
        let err = send_signal(dir.path(), 4242, 111, Signal::Term).unwrap_err();
        assert!(err.to_string().contains("replaced by a different process"));
    }

    #[test]
    fn test_refuses_to_signal_a_vanished_process() {
        let dir = fake_proc(4242, 111);
        let err = send_signal(dir.path(), 9999, 111, Signal::Term).unwrap_err();
        assert!(err.to_string().contains("no longer exists"));
    }
}
