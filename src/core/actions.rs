//! Side effects Portix performs on the world.
//!
//! Killing lives on `Engine::signal_service`, because it needs the platform
//! probe to re-verify process identity before signalling. Action commands are
//! short-lived and their output is kept out of the alternate-screen TUI.
//!
//! Every external command is invoked argv-style. There is no `sh -c` anywhere in
//! this file, and there must never be: a project name is attacker-controlled
//! text, and a shell would happily interpret it.

use anyhow::{anyhow, Context, Result};
use std::env;
use std::path::Path;
use std::process::{Command, Stdio};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BrowserLauncher {
    KioClient,
    KdeOpen,
    XdgOpen,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ClipboardBackend {
    KlipperQdbus6,
    KlipperQdbus,
    KlipperDbusSend,
    WlCopy,
    Xclip,
    Xsel,
}

fn select_browser_launcher(
    is_kde: bool,
    kioclient_available: bool,
    kde_open_available: bool,
) -> BrowserLauncher {
    if is_kde && kioclient_available {
        BrowserLauncher::KioClient
    } else if is_kde && kde_open_available {
        BrowserLauncher::KdeOpen
    } else {
        BrowserLauncher::XdgOpen
    }
}

/// Whether the current environment identifies a KDE/Plasma session.
///
/// `xdg-open` still falls back to the obsolete `kfmclient` when
/// `KDE_SESSION_VERSION` is missing. Plasma 6 sessions can legitimately omit
/// that variable, while `XDG_CURRENT_DESKTOP` or `DESKTOP_SESSION` still
/// identifies Plasma. Detect those signals before invoking `xdg-open`.
fn looks_like_kde_session(
    xdg_current_desktop: Option<&str>,
    desktop_session: Option<&str>,
    kde_full_session: Option<&str>,
) -> bool {
    let current_desktop = xdg_current_desktop.unwrap_or("");
    current_desktop
        .split(':')
        .any(|desktop| desktop.eq_ignore_ascii_case("kde"))
        || desktop_session
            .unwrap_or("")
            .to_ascii_lowercase()
            .contains("plasma")
        || kde_full_session.is_some_and(|value| !value.is_empty())
}

fn command_available(command: &str) -> bool {
    if command.contains('/') {
        return Path::new(command).is_file();
    }

    env::var_os("PATH")
        .into_iter()
        .flat_map(|path| env::split_paths(&path).collect::<Vec<_>>())
        .map(|directory| directory.join(command))
        .any(|path| path.is_file())
}

fn choose_browser_command(
    explicit: Option<&str>,
    is_kde: bool,
    available: impl Fn(&str) -> bool,
) -> Option<String> {
    if let Some(browser) = explicit
        .map(str::trim)
        .filter(|browser| !browser.is_empty())
        .filter(|browser| !browser.contains(char::is_whitespace))
        .filter(|browser| available(browser))
    {
        return Some(browser.to_string());
    }

    if is_kde {
        return [
            "google-chrome-stable",
            "google-chrome",
            "chromium",
            "firefox",
        ]
        .into_iter()
        .find(|command| available(command))
        .map(str::to_string);
    }

    None
}

fn browser_command() -> Option<String> {
    let is_kde = env::var("XDG_CURRENT_DESKTOP").is_ok_and(|desktop| {
        desktop
            .split(':')
            .any(|name| name.eq_ignore_ascii_case("kde"))
    });
    choose_browser_command(
        env::var("BROWSER").ok().as_deref(),
        is_kde,
        command_available,
    )
}

fn select_clipboard_backend(
    is_kde: bool,
    qdbus6_available: bool,
    qdbus_available: bool,
    dbus_send_available: bool,
    wl_copy_available: bool,
    xclip_available: bool,
    xsel_available: bool,
) -> Option<ClipboardBackend> {
    if is_kde && qdbus6_available {
        Some(ClipboardBackend::KlipperQdbus6)
    } else if is_kde && qdbus_available {
        Some(ClipboardBackend::KlipperQdbus)
    } else if is_kde && dbus_send_available {
        Some(ClipboardBackend::KlipperDbusSend)
    } else if wl_copy_available {
        Some(ClipboardBackend::WlCopy)
    } else if xclip_available {
        Some(ClipboardBackend::Xclip)
    } else if xsel_available {
        Some(ClipboardBackend::Xsel)
    } else {
        None
    }
}

fn run_clipboard_backend(backend: ClipboardBackend, text: &str) -> bool {
    let mut command = match backend {
        ClipboardBackend::KlipperQdbus6 => {
            let mut command = Command::new("qdbus6");
            command.args([
                "org.kde.klipper",
                "/klipper",
                "org.kde.klipper.klipper.setClipboardContents",
                text,
            ]);
            command
        }
        ClipboardBackend::KlipperQdbus => {
            let mut command = Command::new("qdbus");
            command.args([
                "org.kde.klipper",
                "/klipper",
                "org.kde.klipper.klipper.setClipboardContents",
                text,
            ]);
            command
        }
        ClipboardBackend::KlipperDbusSend => {
            let mut command = Command::new("dbus-send");
            command.args([
                "--session",
                "--type=method_call",
                "--dest=org.kde.klipper",
                "/klipper",
                "org.kde.klipper.klipper.setClipboardContents",
            ]);
            command.arg(format!("string:{text}"));
            command
        }
        ClipboardBackend::WlCopy => {
            let mut command = Command::new("wl-copy");
            command.arg(text);
            command
        }
        ClipboardBackend::Xclip => {
            let mut command = Command::new("xclip");
            command.args(["-selection", "clipboard", "-in"]);
            command.stdin(Stdio::piped());
            command
        }
        ClipboardBackend::Xsel => {
            let mut command = Command::new("xsel");
            command.args(["--clipboard", "--input"]);
            command.stdin(Stdio::piped());
            command
        }
    };

    match backend {
        ClipboardBackend::Xclip | ClipboardBackend::Xsel => {
            let Some(mut child) = command
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .ok()
            else {
                return false;
            };
            let Some(mut stdin) = child.stdin.take() else {
                return false;
            };
            use std::io::Write;
            if stdin.write_all(text.as_bytes()).is_err() {
                return false;
            }
            drop(stdin);
            child.wait().is_ok_and(|status| status.success())
        }
        _ => command
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|status| status.success()),
    }
}

