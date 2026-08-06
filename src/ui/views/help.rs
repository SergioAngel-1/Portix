use crate::model::Degradation;
use crate::ui::format::cell;
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
    let area = super::centered_percent(60, 80, area);

    let header = Style::default()
        .fg(theme.label)
        .add_modifier(Modifier::BOLD);

    let mut help_text = vec![
        Line::from(Span::styled(
            "Portix · Keyboard Shortcuts",
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD),
        )),
        Line::raw(""),
        Line::from(Span::styled("NAVIGATION", header)),
        Line::raw("  ↑ k        up              g   first"),
        Line::raw("  ↓ j        down            G   last"),
        Line::raw("  i Enter l  info            h   close"),
        Line::raw(""),
        Line::from(Span::styled("SEARCH & FILTER", header)),
        Line::raw("  /          search          Esc cancel"),
        Line::raw("  scope:...  local, network, all, address"),
        Line::raw(""),
        Line::from(Span::styled("ACTIONS", header)),
        Line::raw("  x          kill (SIGTERM)"),
        Line::raw("  X          force kill (SIGKILL)"),
        Line::raw("  o          open in browser"),
        Line::raw("  y          copy URL"),
        Line::raw(""),
        Line::from(Span::styled("GENERAL", header)),
        Line::raw("  r          refresh         q   quit"),
        Line::raw("  t          change theme    ?   this help"),
    ];

    // The warnings section only appears when the scan actually degraded, so an
    // empty run stays uncluttered. Messages can embed system-derived text
    // (error paths), so they pass through the `cell` chokepoint.
    if !degradations.is_empty() {
        help_text.push(Line::raw(""));
        help_text.push(Line::from(Span::styled("WARNINGS", header)));
        for degradation in degradations {
            help_text.push(Line::from(vec![
                Span::styled("  ⚠ ", Style::default().fg(theme.warn)),
                Span::styled(
                    cell(&degradation.message()),
                    Style::default().fg(theme.warn),
                ),
            ]));
        }
    }

    help_text.push(Line::raw(""));
    help_text.push(Line::raw("              Esc or ? to close"));

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
