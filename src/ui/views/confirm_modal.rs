use crate::model::{Service, Signal};
use crate::ui::format::cell;
use crate::ui::theme::Theme;
use ratatui::{
    layout::{Alignment, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph},
    Frame,
};

/// Render the kill confirmation modal.
pub fn render(f: &mut Frame, area: Rect, service: Option<&Service>, signal: Signal, theme: &Theme) {
    let Some(service) = service else {
        return;
    };

    let area = super::centered_fixed(56, 9, area);

    let (verb, color) = match signal {
        Signal::Kill => ("Force kill (SIGKILL)", theme.danger),
        _ => ("Kill (SIGTERM)", theme.warn),
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
            Style::default().fg(theme.label),
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
