use crate::ui::theme::Theme;
use ratatui::{
    layout::Rect,
    style::Style,
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
    Frame,
};

/// Render the search bar.
pub fn render(f: &mut Frame, area: Rect, query: &str, total: usize, matched: usize, theme: &Theme) {
    let block = Block::default()
        .title(" Search ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.warn));

    let cursor = "▏";

    let line = Line::from(vec![
        Span::styled(query, Style::default().fg(theme.text)),
        Span::styled(cursor, Style::default().fg(theme.warn)),
        Span::raw("  "),
        Span::styled(
            format!("{} of {}", matched, total),
            Style::default().fg(theme.label),
        ),
    ]);

    let paragraph = Paragraph::new(line).block(block);

    f.render_widget(paragraph, area);
}
