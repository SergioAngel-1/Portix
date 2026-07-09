use crate::model::{Service, Signal};
use anyhow::{Context, Result};
use std::process::Command;

/// Actions that can be performed on a service.

/// Kill a service by sending SIGTERM to its process group.
pub fn kill_service(service: &Service, force: bool) -> Result<()> {
    let process = service
        .process
        .as_ref()
        .context("No process information available")?;

    let pid = process.pid;

    // Get the process group ID
    let pgid = unsafe { libc::getpgid(pid) };
    if pgid == -1 {
        return Err(std::io::Error::last_os_error().into());
    }

    // Send signal to the process group (negative PID)
    let signal = if force {
        libc::SIGKILL
    } else {
        libc::SIGTERM
    };

    let result = unsafe { libc::kill(-pgid, signal) };
    if result == -1 {
        return Err(std::io::Error::last_os_error().into());
    }

    Ok(())
}

/// Open a service URL in the default browser.
pub fn open_in_browser(service: &Service) -> Result<()> {
    let url = service.socket.url();

    // Use xdg-open on Linux
    Command::new("xdg-open")
        .arg(&url)
        .spawn()
        .with_context(|| format!("Failed to open {}", url))?;

    Ok(())
}

/// Copy text to clipboard.
pub fn copy_to_clipboard(text: &str) -> Result<()> {
    // Try wl-copy (Wayland)
    if Command::new("wl-copy").arg(text).status().is_ok() {
        return Ok(());
    }

    // Try xclip (X11)
    if Command::new("xclip")
        .args(["-selection", "clipboard"])
        .arg(text)
        .status()
        .is_ok()
    {
        return Ok(());
    }

    Err(anyhow::anyhow!("No clipboard tool found (install wl-copy or xclip)"))
}

/// Get the URL for a service.
pub fn get_url(service: &Service) -> String {
    service.socket.url()
}

/// Get the port for a service.
pub fn get_port(service: &Service) -> u16 {
    service.socket.port
}

/// Get the command for a service.
pub fn get_command(service: &Service) -> String {
    service
        .process
        .as_ref()
        .map(|p| p.command_display())
        .unwrap_or_default()
}

/// Get the project path for a service.
pub fn get_project_path(service: &Service) -> String {
    service
        .project
        .as_ref()
        .map(|p| p.root.to_string_lossy().to_string())
        .unwrap_or_default()
}

/// Get the PID for a service.
pub fn get_pid(service: &Service) -> Option<i32> {
    service.process.as_ref().map(|p| p.pid)
}
