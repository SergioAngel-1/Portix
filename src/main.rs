mod cli;

use anyhow::Result;
use clap::Parser;
use crossterm::{
    cursor,
    event::{
        self, DisableFocusChange, EnableFocusChange, Event, KeyCode, KeyEventKind, KeyModifiers,
    },
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{backend::CrosstermBackend, Terminal};
use signal_hook::consts::{SIGHUP, SIGINT, SIGTERM};
use signal_hook::flag;
use std::io::{self, IsTerminal};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use portix_lib::core::{Config, Engine};
use portix_lib::model::SharedSnapshot;
use portix_lib::ui::app::{App, Effect};
use portix_lib::ui::format::cell;
use portix_lib::ui::Theme;

use crate::cli::{
    confirmed_owner_mismatch, exit, kill_port, print_json, print_port, print_table, run_update,
};

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

fn main() {
    match run() {
        Ok(code) => std::process::exit(code),
        // `portix --once | head` closes the pipe early. That is the reader
        // being done, not a failure: `println!` used to panic on the EPIPE,
        // and reporting it as an error would be just as wrong. Exit quietly.
        Err(e) if is_broken_pipe(&e) => std::process::exit(exit::OK),
        Err(e) => {
            // The error can embed system-derived paths; sanitize before it
            // reaches the terminal (PRD §13.2).
            eprintln!("portix: {}", cell(&format!("{e:#}")));
            std::process::exit(exit::ERROR);
        }
    }
}

/// Whether `error` is a write to a stdout whose reader has gone away.
fn is_broken_pipe(error: &anyhow::Error) -> bool {
    error
        .chain()
        .filter_map(|cause| cause.downcast_ref::<io::Error>())
        .any(|io| io.kind() == io::ErrorKind::BrokenPipe)
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

/// Install a panic hook that gives the terminal back before the panic message
/// is printed.
///
/// A panic unwinds through [`RawModeGuard`], which turns raw mode off, but
/// nothing leaves the alternate screen, disables focus reporting or shows the
/// cursor again. The panic message then scrolls by on a screen the shell is
/// about to restore, and the cursor stays hidden. Restoring here — best-effort,
/// a broken terminal must never hide the original panic — and then chaining to
/// the previous hook keeps both the diagnosis and the user's shell usable.
fn install_panic_hook() {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = disable_raw_mode();
        let _ = execute!(
            io::stdout(),
            DisableFocusChange,
            LeaveAlternateScreen,
            cursor::Show
        );
        previous(info);
    }));
}

/// Refresh cadence while the terminal is unfocused.
///
/// The table is not being looked at, so a five-second cadence keeps it warm
/// while letting the machine idle. The collector sleeps in
/// `recv_timeout` until the next scan is due, so a focus event wakes it at
/// once: returning to the terminal does not wait out the idle interval.
const UNFOCUSED_REFRESH: Duration = Duration::from_secs(5);

/// What the UI thread tells the collector to do.
///
/// The collector used to poll two `AtomicBool`s in 50 ms slices, which cost
/// 20 wakeups per second even on an idle, unfocused terminal. A channel makes
/// every control decision an event: the collector blocks until its next scan
/// is due or one of these arrives.
enum Control {
    /// The UI is going away: wake up and return.
    Shutdown,
    /// The terminal gained or lost focus; re-evaluate the cadence (regaining
    /// focus scans immediately when the last snapshot is already stale).
    Focus(bool),
    /// `r` was pressed: run `scan_fresh()` and send the snapshot. Forcing a
    /// full inode revalidation on the UI thread would freeze input for the
    /// duration of the walk.
    RefreshNow,
}