/// Configure a child launcher so it cannot write over the alternate-screen TUI.
fn silence_launcher_output(command: &mut Command) -> &mut Command {
    command.stdout(Stdio::null()).stderr(Stdio::null())
}

/// Open a URL in the default browser.
///
/// The single outbound action Portix performs. Invoked argv-style — never
/// `sh -c` — so a URL derived from attacker-controlled text cannot be a shell
/// injection. Callers pass a `localhost` URL built by `Socket::url`.
///
/// Plasma 6 provides `kioclient`, while some `xdg-open` versions still call
/// the removed `kfmclient` when KDE's session-version variable is absent. Use
/// the native opener for KDE and keep `xdg-open` as the general fallback.
/// Launcher output is discarded because Portix is normally in raw mode and on
/// the alternate screen when this effect runs.
pub fn open_url(url: &str) -> Result<()> {
    let is_kde = looks_like_kde_session(
        env::var("XDG_CURRENT_DESKTOP").ok().as_deref(),
        env::var("DESKTOP_SESSION").ok().as_deref(),
        env::var("KDE_FULL_SESSION").ok().as_deref(),
    );

    // The desktop opener reports success even when its browser integration is
    // unable to hand the URL to an already-running browser. Prefer the explicit
    // browser command when one is discoverable; unlike `kioclient`, Chrome's
    // launcher has a direct, testable `--new-tab URL` contract.
    let mut command = if let Some(browser) = browser_command() {
        let mut command = Command::new(browser);
        command.args(["--new-tab", url]);
        command
    } else {
        let launcher = select_browser_launcher(
            is_kde,
            command_available("kioclient"),
            command_available("kde-open"),
        );
        match launcher {
            BrowserLauncher::KioClient => {
                let mut command = Command::new("kioclient");
                command.args(["exec", url]);
                command
            }
            BrowserLauncher::KdeOpen => {
                let mut command = Command::new("kde-open");
                command.arg(url);
                command
            }
            BrowserLauncher::XdgOpen => {
                let mut command = Command::new("xdg-open");
                command.arg(url);
                command
            }
        }
    };

    // Keep stdin inherited for desktop/browser session integration, but silence
    // output so launcher diagnostics cannot overwrite the alternate screen.
    silence_launcher_output(&mut command)
        .spawn()
        .with_context(|| format!("Failed to open {url}"))?;

    Ok(())
}

