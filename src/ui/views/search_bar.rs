use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
    Frame,
};

/// Render the search bar.
pub fn render(f: &mut Frame, area: Rect, query: &str, total: usize, matched: usize) {
    let block = Block::default()
        .title(" Search ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Yellow));

    let cursor = "▏";

    let line = Line::from(vec![
        Span::styled(query, Style::default().fg(Color::White)),
        Span::styled(cursor, Style::default().fg(Color::Yellow)),
        Span::raw("  "),
        Span::styled(
            format!("{} of {}", matched, total),
            Style::default().fg(Color::Gray),
        ),
    ]);

    let paragraph = Paragraph::new(line).block(block);

    f.render_widget(paragraph, area);
}
