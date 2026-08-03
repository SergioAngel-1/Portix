use crate::model::Degradation;
use crate::ui::theme::Theme;
use ratatui::{
    layout::{Alignment, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph, Wrap},
    Frame,
};

/// Render the help modal. Active scan degradations are listed at the bottom, so
/// the `⚠` count in the footer has somewhere to unfold into.
pub fn render(f: &mut Frame, area: Rect, degradations: &[Degradation], theme: &Theme) {
    let area = centered_rect(60, 80, area);

    let header = Style::default().fg(theme.label).add_modifier(Modifier::BOLD);

    let mut help_text = vec![
        Line::from(Span::styled(
            "Portix · Keyboard Shortcuts",
            Style::default().fg(theme.accent).add_modifier(Modifier::BOLD),
        )),
        Line::raw(""),
        Line::from(Span::styled("NAVIGATION", header)),
        Line::from(vec![Span::raw("  ↑ k        up              g   first")]),
        Line::from(vec![Span::raw("  ↓ j        down            G   last")]),
        Line::from(vec![Span::raw("  i Enter l  info            h   close")]),
        Line::raw(""),
        Line::from(Span::styled("SEARCH & FILTER", header)),
        Line::from(vec![Span::raw("  /          search          Esc cancel")]),
        Line::raw(""),
        Line::from(Span::styled("ACTIONS", header)),
        Line::from(vec![Span::raw("  x          kill (SIGTERM)")]),
        Line::from(vec![Span::raw("  X          force kill (SIGKILL)")]),
        Line::from(vec![Span::raw("  o          open in browser")]),
        Line::from(vec![Span::raw("  y          copy URL")]),
        Line::raw(""),
        Line::from(Span::styled("GENERAL", header)),
        Line::from(vec![Span::raw("  r          refresh         q   quit")]),
        Line::from(vec![Span::raw("  t          change theme    ?   this help")]),
    ];

    // The warnings section only appears when the scan actually degraded, so an
    // empty run stays uncluttered.
    if !degradations.is_empty() {
        help_text.push(Line::raw(""));
        help_text.push(Line::from(Span::styled("WARNINGS", header)));
        for degradation in degradations {
            help_text.push(Line::from(vec![
                Span::styled("  ⚠ ", Style::default().fg(theme.warn)),
                Span::styled(degradation.message(), Style::default().fg(theme.warn)),
            ]));
        }
    }

    help_text.push(Line::raw(""));
    help_text.push(Line::from(vec![Span::raw("              Esc or ? to close")]));

    let block = Block::default()
        .title(" Help ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.accent));

    // Wrap so a long warning message folds inside the modal instead of clipping.
    let paragraph = Paragraph::new(help_text)
        .block(block)
        .alignment(Alignment::Left)
        .wrap(Wrap { trim: false });

    f.render_widget(Clear, area);
    f.render_widget(paragraph, area);
}

/// Create a centered rectangle.
fn centered_rect(percent_x: u16, percent_y: u16, r: Rect) -> Rect {
    let popup_layout = ratatui::layout::Layout::default()
        .direction(ratatui::layout::Direction::Vertical)
        .constraints([
            ratatui::layout::Constraint::Percentage((100 - percent_y) / 2),
            ratatui::layout::Constraint::Percentage(percent_y),
            ratatui::layout::Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(r);

    ratatui::layout::Layout::default()
        .direction(ratatui::layout::Direction::Horizontal)
        .constraints([
            ratatui::layout::Constraint::Percentage((100 - percent_x) / 2),
            ratatui::layout::Constraint::Percentage(percent_x),
            ratatui::layout::Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(popup_layout[1])[1]
}
