//! Side effects Portix performs on the world.
//!
//! Killing lives on `Engine::signal_service`, because it needs the platform
//! probe to re-verify process identity before signalling. Everything here is
//! spawn-and-forget.
//!
//! Every external command is invoked argv-style. There is no `sh -c` anywhere in
//! this file, and there must never be: a project name is attacker-controlled
//! text, and a shell would happily interpret it.

use anyhow::{anyhow, Context, Result};
use std::process::Command;

/// Open a URL in the default browser.
///
/// The single outbound action Portix performs. Invoked argv-style — never
/// `sh -c` — so a URL derived from attacker-controlled text cannot be a shell
/// injection. Callers pass a `localhost` URL built by `Socket::url`.
pub fn open_url(url: &str) -> Result<()> {
    Command::new("xdg-open")
        .arg(url)
        .spawn()
        .with_context(|| format!("Failed to open {url}"))?;

    Ok(())
}

/// Copy text to clipboard, trying Wayland first and X11 second.
pub fn copy_to_clipboard(text: &str) -> Result<()> {
    // `status()` returning Ok only means the child ran; a missing selection
    // owner or a broken display still exits non-zero, so check the code too.
    let ran_successfully = |cmd: &mut Command| cmd.status().map(|s| s.success()).unwrap_or(false);

    if ran_successfully(Command::new("wl-copy").arg(text)) {
        return Ok(());
    }

    if ran_successfully(
        Command::new("xclip")
            .args(["-selection", "clipboard"])
            .arg(text),
    ) {
        return Ok(());
    }

    Err(anyhow!(
        "No clipboard tool found (install wl-copy or xclip)"
    ))
}
