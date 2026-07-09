use anyhow::Result;
use clap::Parser;
use crossterm::{
    event::{self, Event, KeyCode, KeyModifiers},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout},
    Terminal,
};
use std::io;
use std::time::Duration;

use portix_lib::core::{Config, Engine};
use portix_lib::ui::app::{App, Effect};

#[derive(Parser, Debug)]
#[command(name = "portix")]
#[command(about = "The local development service explorer")]
#[command(version)]
struct Cli {
    /// Show service for a specific port
    #[arg(value_name = "PORT")]
    port: Option<u16>,

    /// Output JSON
    #[arg(long)]
    json: bool,

    /// Single frame, no TUI
    #[arg(long)]
    once: bool,

    /// Kill service on port
    #[arg(long, value_name = "PORT")]
    kill: Option<u16>,

    /// Skip confirmation for kill
    #[arg(long)]
    yes: bool,

    /// Alternative /proc root (for testing/containers)
    #[arg(long, value_name = "PATH")]
    proc_root: Option<String>,

    /// Refresh interval in ms
    #[arg(long, value_name = "MS", default_value = "1000")]
    interval: u64,

    /// Disable colors
    #[arg(long)]
    no_color: bool,

    /// Output completions for a shell
    #[arg(long, value_name = "SHELL")]
    completions: Option<clap_complete::Shell>,
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    // Handle completions
    if let Some(shell) = cli.completions {
        let mut cmd = <Cli as clap::CommandFactory>::command();
        clap_complete::generate(shell, &mut cmd, "portix", &mut io::stdout());
        return Ok(());
    }

    // Load configuration
    let config = Config::load();

    // Create engine
    let proc_root = cli.proc_root.as_deref().map(std::path::Path::new);
    let mut engine = Engine::new(proc_root)?;

    // Handle --once mode
    if cli.once {
        let snapshot = engine.scan()?;
        println!("Services: {}", snapshot.services.len());
        for service in &snapshot.services {
            println!(
                "  :{} {} {}",
                service.socket.port,
                service.project_display(),
                service.framework_display()
            );
        }
        return Ok(());
    }

    // Handle --json mode
    if cli.json {
        let snapshot = engine.scan()?;
        let json = serde_json::to_string_pretty(&*snapshot)?;
        println!("{}", json);
        return Ok(());
    }

    // Handle --port mode
    if let Some(port) = cli.port {
        let snapshot = engine.scan()?;
        if let Some(service) = snapshot.services.iter().find(|s| s.socket.port == port) {
            println!(
                ":{} {} {}",
                service.socket.port,
                service.project_display(),
                service.framework_display()
            );
        } else {
            eprintln!("No service listening on port {}", port);
            std::process::exit(3);
        }
        return Ok(());
    }

    // Handle --kill mode
    if let Some(port) = cli.kill {
        let snapshot = engine.scan()?;
        if let Some(service) = snapshot.services.iter().find(|s| s.socket.port == port) {
            if !cli.yes {
                println!(
                    "Kill service on port {}? (PID: {:?})",
                    port,
                    service.process.as_ref().map(|p| p.pid)
                );
                println!("Press Enter to confirm, Ctrl+C to cancel");
                let mut input = String::new();
                io::stdin().read_line(&mut input)?;
                if input.trim() != "" {
                    return Ok(());
                }
            }
            portix_lib::core::actions::kill_service(service, false)?;
            println!("Killed service on port {}", port);
        } else {
            eprintln!("No service listening on port {}", port);
            std::process::exit(3);
        }
        return Ok(());
    }

    // Run TUI
    run_tui(config, engine)
}

fn run_tui(config: Config, mut engine: Engine) -> Result<()> {
    // Setup terminal
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    // Create app
    let mut app = App::new(config.clone());

    // Initial scan
    let snapshot = engine.scan()?;
    app.update_snapshot(snapshot);

    // Main loop
    let tick_rate = Duration::from_millis(config.min_refresh_ms());
    let mut last_scan = std::time::Instant::now();

    loop {
        // Draw
        terminal.draw(|f| {
            let chunks = Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Min(10),    // Table
                    Constraint::Length(1),  // Status bar
                ])
                .split(f.area());

            // Render table
            let services: Vec<&portix_lib::model::Service> = app
                .filtered_indices
                .iter()
                .filter_map(|&idx| app.snapshot.services.get(idx))
                .collect();

            portix_lib::ui::views::main_table::render(
                f,
                chunks[0],
                &services,
                app.selected,
                app.detail_open,
            );

            // Render status bar
            portix_lib::ui::views::status_bar::render(
                f,
                chunks[1],
                &app.snapshot,
                !app.query.query().is_empty(),
            );

            // Render search bar if in search mode
            if app.mode == portix_lib::ui::app::Mode::Search {
                let search_area = ratatui::layout::Layout::default()
                    .direction(ratatui::layout::Direction::Vertical)
                    .constraints([
                        Constraint::Min(10),
                        Constraint::Length(3),
                        Constraint::Length(1),
                    ])
                    .split(f.area());
                portix_lib::ui::views::search_bar::render(
                    f,
                    search_area[1],
                    &app.search_buffer,
                    app.snapshot.services.len(),
                    app.filtered_indices.len(),
                );
            }

            // Render help if open
            if app.show_help {
                portix_lib::ui::views::help::render(f, f.area());
            }
        })?;

        // Handle events
        if event::poll(tick_rate)? {
            if let Event::Key(key) = event::read()? {
                // Ctrl+C always quits
                if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL)
                {
                    break;
                }

                let effects = app.handle_key(key);
                for effect in effects {
                    match effect {
                        Effect::Quit => break,
                        Effect::OpenBrowser(url) => {
                            let _ = std::process::Command::new("xdg-open")
                                .arg(&url)
                                .spawn();
                        }
                        Effect::Kill(id, force) => {
                            // Find the service and kill it
                            if let Some(service) = app
                                .snapshot
                                .services
                                .iter()
                                .find(|s| s.id == id)
                            {
                                let _ = portix_lib::core::actions::kill_service(service, force);
                            }
                        }
                        Effect::CopyToClipboard(text) => {
                            let _ = portix_lib::core::actions::copy_to_clipboard(&text);
                        }
                        Effect::Refresh => {
                            // Force refresh
                        }
                    }
                }
            }
        }

        // Periodic refresh
        if last_scan.elapsed() >= Duration::from_millis(config.min_refresh_ms()) {
            if let Ok(snapshot) = engine.scan() {
                app.update_snapshot(snapshot);
                last_scan = std::time::Instant::now();
            }
        }
    }

    // Restore terminal
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;

    Ok(())
}
