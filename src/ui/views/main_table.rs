use crate::model::Service;
use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Row, Table, TableState},
    Frame,
};

/// Render the main service table.
pub fn render(
    f: &mut Frame,
    area: Rect,
    services: &[&Service],
    selected: usize,
    show_detail: bool,
) {
    // Calculate column widths based on available width
    let width = area.width as usize;

    let header = Row::new(vec![
        "PORT",
        "PROTO",
        "PROJECT",
        "FRAMEWORK",
        "PID",
        "CPU",
        "MEM",
        "UPTIME",
    ])
    .style(
        Style::default()
            .fg(Color::Gray)
            .add_modifier(Modifier::BOLD),
    );

    let rows: Vec<Row> = services
        .iter()
        .enumerate()
        .map(|(idx, service)| {
            let port = format!("{:>5}", service.socket.port);
            let proto = format!("{:<5}", service.socket.protocol);
            let project = truncate_center(&service.project_display(), 20);
            let framework = truncate_center(&service.framework_display(), 16);
            let pid = service
                .process
                .as_ref()
                .map(|p| format!("{:>5}", p.pid))
                .unwrap_or_else(|| "    —".to_string());
            let cpu = service
                .process
                .as_ref()
                .and_then(|p| p.metrics.cpu_percent)
                .map(|c| format!("{:>5.1}%", c))
                .unwrap_or_else(|| "    —".to_string());
            let mem = service
                .process
                .as_ref()
                .and_then(|p| p.metrics.rss_bytes)
                .map(|m| format_mem(m))
                .unwrap_or_else(|| "   —".to_string());
            let uptime = service
                .process
                .as_ref()
                .and_then(|p| p.metrics.uptime)
                .map(|u| format_duration(u))
                .unwrap_or_else(|| "   —".to_string());

            Row::new(vec![port, proto, project, framework, pid, cpu, mem, uptime])
        })
        .collect();

    let widths = [
        ratatui::layout::Constraint::Length(6),
        ratatui::layout::Constraint::Length(6),
        ratatui::layout::Constraint::Min(15),
        ratatui::layout::Constraint::Length(18),
        ratatui::layout::Constraint::Length(6),
        ratatui::layout::Constraint::Length(6),
        ratatui::layout::Constraint::Length(5),
        ratatui::layout::Constraint::Length(8),
    ];

    let table = Table::new(rows, widths)
        .header(header)
        .highlight_style(
            Style::default()
                .bg(Color::DarkGray)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("▸ ");

    let mut state = TableState::default();
    state.select(Some(selected));

    f.render_stateful_widget(table, area, &mut state);
}

/// Truncate a string from the center.
fn truncate_center(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        s.to_string()
    } else {
        let keep = (max_len - 3) / 2;
        format!("{}...{}", &s[..keep], &s[s.len() - keep..])
    }
}

/// Format memory size.
fn format_mem(bytes: u64) -> String {
    if bytes < 1024 {
        format!("{} B", bytes)
    } else if bytes < 1024 * 1024 {
        format!("{:.0} K", bytes as f64 / 1024.0)
    } else if bytes < 1024 * 1024 * 1024 {
        format!("{:.0} M", bytes as f64 / (1024.0 * 1024.0))
    } else {
        format!("{:.1} G", bytes as f64 / (1024.0 * 1024.0 * 1024.0))
    }
}

/// Format duration.
fn format_duration(d: std::time::Duration) -> String {
    let secs = d.as_secs();
    if secs < 60 {
        format!("{}s", secs)
    } else if secs < 3600 {
        format!("{}m", secs / 60)
    } else {
        format!("{}h{}m", secs / 3600, (secs % 3600) / 60)
    }
}
