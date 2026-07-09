use crate::model::Signal;
use anyhow::Result;

/// Send a signal to a process.
/// This is a safe wrapper around the signal system call.
pub fn send_signal(pid: i32, sig: Signal) -> Result<()> {
    let signal = match sig {
        Signal::Term => 15, // SIGTERM
        Signal::Kill => 9,  // SIGKILL
        Signal::Interrupt => 2, // SIGINT
    };

    // Use Command to send signal safely
    let output = std::process::Command::new("kill")
        .arg(format!("-{}", signal))
        .arg(pid.to_string())
        .output()?;

    if output.status.success() {
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        Err(anyhow::anyhow!("Failed to send signal: {}", stderr))
    }
}

/// Send a signal to a process group.
pub fn send_signal_to_group(pgid: i32, sig: Signal) -> Result<()> {
    let signal = match sig {
        Signal::Term => 15, // SIGTERM
        Signal::Kill => 9,  // SIGKILL
        Signal::Interrupt => 2, // SIGINT
    };

    // Use negative PID for process group
    let output = std::process::Command::new("kill")
        .arg(format!("-{}", signal))
        .arg((-pgid).to_string())
        .output()?;

    if output.status.success() {
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        Err(anyhow::anyhow!("Failed to send signal to group: {}", stderr))
    }
}

/// Get the process group ID for a PID.
pub fn get_pgid(pid: i32) -> Result<i32> {
    let output = std::process::Command::new("ps")
        .arg("-o")
        .arg("pgid=")
        .arg("-p")
        .arg(pid.to_string())
        .output()?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let pgid_str = stdout.trim();

    pgid_str
        .parse::<i32>()
        .map_err(|e| anyhow::anyhow!("Failed to parse PGID: {}", e))
}
