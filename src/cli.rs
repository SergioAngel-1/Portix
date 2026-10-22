//! Non-interactive command paths: JSON, plain-text and one-shot output, the
//! `--kill` flow and `--update`.
//!
//! These live in their own bin-private module so `main.rs` is about the
//! runtime (terminal, panic safety, collector and event loop) rather than
//! output formatting. They are not library API.

use anyhow::{Context, Result};
use serde::Serialize;
use std::io::{self, IsTerminal};
use std::time::Duration;

use portix_lib::core::Engine;
use portix_lib::model::{Service, SharedSnapshot, Signal};
use portix_lib::ui::format::{cell, format_bytes_compact, format_uptime, truncate_center};

/// The JSON contract. Consumers switch on `schema_version`; unknown fields must
/// be ignored. Adding a field is a minor bump, removing or retyping one is major.
pub const SCHEMA_VERSION: u32 = 1;

/// Exit codes, as documented in PRD §12.2. Scripts rely on these:
/// `portix :3000 || pnpm dev`.
pub mod exit {
    pub const OK: i32 = 0;
    pub const ERROR: i32 = 1;
    pub const NO_MATCH: i32 = 3;
}

/// A snapshot as emitted by `--json`, wrapped with its schema version.
#[derive(Serialize)]
struct JsonOutput<'a> {
    schema_version: u32,
    #[serde(flatten)]
    snapshot: &'a portix_lib::model::Snapshot,
}

#[derive(Serialize)]
struct JsonService<'a> {
    schema_version: u32,
    #[serde(flatten)]
    service: &'a Service,
}

/// CPU needs two samples to produce a delta. A one-shot invocation would print
/// a dash for every row, so take a throwaway scan and wait one interval.
fn scan_with_cpu(engine: &mut Engine) -> SharedSnapshot {
    engine.scan();
    std::thread::sleep(Duration::from_millis(200));
    engine.scan()
}

/// Like [`scan_with_cpu`], but the samples ignore the display filters.
///
/// The CLI query paths (`portix 80`, `portix --json 80`) answer a direct
/// question about a specific port: hiding system ports or UDP is a table
/// preference, and applying it here would report "no service" for a port that
/// is actually listening.
fn scan_with_cpu_unfiltered(engine: &mut Engine) -> SharedSnapshot {
    engine.scan_unfiltered();
    std::thread::sleep(Duration::from_millis(200));
    engine.scan_unfiltered()
}

pub fn print_json(engine: &mut Engine, port: Option<u16>) -> Result<i32> {
    // A port query answers a direct question, so it scans without the display
    // filters; the full-snapshot form keeps them — it is a display.
    let snapshot = match port {
        Some(_) => scan_with_cpu_unfiltered(engine),
        None => scan_with_cpu(engine),
    };

    // stdout carries JSON and nothing else; warnings go to stderr.
    match port {
        Some(port) => match snapshot.by_port(port) {
            Some(service) => {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&JsonService {
                        schema_version: SCHEMA_VERSION,
                        service,
                    })?
                );
                Ok(exit::OK)
            }
            None => {
                eprintln!("No service listening on port {port}");
                Ok(exit::NO_MATCH)
            }
        },
        None => {
            println!(
                "{}",
                serde_json::to_string_pretty(&JsonOutput {
                    schema_version: SCHEMA_VERSION,
                    snapshot: &snapshot,
                })?
            );
            Ok(exit::OK)
        }
    }
}

pub fn print_port(engine: &mut Engine, port: u16) -> Result<i32> {
    // Unfiltered: the user asked about a specific port; `show_system_ports`
    // and friends control the table, not the answer to a direct query.
    let snapshot = scan_with_cpu_unfiltered(engine);
    let Some(service) = snapshot.by_port(port) else {
        eprintln!("No service listening on port {port}");
        return Ok(exit::NO_MATCH);
    };

    println!(
        ":{} {} {} {}",
        service.socket.port,
        cell(&service.project_display()),
        cell(&service.framework_display()),
        service
            .process
            .as_ref()
            .map(|p| format!("pid {}", p.pid))
            .unwrap_or_default(),
    );
    Ok(exit::OK)
}

