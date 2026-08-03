use anyhow::Result;
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
use std::io::{self, IsTerminal};
use std::time::{Duration, Instant};

use portix_lib::core::{Config, Engine};
use portix_lib::model::{Service, SharedSnapshot, Signal};
use portix_lib::ui::app::{App, Effect, Mode};
use portix_lib::ui::format::{cell, format_bytes_compact, format_uptime, truncate_center};

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
    #[arg(value_name = "PORT", value_parser = parse_port)]
    port: Option<u16>,

    /// Output JSON
    #[arg(long)]
    json: bool,

    /// Single frame, no TUI
    #[arg(long)]
    once: bool,

    /// Kill service on port
    #[arg(long, value_name = "PORT", value_parser = parse_port)]
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
            eprintln!("portix: {e:#}");
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

    let mut config = Config::load();
    if let Some(ms) = cli.interval {
        config.refresh_ms = ms;
    }
    if cli.system_ports {
        config.show_system_ports = true;
    }

    let proc_root = cli.proc_root.as_deref().map(std::path::Path::new);
    let mut engine = Engine::new(proc_root, config.clone())?;

    if let Some(port) = cli.kill {
        return kill_port(&mut engine, port, cli.force, cli.yes);
    }
    if cli.json {
        return print_json(&mut engine, cli.port);
    }
    if let Some(port) = cli.port {
        return print_port(&mut engine, port);
    }
    if cli.once {
        return print_table(&mut engine);
    }

    run_tui(config, engine, use_color(&cli))?;
    Ok(exit::OK)
}

fn use_color(cli: &Cli) -> bool {
    !cli.no_color && std::env::var_os("NO_COLOR").is_none()
}

/// CPU needs two samples to produce a delta. A one-shot invocation would print
/// a dash for every row, so take a throwaway scan and wait one interval.
fn scan_with_cpu(engine: &mut Engine) -> Result<SharedSnapshot> {
    engine.scan()?;
    std::thread::sleep(Duration::from_millis(200));
    engine.scan()
}

fn print_json(engine: &mut Engine, port: Option<u16>) -> Result<i32> {
    let snapshot = scan_with_cpu(engine)?;

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
    let snapshot = scan_with_cpu(engine)?;
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
    let snapshot = scan_with_cpu(engine)?;

    println!(
        "{:<6} {:<6} {:<20} {:<18} {:<18} {:>7} {:>7} {:>7} {:>8}",
        "PORT", "PROTO", "PROJECT", "FRAMEWORK", "COMMAND", "PID", "CPU", "MEM", "UPTIME"
    );

    for service in snapshot.sorted_by_port() {
        let process = service.process.as_ref();
        println!(
            "{:>5}  {:<6} {:<20} {:<18} {:<18} {:>7} {:>7} {:>7} {:>8}",
            service.socket.port,
            service.socket.proto_display(),
            cell(&truncate_center(&service.project_display(), 20)),
            cell(&truncate_center(&service.framework_display(), 18)),
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
        eprintln!("warning: {}", degradation.message());
    }

    Ok(exit::OK)
}

fn kill_port(engine: &mut Engine, port: u16, force: bool, yes: bool) -> Result<i32> {
    let snapshot = scan_with_cpu(engine)?;
    let Some(service) = snapshot.by_port(port) else {
        eprintln!("No service listening on port {port}");
        return Ok(exit::NO_MATCH);
    };

    if !yes {
        let pid = service
            .process
            .as_ref()
            .map(|p| p.pid.to_string())
            .unwrap_or_else(|| "unknown".into());

        eprint!(
            "Kill {} on port {port} (pid {pid})? [y/N] ",
            cell(&service.project_display())
        );
        io::Write::flush(&mut io::stderr())?;

        // Only an explicit yes proceeds. A bare Enter, EOF, or anything else
        // cancels: this prompt is the last thing between the user and SIGKILL.
        let mut input = String::new();
        io::stdin().read_line(&mut input)?;
        if !matches!(input.trim(), "y" | "Y" | "yes") {
            eprintln!("Cancelled.");
            return Ok(exit::OK);
        }
    }

    let signal = if force { Signal::Kill } else { Signal::Term };
    engine.signal_service(service, signal)?;
    eprintln!("Sent {signal:?} to service on port {port}");
    Ok(exit::OK)
}

fn run_tui(config: Config, mut engine: Engine, _color: bool) -> Result<()> {
    if !io::stdout().is_terminal() {
        anyhow::bail!("stdout is not a terminal; use --once or --json");
    }

    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let result = event_loop(&mut terminal, config, &mut engine);

    // Restore the terminal even if the loop failed. Leaving a user in raw mode
    // on the alternate screen is worse than whatever error got us here.
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;

    result
}

fn event_loop<B: ratatui::backend::Backend>(
    terminal: &mut Terminal<B>,
    config: Config,
    engine: &mut Engine,
) -> Result<()> {
    let mut app = App::new(config.clone());
    app.update_snapshot(engine.scan()?);

    let refresh = Duration::from_millis(config.min_refresh_ms());
    let mut last_scan = Instant::now();

    loop {
        terminal.draw(|f| draw(f, &app))?;

        // Wake up for input, but never sleep past the next refresh.
        let timeout = refresh.saturating_sub(last_scan.elapsed());
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
            }
        }

        if last_scan.elapsed() >= refresh {
            match engine.scan() {
                Ok(snapshot) => app.update_snapshot(snapshot),
                Err(e) => app.error = Some(e.to_string()),
            }
            last_scan = Instant::now();
        }
    }
}

#[derive(PartialEq, Eq)]
enum ControlFlow {
    Continue,
    Quit,
}

fn run_effect(effect: Effect, app: &mut App, engine: &mut Engine) -> ControlFlow {
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

        Effect::Kill(id, signal) => {
            let service = app.snapshot.services.iter().find(|s| s.id == id).cloned();
            match service {
                // Errors here are the TOCTOU guard refusing to signal a recycled
                // PID. That must be visible, not swallowed.
                Some(service) => {
                    if let Err(e) = engine.signal_service(&service, signal) {
                        app.error = Some(e.to_string());
                    }
                }
                None => app.error = Some("service disappeared".into()),
            }
        }

        Effect::Refresh => match engine.scan() {
            Ok(snapshot) => app.update_snapshot(snapshot),
            Err(e) => app.error = Some(e.to_string()),
        },
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

    portix_lib::ui::views::main_table::render(f, chunks[0], &services, app.selected);
    portix_lib::ui::views::status_bar::render(
        f,
        chunks[1],
        &app.snapshot,
        !app.query.query().is_empty(),
        app.error.as_deref(),
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
        );
    }

    if app.detail_open {
        portix_lib::ui::views::detail_panel::render(f, f.area(), app.selected_service());
    }

    if let Some(signal) = app.pending_kill() {
        portix_lib::ui::views::confirm_modal::render(f, f.area(), app.selected_service(), signal);
    }

    if app.show_help() {
        portix_lib::ui::views::help::render(f, f.area(), &app.snapshot.degradations);
    }
}
