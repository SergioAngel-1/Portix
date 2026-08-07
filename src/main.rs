use anyhow::{Context, Result};
use clap::Parser;
use crossterm::{
    event::{self, Event, KeyCode, KeyEventKind, KeyModifiers},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout},
    Terminal,
};
use serde::Serialize;
use signal_hook::consts::{SIGHUP, SIGINT, SIGTERM};
use signal_hook::flag;
use std::io::{self, IsTerminal};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use portix_lib::core::{Config, Engine};
use portix_lib::model::{Service, SharedSnapshot, Signal};
use portix_lib::ui::app::{App, Effect, Mode};
use portix_lib::ui::format::{cell, format_bytes_compact, format_uptime, truncate_center};
use portix_lib::ui::Theme;

/// The JSON contract. Consumers switch on `schema_version`; unknown fields must
/// be ignored. Adding a field is a minor bump, removing or retyping one is major.
const SCHEMA_VERSION: u32 = 1;

/// Exit codes, as documented in PRD §12.2. Scripts rely on these:
/// `portix :3000 || pnpm dev`.
mod exit {
    pub const OK: i32 = 0;
    pub const ERROR: i32 = 1;
    pub const NO_MATCH: i32 = 3;
}

#[derive(Parser, Debug)]
#[command(name = "portix")]
#[command(about = "The local development service explorer")]
#[command(version)]
struct Cli {
    /// Show service for a specific port (accepts 3000 or :3000)
    #[arg(value_name = "PORT", value_parser = parse_port, conflicts_with = "kill")]
    port: Option<u16>,

    /// Show service for a specific port (long form, PRD §12.2)
    #[arg(long = "port", value_name = "PORT", value_parser = parse_port, conflicts_with = "port")]
    port_flag: Option<u16>,

    /// Output JSON
    #[arg(long, conflicts_with_all = ["kill", "once"])]
    json: bool,

    /// Single frame, no TUI
    #[arg(long, conflicts_with_all = ["kill", "json"])]
    once: bool,

    /// Kill service on port
    #[arg(
        long,
        value_name = "PORT",
        value_parser = parse_port,
        conflicts_with_all = ["port", "port_flag", "json", "once"]
    )]
    kill: Option<u16>,

    /// Force kill with SIGKILL instead of SIGTERM
    #[arg(long)]
    force: bool,

    /// Skip confirmation for kill
    #[arg(long)]
    yes: bool,

    /// Alternative /proc root (for testing/containers)
    #[arg(long, value_name = "PATH")]
    proc_root: Option<String>,

    /// Refresh interval in ms
    #[arg(long, value_name = "MS")]
    interval: Option<u64>,

    /// Include ports below 1024
    #[arg(long)]
    system_ports: bool,

    /// Disable colors
    #[arg(long)]
    no_color: bool,

    /// Output completions for a shell
    #[arg(long, value_name = "SHELL")]
    completions: Option<clap_complete::Shell>,

    /// Rebuild and reinstall portix from local source tree.
    #[arg(long, conflicts_with_all = ["port", "port_flag", "json", "once", "kill"])]
    update: bool,

    /// Update both the user installation and the privileged installation.
    /// When run through sudo, the privileged updater also refreshes the
    /// invoking user's installation and both runtime cache namespaces.
    #[arg(long)]
    all_contexts: bool,
}