/// Copy text to the desktop clipboard.
///
/// Plasma exposes Klipper over the session bus, so KDE users do not need an
/// extra `wl-clipboard`/`xclip` package. Other desktops use the conventional
/// Wayland/X11 helpers when installed. Each backend is attempted in order and
/// a failing backend does not prevent a later fallback from being tried.
pub fn copy_to_clipboard(text: &str) -> Result<()> {
    let is_kde = looks_like_kde_session(
        env::var("XDG_CURRENT_DESKTOP").ok().as_deref(),
        env::var("DESKTOP_SESSION").ok().as_deref(),
        env::var("KDE_FULL_SESSION").ok().as_deref(),
    );

    let candidates = [
        select_clipboard_backend(
            is_kde,
            command_available("qdbus6"),
            command_available("qdbus"),
            command_available("dbus-send"),
            command_available("wl-copy"),
            command_available("xclip"),
            command_available("xsel"),
        ),
        command_available("wl-copy").then_some(ClipboardBackend::WlCopy),
        command_available("xclip").then_some(ClipboardBackend::Xclip),
        command_available("xsel").then_some(ClipboardBackend::Xsel),
    ];

    let mut attempted = false;
    for backend in candidates.into_iter().flatten() {
        attempted = true;
        if run_clipboard_backend(backend, text) {
            return Ok(());
        }
    }

    if attempted {
        Err(anyhow!(
            "Clipboard backend failed (is the desktop session available?)"
        ))
    } else {
        Err(anyhow!(
            "No clipboard tool found (install wl-clipboard, xclip or xsel)"
        ))
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn detects_kde_from_xdg_desktop_without_kde_session_version() {
        assert!(super::looks_like_kde_session(
            Some("KDE"),
            Some("plasma.desktop"),
            None,
        ));
    }

    #[test]
    fn detects_kde_when_xdg_desktop_contains_multiple_desktops() {
        assert!(super::looks_like_kde_session(Some("KDE:GNOME"), None, None,));
    }

    #[test]
    fn does_not_treat_gnome_as_kde() {
        assert!(!super::looks_like_kde_session(
            Some("GNOME"),
            Some("gnome.desktop"),
            None,
        ));
    }

    #[test]
    fn selects_kio_client_for_kde_when_available() {
        assert_eq!(
            super::select_browser_launcher(true, true, true),
            super::BrowserLauncher::KioClient
        );
    }

    #[test]
    fn selects_xdg_open_outside_kde() {
        assert_eq!(
            super::select_browser_launcher(false, true, true),
            super::BrowserLauncher::XdgOpen
        );
    }

    #[test]
    fn accepts_a_valid_explicit_browser_command() {
        assert_eq!(
            super::choose_browser_command(Some("browser"), false, |name| name == "browser"),
            Some("browser".to_string())
        );
    }

    #[test]
    fn ignores_browser_values_that_contain_arguments() {
        assert_eq!(
            super::choose_browser_command(Some("browser --new-tab"), true, |_| true),
            Some("google-chrome-stable".to_string())
        );
    }

    #[test]
    fn selects_klipper_qdbus6_on_plasma() {
        assert_eq!(
            super::select_clipboard_backend(true, true, true, true, false, false, false),
            Some(super::ClipboardBackend::KlipperQdbus6)
        );
    }

    #[test]
    fn falls_back_to_wayland_clipboard_outside_kde() {
        assert_eq!(
            super::select_clipboard_backend(false, false, false, false, true, false, false),
            Some(super::ClipboardBackend::WlCopy)
        );
    }

    #[test]
    fn open_url_passes_the_url_to_the_selected_browser() {
        use std::os::unix::fs::PermissionsExt;
        use std::time::Duration;

        let dir = tempfile::tempdir().expect("temporary browser directory");
        let script = dir.path().join("browser");
        let log = dir.path().join("args");
        std::fs::write(
            &script,
            format!("#!/bin/sh\nprintf '%s\\n' \"$@\" > '{}'\n", log.display()),
        )
        .expect("write browser test helper");
        let mut permissions = std::fs::metadata(&script).unwrap().permissions();
        permissions.set_mode(0o700);
        std::fs::set_permissions(&script, permissions).unwrap();

        let previous = std::env::var_os("BROWSER");
        std::env::set_var("BROWSER", &script);
        super::open_url("http://localhost:5173/?a=one;two").expect("spawn browser");

        for _ in 0..50 {
            if log.exists() {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }

        match previous {
            Some(value) => std::env::set_var("BROWSER", value),
            None => std::env::remove_var("BROWSER"),
        }

        let args = std::fs::read_to_string(log).expect("browser receives arguments");
        assert_eq!(args, "--new-tab\nhttp://localhost:5173/?a=one;two\n");
    }

    #[test]
    fn copy_to_clipboard_uses_plasma_klipper_when_external_tools_are_absent() {
        if !super::command_available("qdbus6")
            || !super::looks_like_kde_session(
                std::env::var("XDG_CURRENT_DESKTOP").ok().as_deref(),
                std::env::var("DESKTOP_SESSION").ok().as_deref(),
                std::env::var("KDE_FULL_SESSION").ok().as_deref(),
            )
        {
            return;
        }

        let marker = format!("portix-clipboard-regression-{}", std::process::id());
        super::copy_to_clipboard(&marker).expect("Klipper clipboard backend should work");

        let output = std::process::Command::new("qdbus6")
            .args([
                "org.kde.klipper",
                "/klipper",
                "org.kde.klipper.klipper.getClipboardContents",
            ])
            .output()
            .expect("qdbus6 should be callable");
        assert!(output.status.success());
        assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), marker);
    }
}