pub fn print_table(engine: &mut Engine) -> Result<i32> {
    let snapshot = scan_with_cpu(engine);

    println!(
        "{:<6} {:<6} {:<10} {:<20} {:<18} {:<18} {:>7} {:>7} {:>7} {:>8}",
        "PORT", "PROTO", "SCOPE", "PROJECT", "FRAMEWORK", "COMMAND", "PID", "CPU", "MEM", "UPTIME"
    );

    for service in snapshot.sorted_by_port() {
        let process = service.process.as_ref();
        let scope_icon = match service.socket.bind_scope {
            portix_lib::model::BindScope::Loopback => "● ",
            portix_lib::model::BindScope::AllInterfaces => "○ ",
            portix_lib::model::BindScope::Specific(_) => "◆ ",
        };
        println!(
            "{:>5}  {:<6} {:<10} {:<20} {:<18} {:<18} {:>7} {:>7} {:>7} {:>8}",
            service.socket.port,
            service.socket.proto_display(),
            format!("{}{}", scope_icon, service.socket.scope_label()),
            cell(&truncate_center(&service.project_display(), 20)),
            {
                let fw_str = service
                    .framework
                    .as_ref()
                    .map(|fw| {
                        let icon = fw.tech_icon().unwrap_or("");
                        format!("{icon}{}", fw.display())
                    })
                    .unwrap_or_else(|| service.framework_display());
                cell(&truncate_center(&fw_str, 18))
            },
            cell(&truncate_center(&service.command_display(), 18)),
            process.map(|p| p.pid.to_string()).unwrap_or("—".into()),
            process
                .and_then(|p| p.metrics.cpu_percent)
                .map(|c| format!("{c:.1}%"))
                .unwrap_or("—".into()),
            process
                .and_then(|p| p.metrics.rss_bytes)
                .map(format_bytes_compact)
                .unwrap_or("—".into()),
            process
                .and_then(|p| p.metrics.uptime)
                .map(format_uptime)
                .unwrap_or("—".into()),
        );
    }

    for degradation in &snapshot.degradations {
        eprintln!("warning: {}", cell(&degradation.message()));
    }

    Ok(exit::OK)
}

pub fn kill_port(engine: &mut Engine, port: u16, force: bool, skip_confirm: bool) -> Result<i32> {
    // The prompt is only meaningful with a terminal on stdin, and the answer
    // must come from the real stdin. The inner function takes both explicitly
    // so the refusal and the "no" path can be tested without a TTY.
    let stdin = io::stdin();
    let stdin_is_terminal = stdin.is_terminal();
    kill_port_with_stdin(
        engine,
        port,
        force,
        skip_confirm,
        stdin_is_terminal,
        &mut stdin.lock(),
    )
}

