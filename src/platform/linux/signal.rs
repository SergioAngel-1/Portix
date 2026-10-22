use crate::model::Signal;
use anyhow::{anyhow, Result};
use std::fs::File;
use std::os::unix::io::{AsRawFd, FromRawFd};
use std::path::Path;

impl Signal {
    fn as_libc(self) -> i32 {
        match self {
            Signal::Term => libc::SIGTERM,
            Signal::Kill => libc::SIGKILL,
        }
    }
}

/// Send a signal to a single process, after proving it is still the process we
/// meant to signal.
///
/// PIDs are recycled. Between the snapshot the user is looking at and the
/// keystroke that kills, the kernel may have handed this PID to something else.
///
/// The signal goes through a **pidfd** (`pidfd_open` + `pidfd_send_signal`,
/// Linux ≥ 5.3): the fd pins the exact process that owns the PID at open time
/// and keeps referring to it even if the PID is recycled before the signal
/// lands — a recycled `kill(pid)` would hit the *new* owner, a
/// `pidfd_send_signal` on a dead pin returns ESRCH instead. The identity check
/// runs both before the open (cheap snapshot reject) and after it (against
/// the *pinned* process: if the pin is a different or dead process, the
/// `/proc` starttime read either mismatches or fails — refuse in both cases).
///
/// On kernels without `pidfd_open` (pre-5.3, ENOSYS/EINVAL) it falls back to
/// the starttime re-check + `kill(2)` — the original microsecond window, and
/// the best those kernels allow.
///
/// We signal the process, never the process group: `kill(-pgid, …)` takes down
/// every sibling sharing the group, which for a dev server launched from an
/// interactive shell can include the shell itself.
pub fn send_signal(proc_root: &Path, pid: i32, expected_starttime: u64, sig: Signal) -> Result<()> {
    // Cheap snapshot check first: most mismatches are caught here without
    // touching a syscall.
    verify_identity(proc_root, pid, expected_starttime)?;

    // Pin the process. From here on the fd, not the PID, is the identity.
    // SAFETY: pidfd_open(2) with a positive pid and flags=0; the fd is owned
    // by the File we wrap it in immediately.
    #[allow(unsafe_code)]
    let fd = unsafe { libc::syscall(libc::SYS_pidfd_open, pid, 0) };

    if fd < 0 {
        let err = std::io::Error::last_os_error();
        return match err.raw_os_error() {
            // ENOSYS: kernel without pidfd_open (pre-5.3) — fall back to the
            // starttime re-check + kill(2), the best those kernels allow.
            Some(libc::ENOSYS) => signal_via_kill(proc_root, pid, expected_starttime, sig),
            // EINVAL is *not* "no pidfd support": pidfd_open returns it only
            // for an invalid pid (pid <= 0). Treat it as a hard error — never
            // weaken the identity guarantee by falling back to kill(2) for a
            // pid we could not pin. (A pid read from /proc is always valid,
            // so this is defensive; a recycled-beyond-pid_max pid surfaces as
            // ESRCH below instead.)
            Some(libc::EINVAL) => Err(anyhow!(
                "refusing to signal PID {pid}: pidfd_open rejected it with EINVAL"
            )),
            // ESRCH and everything else: the process is gone.
            _ => Err(err.into()),
        };
    }
    // SAFETY: from_raw_fd takes ownership of the fd returned by pidfd_open;
    // the File closes it on drop.
    #[allow(unsafe_code)]
    let pidfd = unsafe { File::from_raw_fd(fd as i32) };
    // Re-verify identity against the *pinned* process. If the PID was
    // recycled between the snapshot check and pidfd_open, the pin is the new
    // process and /proc now shows it: mismatch, refuse. If the pinned process
    // died meanwhile, the read fails or shows a recycled PID: mismatch, refuse.
    verify_identity(proc_root, pid, expected_starttime)?;

    // SAFETY: pidfd_send_signal(2) on a live pidfd we own, with a valid signal
    // and a NULL siginfo (default semantics, like kill). If the pinned process
    // died after the check, this returns ESRCH — it can never reach a recycled
    // PID's new owner.
    #[allow(unsafe_code)]
    let rc = unsafe {
        libc::syscall(
            libc::SYS_pidfd_send_signal,
            pidfd.as_raw_fd(),
            sig.as_libc(),
            std::ptr::null::<libc::siginfo_t>(),
            0,
        )
    };

    if rc == -1 {
        return Err(std::io::Error::last_os_error().into());
    }
    Ok(())
}

/// The pre-pidfd path: re-read starttime immediately before `kill(2)`.
///
/// Only reached on kernels without `pidfd_open`. The re-read is the same
/// TOCTOU guard as before, with the unavoidable microsecond window between
/// the read and the syscall.
fn signal_via_kill(proc_root: &Path, pid: i32, expected_starttime: u64, sig: Signal) -> Result<()> {
    verify_identity(proc_root, pid, expected_starttime)?;

    // SAFETY: kill(2) with a positive pid and a valid signal number, on a pid
    // we just verified to be the process we intend to signal.
    #[allow(unsafe_code)]
    let rc = unsafe { libc::kill(pid, sig.as_libc()) };

    if rc == -1 {
        return Err(std::io::Error::last_os_error().into());
    }
    Ok(())
}

/// Re-read the starttime and refuse unless it still matches `expected`.
///
/// Three checkpoints share this check (pre-open, post-`pidfd_open`, and the
/// `kill(2)` fallback). Keeping it in one place keeps the messages identical:
/// "no longer exists" for a vanished PID, "replaced by a different process"
/// for a recycled one, whichever checkpoint saw it.
fn verify_identity(proc_root: &Path, pid: i32, expected: u64) -> Result<()> {
    let actual = super::proc::read_starttime_jiffies(proc_root, pid)
        .ok_or_else(|| anyhow!("process {pid} no longer exists"))?;

    if actual != expected {
        return Err(anyhow!(
            "refusing to signal PID {pid}: it was replaced by a different process \
             (start time {actual} != {expected})"
        ));
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

    /// The pidfd path against a real process: the pinned child must receive
    /// the signal and die. Uses the real /proc — a synthetic root has no live
    /// processes for pidfd_open to pin.
    #[test]
    fn test_pidfd_signals_a_real_child_process() {
        use std::process::Command;
        let mut child = Command::new("sleep").arg("30").spawn().unwrap();
        let pid = child.id() as i32;
        let starttime = super::super::proc::read_starttime_jiffies(Path::new("/proc"), pid)
            .expect("a live child must have a starttime");

        send_signal(Path::new("/proc"), pid, starttime, Signal::Kill).unwrap();

        let status = child.wait().unwrap();
        assert!(
            status.code().is_none(),
            "child must die by signal, not by exit: {status:?}"
        );
    }

    /// The identity check runs before any signalling syscall: a wrong
    /// starttime on a real process must be refused without killing it.
    #[test]
    fn test_pidfd_refuses_wrong_starttime_on_a_real_process() {
        use std::process::Command;
        let mut child = Command::new("sleep").arg("30").spawn().unwrap();
        let pid = child.id() as i32;

        // starttime 1 is impossible for a real process (the kernel itself
        // takes more than one jiffy to boot), so this always mismatches.
        let err = send_signal(Path::new("/proc"), pid, 1, Signal::Term).unwrap_err();
        assert!(
            err.to_string().contains("replaced by a different process"),
            "unexpected error: {err}"
        );

        // The child must still be alive: the refusal happened before any
        // signal was sent.
        assert!(
            child.try_wait().unwrap().is_none(),
            "refused signal must not kill the child"
        );
        let _ = child.kill();
        let _ = child.wait();
    }
}