/// How long the collector may wait before its next scan.
///
/// A focused terminal scans every `refresh`; a backgrounded one is not worth
/// more than one scan per [`UNFOCUSED_REFRESH`]. Saturating at zero makes an
/// overdue scan run immediately, which is also how regaining focus refreshes a
/// stale table without waiting out the idle interval.
fn scan_delay(last_scan: Instant, now: Instant, focused: bool, refresh: Duration) -> Duration {
    let interval = if focused {
        refresh
    } else {
        refresh.max(UNFOCUSED_REFRESH)
    };
    interval.saturating_sub(now.saturating_duration_since(last_scan))
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

    // A panic unwinds through `RawModeGuard` (raw mode off), but the alternate
    // screen, focus reporting and the hidden cursor would survive it: the
    // panic message lands on a screen that is about to disappear and the shell
    // is left broken. Restore first, then chain to the previous hook so the
    // message still reaches the user.
    install_panic_hook();

    enable_raw_mode()?;
    // From here on, every exit path must restore the terminal — leaving a
    // user in raw mode on the alternate screen is worse than the error.
    let _raw = RawModeGuard;
    let mut stdout = io::stdout();
    // Focus events let the collector slow down when the terminal is in the
    // background. Not every terminal supports the sequence; one that does not
    // simply never sends the events, and the app stays on the focused cadence.
    execute!(stdout, EnterAlternateScreen, EnableFocusChange)?;
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
    let (ctrl_tx, ctrl_rx) = mpsc::channel();
    let collector = {
        let engine = Arc::clone(&engine);
        thread::spawn(move || collector_loop(engine, tx, refresh, ctrl_rx))
    };

    let result = event_loop(
        &mut terminal,
        config,
        &engine,
        rx,
        &ctrl_tx,
        theme,
        color,
        interrupted,
    );

    // Restore the terminal even if the loop failed. Leaving a user in raw mode
    // on the alternate screen is worse than whatever error got us here. Stop
    // focus reporting first so the shell does not receive focus sequences.
    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        DisableFocusChange,
        LeaveAlternateScreen
    )?;
    terminal.show_cursor()?;

    // Stop the collector: it is blocked in `recv_timeout`, so this wakes it at
    // once instead of waiting out the refresh interval, then wait for it to
    // release the engine before `result` is returned.
    let _ = ctrl_tx.send(Control::Shutdown);
    let _ = collector.join();

    result
}

