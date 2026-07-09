use crate::model::{Service, Signal};
use crate::ui::format::cell;
use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph},
    Frame,
};

/// Render the kill confirmation modal.
pub fn render(f: &mut Frame, area: Rect, service: Option<&Service>, signal: Signal) {
    let Some(service) = service else {
        return;
    };

    let area = centered_rect(56, 9, area);

    let (verb, color) = match signal {
        Signal::Kill => ("Force kill (SIGKILL)", Color::Red),
        _ => ("Kill (SIGTERM)", Color::Yellow),
    };

    let pid = service
        .process
        .as_ref()
        .map(|p| p.pid.to_string())
        .unwrap_or_else(|| "unknown".into());

    let lines = vec![
        Line::from(Span::styled(
            verb,
            Style::default().fg(color).add_modifier(Modifier::BOLD),
        )),
        Line::raw(""),
        Line::from(format!("  Port     :{}", service.socket.port)),
        Line::from(format!("  Project  {}", cell(&service.project_display()))),
        Line::from(format!("  Command  {}", cell(&service.command_display()))),
        Line::from(format!("  PID      {}", pid)),
        Line::raw(""),
        Line::from(Span::styled(
            "  Enter / y  confirm      Esc / n  cancel",
            Style::default().fg(Color::Gray),
        )),
    ];

    let block = Block::default()
        .title(" Confirm ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(color));

    f.render_widget(Clear, area);
    f.render_widget(
        Paragraph::new(lines)
            .block(block)
            .alignment(Alignment::Left),
        area,
    );
}

/// A rect of the given size, centered in `r`.
fn centered_rect(width: u16, height: u16, r: Rect) -> Rect {
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(r.height.saturating_sub(height) / 2),
            Constraint::Length(height.min(r.height)),
            Constraint::Min(0),
        ])
        .split(r);

    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Length(r.width.saturating_sub(width) / 2),
            Constraint::Length(width.min(r.width)),
            Constraint::Min(0),
        ])
        .split(vertical[1])[1]
}