/// Accept both `3000` and the `:3000` form users type from muscle memory.
fn parse_port(raw: &str) -> Result<u16, String> {
    raw.strip_prefix(':')
        .unwrap_or(raw)
        .parse()
        .map_err(|_| format!("`{raw}` is not a port number"))
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

fn main() {
    match run() {
        Ok(code) => std::process::exit(code),
        Err(e) => {
            // The error can embed system-derived paths; sanitize before it
            // reaches the terminal (PRD §13.2).
            eprintln!("portix: {}", cell(&format!("{e:#}")));
            std::process::exit(exit::ERROR);
        }
    }
}

fn run() -> Result<i32> {
    let cli = Cli::parse();

    if let Some(shell) = cli.completions {
        let mut cmd = <Cli as clap::CommandFactory>::command();
        clap_complete::generate(shell, &mut cmd, "portix", &mut io::stdout());
        return Ok(exit::OK);
    }

    if cli.all_contexts && !cli.update {
        anyhow::bail!("--all-contexts requires --update");
    }

    if cli.update {
        return run_update(cli.all_contexts);
    }

    let mut config = Config::load();
    if let Some(ms) = cli.interval {
        config.refresh_ms = ms;
    }
    if cli.system_ports {
        config.show_system_ports = true;
    }

    let proc_root = cli.proc_root.as_deref().map(std::path::Path::new);
    let mut engine = Engine::new(proc_root, config.clone());

    // `portix 3000`, `portix :3000`, and `portix --port 3000` are the same
    // query (PRD §12.2); clap forbids passing both.
    let port = cli.port.or(cli.port_flag);

    if let Some(port) = cli.kill {
        // `kill_confirm = false` in config.toml means "never ask" — the same
        // setting the TUI honours. `--yes` remains the explicit per-invocation
        // override; either one skips the prompt.
        let skip_confirm = cli.yes || !config.kill_confirm;
        return kill_port(&mut engine, port, cli.force, skip_confirm);
    }
    if cli.json {
        return print_json(&mut engine, port);
    }
    if let Some(port) = port {
        return print_port(&mut engine, port);
    }
    if cli.once {
        return print_table(&mut engine);
    }

    let color = use_color(&cli);
    let theme = Theme::select(&config.theme, color);
    run_tui(config, engine, theme, color)?;
    Ok(exit::OK)
}

fn use_color(cli: &Cli) -> bool {
    // no-color.org: NO_COLOR takes effect only when present *and non-empty*;
    // `NO_COLOR=` (common under `env -i`) must not silently disable color.
    let no_color = std::env::var_os("NO_COLOR").is_some_and(|v| !v.is_empty());
    !cli.no_color && !no_color
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

fn print_json(engine: &mut Engine, port: Option<u16>) -> Result<i32> {
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

fn print_port(engine: &mut Engine, port: u16) -> Result<i32> {
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

fn print_table(engine: &mut Engine) -> Result<i32> {
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

fn kill_port(engine: &mut Engine, port: u16, force: bool, skip_confirm: bool) -> Result<i32> {
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
    if !io::stdin().is_terminal() {
        eprintln!("stdin is not a terminal; pass --yes to skip confirmation");
        return Ok(exit::OK);
    }

    eprint!(
        "Kill {} on port {port} (pid {pid})? [y/N] ",
        cell(&service.project_display())
    );
    io::Write::flush(&mut io::stderr())?;

    let mut input = String::new();
    io::stdin().read_line(&mut input)?;
    if !matches!(input.trim(), "y" | "Y" | "yes") {
        eprintln!("Cancelled.");
        return Ok(exit::OK);
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

/// Restores raw mode on drop, so an early `?` between `enable_raw_mode` and
/// the explicit restore cannot strand the user's terminal (no echo, no
/// newline). Double-restoring is harmless: the explicit `disable_raw_mode` at
/// the end of `run_tui` runs first and the guard's second call is a no-op.
struct RawModeGuard;

impl Drop for RawModeGuard {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
    }
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
fn run_update(all_contexts: bool) -> Result<i32> {
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
    // A sudo invocation is the privileged context the user explicitly asked
    // to update. In that case update both installations and clear both cache
    // namespaces; a normal user invocation updates only the user install unless
    // --all-contexts is supplied.
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

fn run_tui(config: Config, engine: Engine, theme: Theme, color: bool) -> Result<()> {
    if !io::stdout().is_terminal() {
        anyhow::bail!("stdout is not a terminal; use --once or --json");
    }

    // A termination signal must exit through the normal restore path below,
    // not kill the process mid-raw-mode: SIGTERM/SIGHUP/SIGINT (from outside
    // the terminal — Ctrl+C inside raw mode arrives as a key event instead)
    // set this flag, and the event loop checks it and returns cleanly. The
    // handlers are registered before raw mode, so a signal during startup is
    // caught too.
    let interrupted = Arc::new(AtomicBool::new(false));
    for sig in [SIGTERM, SIGHUP, SIGINT] {
        flag::register(sig, Arc::clone(&interrupted))?;
    }

    enable_raw_mode()?;
    // From here on, every exit path must restore the terminal — leaving a
    // user in raw mode on the alternate screen is worse than the error.
    let _raw = RawModeGuard;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    // The engine is shared: the collector thread owns the periodic scans (so
    // a slow scan never freezes the UI), while the UI thread locks it for the
    // rare synchronous re-scans the kill path needs — the modal must be armed
    // before the next draw. The mutex is held only for the duration of a
    // scan, so the two never deadlock (neither waits on the other while
    // holding the lock).
    let engine = Arc::new(Mutex::new(engine));
    let refresh = Duration::from_millis(config.min_refresh_ms());
    let (tx, rx) = mpsc::channel();
    let shutdown = Arc::new(AtomicBool::new(false));
    let collector = {
        let engine = Arc::clone(&engine);
        let shutdown = Arc::clone(&shutdown);
        thread::spawn(move || collector_loop(engine, tx, refresh, shutdown))
    };

    let result = event_loop(
        &mut terminal,
        config,
        &engine,
        rx,
        theme,
        color,
        interrupted,
    );

    // Restore the terminal even if the loop failed. Leaving a user in raw mode
    // on the alternate screen is worse than whatever error got us here.
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;

    // Stop the collector (it sleeps in short slices, so this returns quickly)
    // and wait for it to release the engine before `result` is returned.
    shutdown.store(true, Ordering::Relaxed);
    let _ = collector.join();

    result
}

/// The periodic scan loop, off the UI thread.
///
/// Scans run back-to-back with a `refresh` pause between them; each snapshot
/// is handed to the UI over the channel. The engine is locked only for the
/// duration of each scan, so the UI thread can still lock it for the
/// synchronous kill-path re-scans (`Effect::KillRequest` / `Effect::Kill`).
/// The pause is slept in short slices so `shutdown` is honoured promptly.
fn collector_loop(
    engine: Arc<Mutex<Engine>>,
    tx: mpsc::Sender<SharedSnapshot>,
    refresh: Duration,
    shutdown: Arc<AtomicBool>,
) {
    loop {
        if shutdown.load(Ordering::Relaxed) {
            break;
        }
        let snapshot = engine.lock().unwrap_or_else(|p| p.into_inner()).scan();
        if tx.send(snapshot).is_err() {
            break; // The UI is gone; nothing left to feed.
        }
        let deadline = Instant::now() + refresh;
        while Instant::now() < deadline {
            if shutdown.load(Ordering::Relaxed) {
                return;
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            thread::sleep(remaining.min(Duration::from_millis(50)));
        }
    }
}

fn event_loop<B: ratatui::backend::Backend>(
    terminal: &mut Terminal<B>,
    config: Config,
    engine: &Arc<Mutex<Engine>>,
    rx: mpsc::Receiver<SharedSnapshot>,
    theme: Theme,
    color: bool,
    interrupted: Arc<AtomicBool>,
) -> Result<()> {
    let mut app = App::new(config.clone(), theme, color);

    let refresh = Duration::from_millis(config.min_refresh_ms());
    // First frame draws immediately with the empty snapshot; the collector's
    // first scan fills the table within one scan duration. Input is never
    // blocked by the scan, which runs on its own thread.
    let mut dirty = true;

    loop {
        // A termination signal (SIGTERM/SIGHUP/kill -INT) must exit through
        // this path so `run_tui` restores the terminal. The signal handler
        // sets the flag; `poll` returns on the EINTR it provokes, so the
        // check below wakes promptly instead of sleeping out the timeout.
        if interrupted.load(Ordering::Relaxed) {
            return Ok(());
        }

        // Drain whatever snapshots the collector produced since the last
        // iteration. Drawing only when something actually changed keeps CPU
        // at rest between events indistinguishable from zero.
        let mut got_snapshot = false;
        while let Ok(snapshot) = rx.try_recv() {
            app.update_snapshot(snapshot);
            got_snapshot = true;
        }
        if got_snapshot || dirty {
            terminal.draw(|f| draw(f, &app))?;
            dirty = false;
        }

        // Wake up for input, but never sleep past the next refresh. The poll
        // timeout is deliberately shorter than the refresh: the collector's
        // cadence (scan + pause) drifts against this loop's own cadence, and
        // a full `refresh` poll would let snapshots sit in the channel for up
        // to one scan duration. A 100 ms cap bounds pickup latency while
        // costing only a handful of near-zero-cost wakeups per second (no
        // redraw happens unless something changed).
        let timeout = refresh.min(Duration::from_millis(100));
        if event::poll(timeout)? {
            if let Event::Key(key) = event::read()? {
                // Key events fire on press *and* release on some terminals.
                if key.kind != KeyEventKind::Press {
                    continue;
                }
                if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
                    return Ok(());
                }

                for effect in app.handle_key(key) {
                    // `Quit` must leave the loop, not just this `for`.
                    if run_effect(effect, &mut app, engine) == ControlFlow::Quit {
                        return Ok(());
                    }
                }
                dirty = true;
            }
        }

        // A signal that arrived while blocked in poll (or handled as a key
        // event above) still ends the loop cleanly.
        if interrupted.load(Ordering::Relaxed) {
            return Ok(());
        }
    }
}

#[derive(PartialEq, Eq)]
enum ControlFlow {
    Continue,
    Quit,
}

/// The `(pid, starttime)` identity pin of a service's process, if readable.
///
/// `starttime_jiffies` is the half of the identity that survives PID reuse: a
/// recycled PID carries a different start time. This pair is what the kill
/// confirmation pins, so a port re-bound between confirmation and signal can
/// be detected.
fn owner_pin(service: &Service) -> Option<(i32, u64)> {
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
fn confirmed_owner_mismatch(service: &Service, expected: Option<(i32, u64)>) -> Option<String> {
    let expected = expected?; // no confirmation shown: kill whoever owns it now
    match owner_pin(service) {
        Some(actual) if actual == expected => None,
        Some((pid, _)) => Some(format!("pid {} -> pid {}", expected.0, pid)),
        None => Some("process metadata is unreadable".to_string()),
    }
}

fn run_effect(effect: Effect, app: &mut App, engine: &Arc<Mutex<Engine>>) -> ControlFlow {
    match effect {
        Effect::Quit => return ControlFlow::Quit,

        Effect::OpenBrowser(url) => {
            if let Err(e) = portix_lib::core::actions::open_url(&url) {
                app.error = Some(e.to_string());
            }
        }

        Effect::CopyToClipboard(text) => {
            if let Err(e) = portix_lib::core::actions::copy_to_clipboard(&text) {
                app.error = Some(e.to_string());
            }
        }

        Effect::KillRequest(inode, signal) => {
            // Re-resolve from a fresh scan before showing the confirmation:
            // the snapshot the UI holds can be seconds old, and the modal must
            // describe the process that owns the socket *now*. The inode pins
            // the exact row the user selected — another process on the same
            // port (SO_REUSEPORT, containers) is not that row.
            //
            // This locks the engine away from the collector for the duration
            // of one scan. The modal must be armed before the next draw, so
            // the wait is unavoidable and rare (one user keystroke).
            let mut engine = engine.lock().unwrap_or_else(|p| p.into_inner());
            match engine.service_on_inode(inode) {
                Some(service) => app.arm_kill(service, signal),
                None => app.error = Some("the selected service is no longer listening".into()),
            }
        }

        Effect::Kill(inode, expected, signal) => {
            // The snapshot the UI holds can be seconds old: the selected
            // socket may have been closed (and its inode freed) in that
            // window. Re-resolve from a fresh scan so we signal the process
            // that owns the socket now, not a ghost.
            let mut engine = engine.lock().unwrap_or_else(|p| p.into_inner());
            match engine.service_on_inode(inode) {
                Some(service) => {
                    // If the user confirmed a specific process in the modal,
                    // only that process may be signalled. The owner of a
                    // socket can change while the modal is open (fd handoff,
                    // fork), and signalling a different process would kill
                    // something the user never confirmed.
                    if let Some(why) = confirmed_owner_mismatch(&service, expected) {
                        let port = service.socket.port;
                        app.error = Some(format!(
                            "not killing: the owner of port {port} changed ({why})"
                        ));
                    } else if let Err(e) = engine.signal_service(&service, signal) {
                        app.error = Some(e.to_string());
                    }
                }
                None => app.error = Some("service disappeared".into()),
            }
        }

        Effect::Refresh => {
            // A manual refresh is user-initiated, so an inline scan is fine —
            // it locks the engine away from the collector for one scan.
            let snapshot = engine.lock().unwrap_or_else(|p| p.into_inner()).scan();
            app.update_snapshot(snapshot);
        }

        Effect::PersistConfig => {
            if let Err(e) = app.config.save_theme() {
                app.error = Some(format!("could not save theme: {e}"));
            }
        }
    }
    ControlFlow::Continue
}

fn draw(f: &mut ratatui::Frame, app: &App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(3), Constraint::Length(1)])
        .split(f.area());

    let services: Vec<&Service> = app
        .filtered_indices
        .iter()
        .filter_map(|&idx| app.snapshot.services.get(idx))
        .collect();

    portix_lib::ui::views::main_table::render(f, chunks[0], &services, app.selected, &app.theme);
    portix_lib::ui::views::status_bar::render(
        f,
        chunks[1],
        &app.snapshot,
        !app.query.query().is_empty(),
        app.error.as_deref(),
        &app.theme,
    );

    if app.mode == Mode::Search {
        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Min(3),
                Constraint::Length(3),
                Constraint::Length(1),
            ])
            .split(f.area());
        portix_lib::ui::views::search_bar::render(
            f,
            rows[1],
            &app.search_buffer,
            app.snapshot.services.len(),
            app.filtered_indices.len(),
            &app.theme,
        );
    }

    if app.detail_open {
        portix_lib::ui::views::detail_panel::render(
            f,
            f.area(),
            app.selected_service(),
            &app.theme,
        );
    }

    if let Some(signal) = app.pending_kill() {
        portix_lib::ui::views::confirm_modal::render(
            f,
            f.area(),
            // The freshly re-resolved service (Effect::KillRequest), never the
            // possibly-stale selection.
            app.pending_kill_service(),
            signal,
            &app.theme,
        );
    }

    if app.mode == Mode::ThemePicker {
        portix_lib::ui::views::theme_picker::render(f, f.area(), app.theme_cursor, &app.theme);
    }

    if app.show_help() {
        portix_lib::ui::views::help::render(f, f.area(), &app.snapshot.degradations, &app.theme);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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
}
