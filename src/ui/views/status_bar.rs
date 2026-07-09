use crate::model::Snapshot;
use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    Frame,
};

/// Render the status bar at the bottom.
pub fn render(
    f: &mut Frame,
    area: Rect,
    snapshot: &Snapshot,
    search_active: bool,
    error: Option<&str>,
) {
    // An effect failure — notably the TOCTOU guard refusing to signal a recycled
    // PID — takes over the bar until the next keypress clears it. It must not be
    // swallowed.
    if let Some(err) = error {
        let line = Line::from(Span::styled(
            format!("⚠ {err}"),
            Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
        ));
        f.render_widget(line, area);
        return;
    }

    let services_count = snapshot.services.len();
    let projects_count: usize = snapshot
        .services
        .iter()
        .filter(|s| s.project.is_some())
        .count();
    let containers_count: usize = snapshot
        .services
        .iter()
        .filter(|s| s.container.is_some())
        .count();

    let scan_ms = snapshot.scan_duration_ms;

    let mut spans = vec![
        Span::styled(
            format!("{} services", services_count),
            Style::default().fg(Color::Gray),
        ),
        Span::raw(" · "),
        Span::styled(
            format!("{} projects", projects_count),
            Style::default().fg(Color::Gray),
        ),
    ];

    if containers_count > 0 {
        spans.push(Span::raw(" · "));
        spans.push(Span::styled(
            format!("{} containers", containers_count),
            Style::default().fg(Color::Blue),
        ));
    }

    spans.push(Span::raw("            "));

    // Scan time
    spans.push(Span::styled(
        format!("⟳ {}ms", scan_ms),
        Style::default().fg(Color::DarkGray),
    ));

    // Search indicator
    if search_active {
        spans.push(Span::raw("  "));
        spans.push(Span::styled(
            "/",
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        ));
    }

    // Degradations
    if !snapshot.degradations.is_empty() {
        spans.push(Span::raw("  "));
        spans.push(Span::styled(
            format!("⚠ {}", snapshot.degradations.len()),
            Style::default().fg(Color::Yellow),
        ));
    }

    let line = Line::from(spans);

    f.render_widget(line, area);
}
