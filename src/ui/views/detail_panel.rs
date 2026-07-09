use crate::model::Service;
use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, Borders, Paragraph, Wrap},
    Frame,
};

/// Render the detail panel for a service.
pub fn render(f: &mut Frame, area: Rect, service: &Service) {
    let block = Block::default()
        .title(format!(" {} ", service.socket.url()))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Gray));

    let mut lines = Vec::new();

    // Framework section
    if let Some(ref framework) = service.framework {
        lines.push(Line::from(vec![
            Span::styled(
                framework.confidence.glyph().to_string(),
                Style::default().fg(Color::Green),
            ),
            Span::raw(" "),
            Span::styled(
                &framework.display_name,
                Style::default()
                    .fg(Color::Green)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(format!(
                " {}",
                framework.version.as_deref().unwrap_or("")
            )),
        ]));
    }

    lines.push(Line::raw(""));

    // Project section
    lines.push(Line::from(Span::styled(
        "PROJECT",
        Style::default()
            .fg(Color::Gray)
            .add_modifier(Modifier::BOLD),
    )));

    if let Some(ref project) = service.project {
        lines.push(Line::from(vec![
            Span::styled("  Name     ", Style::default().fg(Color::Gray)),
            Span::raw(&project.name),
        ]));
        lines.push(Line::from(vec![
            Span::styled("  Route    ", Style::default().fg(Color::Gray)),
            Span::raw(project.root.to_string_lossy()),
        ]));
        if let Some(ref ws) = project.workspace_root {
            lines.push(Line::from(vec![
                Span::styled("  Workspace", Style::default().fg(Color::Gray)),
                Span::raw(ws.to_string_lossy()),
            ]));
        }
    } else {
        lines.push(Line::from(Span::styled(
            "  —",
            Style::default().fg(Color::DarkGray),
        )));
    }

    lines.push(Line::raw(""));

    // Process section
    lines.push(Line::from(Span::styled(
        "PROCESS",
        Style::default()
            .fg(Color::Gray)
            .add_modifier(Modifier::BOLD),
    )));

    if let Some(ref process) = service.process {
        lines.push(Line::from(vec![
            Span::styled("  PID      ", Style::default().fg(Color::Gray)),
            Span::raw(format!("{}", process.pid)),
        ]));
        lines.push(Line::from(vec![
            Span::styled("  User     ", Style::default().fg(Color::Gray)),
            Span::raw(process.user.as_deref().unwrap_or("—")),
        ]));
        lines.push(Line::from(vec![
            Span::styled("  State    ", Style::default().fg(Color::Gray)),
            Span::raw(process.state.display()),
        ]));
        lines.push(Line::from(vec![
            Span::styled("  Threads  ", Style::default().fg(Color::Gray)),
            Span::raw(format!("{}", process.threads)),
        ]));
        if let Some(ref exe) = process.exe {
            lines.push(Line::from(vec![
                Span::styled("  Exec     ", Style::default().fg(Color::Gray)),
                Span::raw(exe.to_string_lossy()),
            ]));
        }
        if let Some(ref cwd) = process.cwd {
            lines.push(Line::from(vec![
                Span::styled("  CWD      ", Style::default().fg(Color::Gray)),
                Span::raw(cwd.to_string_lossy()),
            ]));
        }
    } else {
        lines.push(Line::from(Span::styled(
            "  — (no permission or process exited)",
            Style::default().fg(Color::DarkGray),
        )));
    }

    lines.push(Line::raw(""));

    // Socket section
    lines.push(Line::from(Span::styled(
        "SOCKET",
        Style::default()
            .fg(Color::Gray)
            .add_modifier(Modifier::BOLD),
    )));
    lines.push(Line::from(vec![
        Span::styled("  Listen   ", Style::default().fg(Color::Gray)),
        Span::raw(format!("[::]:{} ({})", service.socket.port, service.socket.protocol)),
    ]));
    lines.push(Line::from(vec![
        Span::styled("  Scope    ", Style::default().fg(Color::Gray)),
        Span::raw(format!("{:?}", service.socket.bind_scope)),
    ]));
    lines.push(Line::from(vec![
        Span::styled("  Inode    ", Style::default().fg(Color::Gray)),
        Span::raw(format!("{}", service.socket.inode)),
    ]));

    let paragraph = Paragraph::new(Text::from(lines))
        .block(block)
        .wrap(Wrap { trim: true });

    f.render_widget(paragraph, area);
}