/// The periodic scan loop, off the UI thread.
///
/// Scans run back-to-back with a pause between them; each snapshot is handed
/// to the UI over the snapshot channel. The engine is locked only for the
/// duration of each scan, so the UI thread can still lock it for the
/// synchronous kill-path re-scans (`Effect::KillRequest` / `Effect::Kill`).
///
/// The pause adapts to terminal focus: `refresh` while focused, at least
/// [`UNFOCUSED_REFRESH`] while backgrounded. The collector blocks in
/// `recv_timeout` for exactly [`scan_delay`], so it wakes only when the next
/// scan is due or the UI sends a [`Control`] — no idle polling, and a focus
/// change is honoured at once. A closed control channel means the UI is gone.
fn collector_loop(
    engine: Arc<Mutex<Engine>>,
    tx: mpsc::Sender<SharedSnapshot>,
    refresh: Duration,
    ctrl_rx: mpsc::Receiver<Control>,
) {
    let mut focused = true;
    let mut last_scan: Option<Instant> = None;

    loop {
        // The first iteration has no snapshot yet and scans immediately.
        let wait = last_scan.map_or(Duration::ZERO, |at| {
            scan_delay(at, Instant::now(), focused, refresh)
        });
        if wait.is_zero() {
            let snapshot = engine.lock().unwrap_or_else(|p| p.into_inner()).scan();
            if tx.send(snapshot).is_err() {
                return; // The UI is gone; nothing left to feed.
            }
            last_scan = Some(Instant::now());
            continue;
        }

        match ctrl_rx.recv_timeout(wait) {
            Ok(Control::Shutdown) | Err(mpsc::RecvTimeoutError::Disconnected) => return,
            Ok(Control::Focus(current)) => focused = current,
            Ok(Control::RefreshNow) => {
                // `r` is the recovery action for a stale ownership cache: the
                // forced refresh must bypass the inode cache, not just the
                // pause.
                let snapshot = engine
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .scan_fresh();
                if tx.send(snapshot).is_err() {
                    return;
                }
                last_scan = Some(Instant::now());
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {} // a scan is due
        }
    }
}

/// Move every snapshot waiting on the collector channel into `app`.
///
/// Returns whether any snapshot was applied. A disconnected channel means the
/// collector thread is gone: whatever it sent has already been drained above,
/// and continuing would leave the table frozen on a snapshot nobody will ever
/// update, so the caller must surface "collector thread stopped" instead.
fn drain_snapshots(rx: &mpsc::Receiver<SharedSnapshot>, app: &mut App) -> Result<bool> {
    let mut got_snapshot = false;
    loop {
        match rx.try_recv() {
            Ok(snapshot) => {
                app.update_snapshot(snapshot);
                got_snapshot = true;
            }
            Err(mpsc::TryRecvError::Empty) => return Ok(got_snapshot),
            Err(mpsc::TryRecvError::Disconnected) => anyhow::bail!("collector thread stopped"),
        }
    }
}

// The loop owns the terminal, the shared engine, the snapshot and control
// channels and the interruption flag; splitting them into a context struct
// would only move the same state one level down.
#[allow(clippy::too_many_arguments)]
fn event_loop<B: ratatui::backend::Backend>(
    terminal: &mut Terminal<B>,
    config: Config,
    engine: &Arc<Mutex<Engine>>,
    rx: mpsc::Receiver<SharedSnapshot>,
    ctrl_tx: &mpsc::Sender<Control>,
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
    // Local mirror of the terminal focus, for this loop's own poll timeout.
    // The collector keeps its own copy, updated through `Control::Focus`.
    let mut focused = true;

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
        let got_snapshot = drain_snapshots(&rx, &mut app)?;
        if got_snapshot || dirty {
            terminal.draw(|f| portix_lib::ui::views::render_app(f, &app))?;
            dirty = false;
        }

        // Wake up for input, but never sleep past the next refresh. The poll
        // timeout is deliberately shorter than the refresh: the collector's
        // cadence (scan + pause) drifts against this loop's own cadence, and
        // a full `refresh` poll would let snapshots sit in the channel for up
        // to one scan duration. A 100 ms cap bounds pickup latency while
        // costing only a handful of near-zero-cost wakeups per second (no
        // redraw happens unless something changed). A backgrounded terminal
        // polls less often; focus events still wake it.
        let timeout = if focused {
            refresh.min(Duration::from_millis(100))
        } else {
            Duration::from_millis(250)
        };
        if event::poll(timeout)? {
            match event::read()? {
                Event::Key(key) => {
                    // Key events fire on press *and* release on some terminals.
                    if key.kind != KeyEventKind::Press {
                        continue;
                    }
                    if key.code == KeyCode::Char('c')
                        && key.modifiers.contains(KeyModifiers::CONTROL)
                    {
                        return Ok(());
                    }

                    for effect in app.handle_key(key) {
                        // `Quit` must leave the loop, not just this `for`.
                        if run_effect(effect, &mut app, engine, ctrl_tx) == ControlFlow::Quit {
                            return Ok(());
                        }
                    }
                    dirty = true;
                }
                // The terminal gained focus: tell the collector, which
                // re-evaluates its cadence and scans immediately if the last
                // snapshot is older than the focused interval.
                Event::FocusGained => {
                    focused = true;
                    let _ = ctrl_tx.send(Control::Focus(true));
                }
                Event::FocusLost => {
                    focused = false;
                    let _ = ctrl_tx.send(Control::Focus(false));
                }
                // A resize needs a redraw even if no snapshot arrived — while
                // backgrounded, waiting for the next scan could leave a stale
                // frame for seconds.
                Event::Resize(_, _) => dirty = true,
                _ => {}
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

fn run_effect(
    effect: Effect,
    app: &mut App,
    engine: &Arc<Mutex<Engine>>,
    ctrl_tx: &mpsc::Sender<Control>,
) -> ControlFlow {
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
            // `r` is user-initiated, but a forced full inode walk on the UI
            // thread would freeze input for its whole duration. Hand it to the
            // collector, which runs `scan_fresh()` and sends the snapshot back
            // through the normal channel. A failed send means the collector is
            // gone; the event loop reports that when the channel disconnects.
            let _ = ctrl_tx.send(Control::RefreshNow);
        }

        Effect::PersistConfig => {
            if let Err(e) = app.config.save_theme() {
                app.error = Some(format!("could not save theme: {e}"));
            }
        }
    }
    ControlFlow::Continue
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A closed downstream pipe ends the output quietly; any other error,
    /// I/O or not, is still reported.
    #[test]
    fn test_broken_pipe_is_recognised_through_context() {
        let pipe = anyhow::Error::new(io::Error::from(io::ErrorKind::BrokenPipe))
            .context("writing the table");
        assert!(is_broken_pipe(&pipe));

        let denied = anyhow::Error::new(io::Error::from(io::ErrorKind::PermissionDenied));
        assert!(!is_broken_pipe(&denied));
        assert!(!is_broken_pipe(&anyhow::anyhow!("scan failed")));
    }

    /// Focused: wait out the remainder of the configured refresh interval.
    #[test]
    fn test_scan_delay_uses_focused_interval() {
        let last = Instant::now();
        let now = last + Duration::from_millis(200);
        assert_eq!(
            scan_delay(last, now, true, Duration::from_secs(1)),
            Duration::from_millis(800)
        );
    }

    /// Unfocused: the idle interval is the floor even when `refresh` is much
    /// shorter — a backgrounded terminal is not worth 20 scans a second.
    #[test]
    fn test_scan_delay_backs_off_when_unfocused() {
        let last = Instant::now();
        let now = last + Duration::from_millis(200);
        assert_eq!(
            scan_delay(last, now, false, Duration::from_secs(1)),
            Duration::from_millis(4_800)
        );
    }

    /// An overdue scan has a zero delay (run it now, do not wait a negative
    /// time), and a configured refresh slower than the unfocused floor wins.
    #[test]
    fn test_scan_delay_is_saturating_and_honours_slow_refresh() {
        let last = Instant::now();
        assert_eq!(
            scan_delay(
                last,
                last + Duration::from_secs(3),
                true,
                Duration::from_secs(1)
            ),
            Duration::ZERO
        );
        assert_eq!(
            scan_delay(last, last, false, Duration::from_secs(10)),
            Duration::from_secs(10)
        );
    }

    /// The collector blocks in `recv_timeout`; `Shutdown` must wake it instead
    /// of leaving the UI to wait out a long refresh interval at exit.
    #[test]
    fn test_collector_exits_promptly_on_shutdown() {
        let dir = tempfile::tempdir().unwrap();
        let engine = Arc::new(Mutex::new(Engine::new(Some(dir.path()), Config::default())));
        let (tx, _rx) = mpsc::channel();
        let (ctrl_tx, ctrl_rx) = mpsc::channel();
        let (done_tx, done_rx) = mpsc::channel();
        let collector = thread::spawn(move || {
            collector_loop(engine, tx, Duration::from_secs(30), ctrl_rx);
            let _ = done_tx.send(());
        });

        ctrl_tx.send(Control::Shutdown).unwrap();
        done_rx
            .recv_timeout(Duration::from_secs(5))
            .expect("collector must stop on Control::Shutdown");
        collector.join().unwrap();
    }

    fn test_app() -> App {
        App::new(
            Config::default(),
            Theme::select("default-dark", false),
            false,
        )
    }

    fn snapshot_at(epoch_secs: u64) -> SharedSnapshot {
        Arc::new(portix_lib::model::Snapshot {
            services: Vec::new(),
            taken_at_epoch_secs: epoch_secs,
            scan_duration_ms: 0,
            degradations: Vec::new(),
        })
    }

    /// Pending snapshots are applied in order, and an empty channel is not an
    /// error — the loop must keep polling without redrawing.
    #[test]
    fn test_drain_snapshots_applies_pending_then_reports_empty() {
        let (tx, rx) = mpsc::channel();
        let mut app = test_app();

        assert!(!drain_snapshots(&rx, &mut app).unwrap());
        tx.send(snapshot_at(1)).unwrap();
        tx.send(snapshot_at(2)).unwrap();
        assert!(drain_snapshots(&rx, &mut app).unwrap());
        assert_eq!(app.snapshot.taken_at_epoch_secs, 2);
        assert!(!drain_snapshots(&rx, &mut app).unwrap());
    }

    /// A collector that dies stops sending but leaves the receiver alive: the
    /// old loop would sit on a frozen table forever. It must surface an error
    /// after draining whatever the collector managed to send.
    #[test]
    fn test_drain_snapshots_errors_when_collector_stops() {
        let (tx, rx) = mpsc::channel();
        let mut app = test_app();
        tx.send(snapshot_at(7)).unwrap();
        drop(tx);

        let err = drain_snapshots(&rx, &mut app).unwrap_err();
        assert_eq!(err.to_string(), "collector thread stopped");
        // The last snapshot was not thrown away on the way out.
        assert_eq!(app.snapshot.taken_at_epoch_secs, 7);
    }
}
