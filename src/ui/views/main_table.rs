use crate::model::Service;
use crate::ui::format::{cell, format_bytes_compact, format_uptime, truncate_center};
use crate::ui::theme::Theme;
use ratatui::{
    layout::Rect,
    style::{Modifier, Style},
    widgets::{Cell, Row, Table, TableState},
    Frame,
};

const DASH: &str = "—";

/// Render the main service table.
pub fn render(f: &mut Frame, area: Rect, services: &[&Service], selected: usize, theme: &Theme) {
    let header = Row::new(vec![
        "PORT",
        "PROTO",
        "SCOPE",
        "PROJECT",
        "FRAMEWORK",
        "COMMAND",
        "PID",
        "CPU",
        "MEM",
        "UPTIME",
    ])
    .style(
        Style::default()
            .fg(theme.label)
            .add_modifier(Modifier::BOLD),
    );

    let rows: Vec<Row> = services
        .iter()
        .map(|service| {
            let process = service.process.as_ref();

            // Every one of these strings can carry attacker-controlled bytes
            // (project names, argv). `cell` is the sanitizing chokepoint.
            let port = format!("{:>5}", service.socket.port);
            let proto = service.socket.proto_display();
            let scope = Cell::from(service.socket.scope_label()).style(
                if service.socket.is_network_bound() {
                    Style::default().fg(theme.warn).add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(theme.ok)
                },
            );
            let project = cell(&truncate_center(&service.project_display(), 20));
            let framework = cell(&truncate_center(&service.framework_display(), 16));
            let command = cell(&truncate_center(&service.command_display(), 16));

            let pid = process
                .map(|p| format!("{:>6}", p.pid))
                .unwrap_or_else(|| format!("{:>6}", DASH));
            let cpu = process
                .and_then(|p| p.metrics.cpu_percent)
                .map(|c| format!("{:>5.1}%", c))
                .unwrap_or_else(|| format!("{:>6}", DASH));
            let mem = process
                .and_then(|p| p.metrics.rss_bytes)
                .map(format_bytes_compact)
                .unwrap_or_else(|| DASH.to_string());
            let uptime = process
                .and_then(|p| p.metrics.uptime)
                .map(format_uptime)
                .unwrap_or_else(|| DASH.to_string());

            Row::new(vec![
                port.into(),
                proto.into(),
                scope,
                project.into(),
                framework.into(),
                command.into(),
                pid.into(),
                cpu.into(),
                mem.into(),
                uptime.into(),
            ])
        })
        .collect();

    let widths = [
        ratatui::layout::Constraint::Length(6),
        ratatui::layout::Constraint::Length(6),
        ratatui::layout::Constraint::Length(8),
        ratatui::layout::Constraint::Min(15),
        ratatui::layout::Constraint::Length(17),
        ratatui::layout::Constraint::Length(17),
        ratatui::layout::Constraint::Length(7),
        ratatui::layout::Constraint::Length(7),
        ratatui::layout::Constraint::Length(7),
        ratatui::layout::Constraint::Length(8),
    ];

    let table = Table::new(rows, widths)
        .header(header)
        .row_highlight_style(
            Style::default()
                .bg(theme.selection)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("▸ ");

    let mut state = TableState::default();
    if !services.is_empty() {
        state.select(Some(selected));
    }

    f.render_stateful_widget(table, area, &mut state);
}