/// [`kill_port`] with the stdin facts injected for testing.
///
/// Any outcome other than [`exit::OK`] means the port was **not** freed: a
/// script chaining `portix --kill 80 && pnpm dev` must not start a second
/// server on a port that is still taken.
fn kill_port_with_stdin(
    engine: &mut Engine,
    port: u16,
    force: bool,
    skip_confirm: bool,
    stdin_is_terminal: bool,
    input: &mut impl io::BufRead,
) -> Result<i32> {
    let signal = if force { Signal::Kill } else { Signal::Term };

    // Fresh scan up front: the confirmation text must describe the process
    // that owns the port *now*, not a seconds-old snapshot. (CPU columns are
    // irrelevant here, so a single scan suffices.) Unfiltered: `portix --kill
    // 80` is a direct question about a port, and a display filter must not
    // turn it into a silent "no service".
    let snapshot = engine.scan_unfiltered();
    let owners: Vec<&Service> = snapshot
        .services
        .iter()
        .filter(|s| s.socket.port == port)
        .collect();
    let Some(service) = owners.first().copied() else {
        eprintln!("No service listening on port {port}");
        return Ok(exit::NO_MATCH);
    };
    if owners.len() > 1 {
        // Several processes share this port (SO_REUSEPORT workers, containers
        // publishing the same port). A port query addresses the first owner;
        // say so instead of pretending the port is unambiguous.
        let pid = service
            .process
            .as_ref()
            .map(|p| p.pid.to_string())
            .unwrap_or_else(|| "unknown".into());
        eprintln!(
            "note: {} services share port {port}; targeting pid {pid}",
            owners.len()
        );
    }

    if skip_confirm {
        // No confirmation was shown, so no identity was pinned: the snapshot
        // just taken is the target. There is no human delay to guard against
        // (the re-scan below exists for that), and `signal_service` re-checks
        // the (pid, starttime) identity against live /proc right before
        // signalling — via a pidfd pin — which closes the millisecond window.
        engine.signal_service(service, signal)?;
        eprintln!("Sent {signal:?} to service on port {port}");
        return Ok(exit::OK);
    }

    // The identity pin of the process the confirmation text described.
    let pid = service
        .process
        .as_ref()
        .map(|p| p.pid.to_string())
        .unwrap_or_else(|| "unknown".into());

    // Confirmation is an interactive act. A non-terminal stdin (a pipe, a
    // fifo, /dev/null) must not block the prompt forever waiting for a line
    // that never comes — and silently reading a piped "y" would make the
    // confirmation meaningless anyway. Scripts get the same effect through
    // `--yes` / `kill_confirm = false`, which took the skip_confirm path
    // above.
    if !stdin_is_terminal {
        // Refusing is not success: the port is still taken, and exit 0 would
        // let a `--kill 80 && pnpm dev` chain sail past a busy port.
        eprintln!("stdin is not a terminal; pass --yes to skip confirmation");
        return Ok(exit::ERROR);
    }

    eprint!(
        "Kill {} on port {port} (pid {pid})? [y/N] ",
        cell(&service.project_display())
    );
    io::Write::flush(&mut io::stderr())?;

    let mut answer = String::new();
    input.read_line(&mut answer)?;
    if !matches!(answer.trim(), "y" | "Y" | "yes") {
        // "Cancelled." is a refusal to free the port, not a successful kill.
        eprintln!("Cancelled.");
        return Ok(exit::ERROR);
    }
    let expected = owner_pin(service);

    // The user may have taken seconds to answer; re-resolve from a fresh scan
    // so the signal targets the process currently on this port, not a ghost.
    let service = match engine.service_on_port(port) {
        Some(s) => s,
        None => {
            eprintln!("No service listening on port {port} anymore");
            return Ok(exit::NO_MATCH);
        }
    };
    // And if that fresh scan shows a *different* process than the one the
    // prompt described, refuse: the user confirmed that process, not this one.
    if let Some(why) = confirmed_owner_mismatch(&service, expected) {
        eprintln!("Refusing to kill: the owner of port {port} changed ({why})");
        return Ok(exit::ERROR);
    }
    engine.signal_service(&service, signal)?;
    eprintln!("Sent {signal:?} to service on port {port}");
    Ok(exit::OK)
}

/// Rebuild and reinstall portix from the local source tree.
///
/// Finds `scripts/install.sh` relative to the source directory that was used
/// at compile time (`CARGO_MANIFEST_DIR`). The script runs `cargo build
/// --release` and overwrites the installed binary, so re-running `--update`
/// after a `git pull` is the supported update path.
///
/// The source tree path is baked in at compile time. If the binary was
/// cross-compiled or moved away from the source, `--update` reports the path
/// it expected and exits with an error.
pub fn run_update(all_contexts: bool) -> Result<i32> {
    // `option_env!` evaluates at compile time. When building from the project
    // root (the normal `cargo build` / `scripts/install.sh` flow), this is
    // always set. A binary built elsewhere (e.g., a distro package) gets None.
    let manifest_dir = option_env!("CARGO_MANIFEST_DIR").ok_or_else(|| {
        anyhow::anyhow!(
            "source tree not found; --update only works when the binary was \
             built from the project directory"
        )
    })?;

    let script = std::path::Path::new(manifest_dir).join("scripts/install.sh");
    if !script.exists() {
        anyhow::bail!("install script not found: {}", script.display());
    }

    eprintln!("==> running {}", script.display());
    let mut command = std::process::Command::new("bash");
    command.arg(&script);
    // The installer stops every other Portix instance before replacing the
    // binary. Keep this updater alive until the script returns; otherwise the
    // cleanup would kill the process that is currently performing the update.
    command.env("PORTIX_UPDATE_KEEP_PID", std::process::id().to_string());
    // A normal terminal update updates the user installation, but first asks
    // the installer to stop root-owned stale TUIs through sudo. A direct sudo
    // invocation updates the privileged installation and then the invoking
    // user's context.
    if all_contexts || std::env::var_os("SUDO_USER").is_some() {
        command.arg("--all-contexts");
    }
    let status = command
        .status()
        .with_context(|| format!("failed to run {}", script.display()))?;

    Ok(if status.success() {
        exit::OK
    } else {
        exit::ERROR
    })
}

