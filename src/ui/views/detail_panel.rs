use crate::model::{BindScope, Service};
use crate::ui::format::{cell, format_bytes, format_uptime};
use crate::ui::theme::Theme;
use ratatui::{
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph, Wrap},
    Frame,
};

/// Render the info panel for the selected service as a centered modal overlay.
///
/// Everything drawn here that comes from the system — command line, paths,
/// project and container names — is attacker-controlled and passes through
/// `format::cell`, the mandatory sanitisation chokepoint (PRD §13.2). A process
/// with `argv[0] = "\x1b]0;pwned\x07"` must not hijack the terminal from here.
pub fn render(f: &mut Frame, area: Rect, service: Option<&Service>, theme: &Theme) {
    let area = centered_rect(66, 84, area);
    f.render_widget(Clear, area);

    let Some(service) = service else {
        let block = Block::default()
            .title(" Info ")
            .borders(Borders::ALL)
            .border_style(Style::default().fg(theme.muted));
        f.render_widget(Paragraph::new("No service selected.").block(block), area);
        return;
    };

    // Local styling helpers so the many rows below stay terse. Colors are Copy,
    // captured here from the active theme.
    let label_c = theme.label;
    let muted_c = theme.muted;
    let section = |title: &str| {
        Line::from(Span::styled(
            title.to_string(),
            Style::default().fg(label_c).add_modifier(Modifier::BOLD),
        ))
    };
    let kv = |label: &str, value: String| {
        Line::from(vec![
            Span::styled(format!("  {label:<9} "), Style::default().fg(muted_c)),
            Span::raw(value),
        ])
    };
    let dash_msg =
        |msg: &str| Line::from(Span::styled(msg.to_string(), Style::default().fg(muted_c)));
    let blank = || Line::raw("");

    let mut lines: Vec<Line> = Vec::new();

    // Framework (only when something was actually detected).
    if let Some(fw) = &service.framework {
        lines.push(Line::from(vec![
            Span::styled(
                fw.confidence.glyph().to_string(),
                Style::default().fg(theme.ok),
            ),
            Span::raw(" "),
            Span::styled(
                cell(&fw.display_name),
                Style::default().fg(theme.ok).add_modifier(Modifier::BOLD),
            ),
            Span::raw(format!(" {}", fw.version.as_deref().unwrap_or(""))),
            Span::styled(
                format!("  ({})", fw.confidence.display()),
                Style::default().fg(theme.muted),
            ),
        ]));
        if !fw.matched_signals.is_empty() {
            lines.push(kv("Evidence", cell(&fw.matched_signals.join(", "))));
        }
        lines.push(blank());
    }

    // Project. Paths are shortened to `~/…` when under $HOME so the common case
    // reads clearly instead of wrapping on the home prefix.
    let home = std::env::var_os("HOME").map(std::path::PathBuf::from);
    lines.push(section("PROJECT"));
    if let Some(project) = &service.project {
        lines.push(kv("Name", cell(&project.name)));
        if let Some(tech) = &project.technical_name {
            lines.push(kv("Package", cell(tech)));
        }
        lines.push(kv("Route", cell(&project.short_path(home.as_deref()))));
        if let Some(ws) = &project.workspace_root {
            lines.push(kv("Workspace", cell(&ws.to_string_lossy())));
        }
        if let Some(lang) = &project.language {
            lines.push(kv("Language", cell(lang)));
        }
        if let Some(pm) = &project.package_manager {
            lines.push(kv("Manager", pm.display().to_string()));
        }
    } else {
        lines.push(dash_msg("  —"));
    }
    lines.push(blank());

    // Process.
    lines.push(section("PROCESS"));
    if let Some(p) = &service.process {
        lines.push(kv("Command", cell(&p.command_display())));
        lines.push(kv("PID", p.pid.to_string()));
        lines.push(kv("PPID", p.ppid.to_string()));
        lines.push(kv("User", cell(p.user.as_deref().unwrap_or("—"))));
        lines.push(kv("State", p.state.display().to_string()));
        lines.push(kv("Threads", p.threads.to_string()));
        lines.push(kv(
            "CPU",
            p.metrics
                .cpu_percent
                .map(|c| format!("{c:.1}%"))
                .unwrap_or_else(|| "—".into()),
        ));
        lines.push(kv("Memory", memory_line(p)));
        lines.push(kv(
            "Uptime",
            p.metrics
                .uptime
                .map(format_uptime)
                .unwrap_or_else(|| "—".into()),
        ));
        if let Some(exe) = &p.exe {
            lines.push(kv("Exec", cell(&exe.to_string_lossy())));
        }
        if let Some(cwd) = &p.cwd {
            lines.push(kv("CWD", cell(&cwd.to_string_lossy())));
        }
    } else {
        lines.push(dash_msg("— (no permission, or the process exited)"));
    }
    lines.push(blank());

    // Container (only when detected).
    if let Some(c) = &service.container {
        lines.push(section("CONTAINER"));
        lines.push(kv("Runtime", c.runtime.display().to_string()));
        lines.push(kv("ID", cell(&c.id)));
        if let Some(name) = &c.name {
            lines.push(kv("Name", cell(name)));
        }
        if let Some(compose) = &c.compose_project {
            lines.push(kv("Compose", cell(compose)));
        }
        lines.push(blank());
    }

    // Socket.
    lines.push(section("SOCKET"));
    lines.push(kv("URL", service.socket.url()));
    lines.push(kv("Listen", listen_addr(service)));
    lines.push(kv("Protocol", service.socket.proto_display()));
    lines.push(kv("Scope", bind_scope_label(&service.socket.bind_scope)));
    lines.push(kv("Inode", service.socket.inode.to_string()));
    lines.push(kv("Owner", format!("uid {}", service.socket.uid)));
    if service.socket.dual_stack {
        lines.push(kv("Dual-stack", "IPv4 + IPv6".to_string()));
    }

    lines.push(blank());
    lines.push(Line::from(Span::styled(
        "  Esc / i to close",
        Style::default().fg(theme.muted),
    )));

    let title = format!(
        " :{}  {} ",
        service.socket.port,
        service.socket.proto_display()
    );
    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.accent));

    // Wrap so long command lines and paths fold inside the modal.
    let paragraph = Paragraph::new(lines)
        .block(block)
        .wrap(Wrap { trim: false });

    f.render_widget(paragraph, area);
}

