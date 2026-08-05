use crate::ui::theme::Theme;
use ratatui::{
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph},
    Frame,
};

/// Render the theme picker modal. The UI behind it already reflects the
/// previewed theme (App swaps the palette live as the cursor moves), so this
/// only draws the list of choices.
pub fn render(f: &mut Frame, area: Rect, cursor: usize, theme: &Theme) {
    let area = super::centered_fixed(46, (Theme::NAMES.len() + 6) as u16, area);
    f.render_widget(Clear, area);

    let mut lines = vec![
        Line::from(Span::styled(
            "Theme",
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD),
        )),
        Line::raw(""),
    ];

    for (i, &name) in Theme::NAMES.iter().enumerate() {
        let selected = i == cursor;
        let marker = if selected { "▸ " } else { "  " };
        let name_style = if selected {
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme.text)
        };
        lines.push(Line::from(vec![
            Span::styled(marker, Style::default().fg(theme.accent)),
            Span::styled(format!("{:<14}", pretty_name(name)), name_style),
            Span::styled(description(name), Style::default().fg(theme.muted)),
        ]));
    }

    lines.push(Line::raw(""));
    lines.push(Line::from(Span::styled(
        "  j/k preview · Enter save · Esc cancel",
        Style::default().fg(theme.muted),
    )));

    let block = Block::default()
        .title(" Theme ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.accent));

    f.render_widget(Paragraph::new(lines).block(block), area);
}

fn pretty_name(name: &str) -> &str {
    match name {
        "default-dark" => "Dark",
        "mono" => "Monochrome",
        "auto" => "Auto",
        other => other,
    }
}

fn description(name: &str) -> &str {
    match name {
        "default-dark" => "full color (default)",
        "mono" => "grays only",
        "auto" => "match the terminal",
        _ => "",
    }
}