/// The `(pid, starttime)` identity pin of a service's process, if readable.
///
/// `starttime_jiffies` is the half of the identity that survives PID reuse: a
/// recycled PID carries a different start time. This pair is what the kill
/// confirmation pins, so a port re-bound between confirmation and signal can
/// be detected.
pub fn owner_pin(service: &Service) -> Option<(i32, u64)> {
    service
        .process
        .as_ref()
        .map(|p| (p.pid, p.starttime_jiffies))
}

/// Compare a freshly re-resolved service against the identity the user
/// confirmed in the kill modal.
///
/// `expected` is `Some((pid, starttime))` when the confirmation described a
/// specific process, `None` for an immediate (unconfirmed) kill. Returns the
/// reason the confirmed owner no longer matches, or `None` when signalling is
/// safe. The runner must abort on `Some(..)`: killing the new owner of a
/// re-bound port would kill a process the user never confirmed.
pub fn confirmed_owner_mismatch(service: &Service, expected: Option<(i32, u64)>) -> Option<String> {
    let expected = expected?; // no confirmation shown: kill whoever owns it now
    match owner_pin(service) {
        Some(actual) if actual == expected => None,
        Some((pid, _)) => Some(format!("pid {} -> pid {}", expected.0, pid)),
        None => Some("process metadata is unreadable".to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use portix_lib::core::Config;
    use portix_lib::model::{
        BindScope, Process, ProcessState, Protocol, ServiceId, Socket, TcpState,
    };

    fn service_with(pid: i32, starttime: u64) -> Service {
        let socket = Socket {
            protocol: Protocol::Tcp,
            local_addr: std::net::Ipv4Addr::LOCALHOST.into(),
            port: 3000,
            state: TcpState::Listen,
            bind_scope: BindScope::Loopback,
            inode: 1,
            uid: 1000,
            dual_stack: false,
        };
        Service {
            id: ServiceId::from_socket(&socket),
            socket,
            process: Some(Process {
                pid,
                ppid: 1,
                name: "node".into(),
                cmdline: vec!["node".into()],
                exe: None,
                cwd: None,
                uid: Some(1000),
                user: None,
                started_at: None,
                starttime_jiffies: starttime,
                cpu_jiffies: 0,
                metrics: Default::default(),
                state: ProcessState::Sleeping,
                threads: 1,
            }),
            project: None,
            framework: None,
            container: None,
            docker_proxy_rewritten: false,
        }
    }

    /// An immediate (unconfirmed) kill carries no pin: whoever owns the port
    /// now is the target.
    #[test]
    fn test_unconfirmed_kill_has_no_pin() {
        let svc = service_with(10, 5);
        assert_eq!(confirmed_owner_mismatch(&svc, None), None);
    }

    /// The exact process the user confirmed is still the owner: signal it.
    #[test]
    fn test_matching_owner_is_confirmable() {
        let svc = service_with(10, 5);
        assert_eq!(confirmed_owner_mismatch(&svc, Some((10, 5))), None);
    }

    /// Same pid, different starttime: the pid was recycled into a different
    /// process since the confirmation. Refuse.
    #[test]
    fn test_recycled_pid_is_not_the_confirmed_owner() {
        let svc = service_with(10, 999);
        assert!(confirmed_owner_mismatch(&svc, Some((10, 5))).is_some());
    }

    /// The port was re-bound by a completely different process: refuse, and
    /// say which pid took over.
    #[test]
    fn test_rebound_port_is_not_the_confirmed_owner() {
        let svc = service_with(20, 5);
        let why = confirmed_owner_mismatch(&svc, Some((10, 5))).unwrap();
        assert!(
            why.contains("pid 10"),
            "reason mentions the confirmed pid: {why}"
        );
    }

    /// The re-resolved service has no readable process metadata: cannot prove
    /// it is the confirmed owner, so refuse rather than kill blind.
    #[test]
    fn test_unreadable_process_cannot_be_confirmed() {
        let mut svc = service_with(10, 5);
        svc.process = None;
        assert!(confirmed_owner_mismatch(&svc, Some((10, 5))).is_some());
    }

    /// `--kill` on a port with no listener exits NO_MATCH without prompting.
    #[test]
    fn test_kill_port_no_service_returns_no_match() {
        let dir = tempfile::tempdir().unwrap();
        let engine = &mut Engine::new(Some(dir.path()), Config::default());
        let code = kill_port(engine, 39999, false, true).unwrap();
        assert_eq!(code, exit::NO_MATCH);
    }

    /// A synthetic /proc root with one TCP LISTEN socket on `port`, owned by
    /// `pid`, whose fd table links the socket inode.
    fn synthetic_listener_proc(port: u16, pid: i32) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::write(root.join("stat"), "cpu  1 2 3 4\nbtime 1700000000\n").unwrap();
        std::fs::create_dir_all(root.join("net")).unwrap();
        std::fs::write(
            root.join("net/tcp"),
            format!(
                "  sl  local_address rem_address   st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode\n   0: 0100007F:{port:04X} 00000000:0000 0A 00000000:00000000 00:00000000 00000000  1000        0 777001 1 0000 100 0 0 10 0\n"
            ),
        )
        .unwrap();
        let pdir = root.join(pid.to_string());
        std::fs::create_dir_all(pdir.join("fd")).unwrap();
        std::fs::write(
            pdir.join("stat"),
            format!("{pid} (svc) S 1 1 1 0 -1 0 0 0 0 0 10 20 0 0 20 0 1 0 12345 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0"),
        )
        .unwrap();
        std::fs::write(
            pdir.join("status"),
            "Name:\tsvc\nUid:\t1000\t1000\t1000\t1000\nVmRSS:\t1024 kB\nThreads:\t1\n",
        )
        .unwrap();
        std::fs::write(pdir.join("cmdline"), b"node\x00server.js\x00").unwrap();
        std::os::unix::fs::symlink("socket:[777001]", pdir.join("fd/3")).unwrap();
        dir
    }

    /// `--kill` on a fixture service runs the full path up to the signal,
    /// which fails deterministically on a synthetic pid: a pid beyond pid_max
    /// can never be pinned by pidfd_open nor hit by kill(2) (ESRCH on modern
    /// kernels, "No such process"), so the test can never signal a real
    /// process.
    #[test]
    fn test_kill_port_reaches_signal_and_fails_on_synthetic_pid() {
        let dir = synthetic_listener_proc(8080, 9_000_000);
        let engine = &mut Engine::new(Some(dir.path()), Config::default());
        let err = kill_port(engine, 8080, false, true).unwrap_err();
        assert!(
            err.to_string().contains("process"),
            "the signal step must fail on the synthetic pid: {err}"
        );
    }

    /// `--kill 80` must see system ports: a port query answers a direct
    /// question, and the table filter for system ports must not turn it into a
    /// silent "no service" (which would make `portix :80 || pnpm dev` start a
    /// server on a taken port).
    #[test]
    fn test_kill_port_sees_system_ports_despite_display_filter() {
        let dir = synthetic_listener_proc(80, 9_000_000);
        let engine = &mut Engine::new(Some(dir.path()), Config::default());
        // The port is found (filters off) and the signal step fails on the
        // synthetic pid — never an Ok(NO_MATCH) "no service" answer.
        let err = kill_port(engine, 80, false, true).unwrap_err();
        assert!(
            err.to_string().contains("process"),
            "port 80 must be found and the signal step must fail safely: {err}"
        );
    }

    /// A non-terminal stdin cannot answer the prompt, so `--kill` refuses.
    /// That refusal must be an error: exit 0 would claim the port was freed
    /// and let `portix --kill 80 && pnpm dev` proceed onto a busy port. The
    /// synthetic pid would make any signal attempt fail loudly, so an
    /// `Ok(ERROR)` proves no signal was sent.
    #[test]
    fn test_kill_port_non_tty_refuses_with_error() {
        let dir = synthetic_listener_proc(8080, 9_000_000);
        let engine = &mut Engine::new(Some(dir.path()), Config::default());
        let mut input = io::Cursor::new(Vec::new());
        let code = kill_port_with_stdin(engine, 8080, false, false, false, &mut input).unwrap();
        assert_eq!(code, exit::ERROR);
    }

    /// Answering "no" leaves the port taken: that is a failure, not a
    /// successful kill.
    #[test]
    fn test_kill_port_declined_prompt_is_error() {
        let dir = synthetic_listener_proc(8080, 9_000_000);
        let engine = &mut Engine::new(Some(dir.path()), Config::default());
        let mut input = io::Cursor::new(b"n\n".to_vec());
        let code = kill_port_with_stdin(engine, 8080, false, false, true, &mut input).unwrap();
        assert_eq!(code, exit::ERROR);
    }
}