/// `RSS` alone, or `RSS / VIRT` when the virtual size is known too.
fn memory_line(p: &crate::model::Process) -> String {
    match (p.metrics.rss_bytes, p.metrics.virt_bytes) {
        (Some(rss), Some(virt)) => format!("{} / {} virt", format_bytes(rss), format_bytes(virt)),
        (Some(rss), None) => format_bytes(rss),
        _ => "—".to_string(),
    }
}

/// The real listen address, bracketing IPv6 the standard way (`[::]:7777`)
/// rather than the ambiguous `:::7777`. Fixes the old hardcoded `[::]:{port}`
/// that ignored the actual address.
fn listen_addr(service: &Service) -> String {
    use std::net::IpAddr;
    match service.socket.local_addr {
        IpAddr::V4(v4) => format!("{v4}:{}", service.socket.port),
        IpAddr::V6(v6) => format!("[{v6}]:{}", service.socket.port),
    }
}

/// Human-readable bind scope, replacing the old `{:?}` Debug output.
fn bind_scope_label(scope: &BindScope) -> String {
    match scope {
        BindScope::Loopback => "loopback (localhost only)".to_string(),
        BindScope::AllInterfaces => "all interfaces".to_string(),
        BindScope::Specific(ip) => ip.to_string(),
    }
}

/// Create a centered rectangle covering `percent_x` × `percent_y` of `r`.
fn centered_rect(percent_x: u16, percent_y: u16, r: Rect) -> Rect {
    use ratatui::layout::{Constraint, Direction, Layout};
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(r);

    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(vertical[1])[1]
}
