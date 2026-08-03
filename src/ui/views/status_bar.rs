use crate::model::Snapshot;
use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    Frame,
};

/// Key hints shown in the footer, left to right. The pinned `?` entry is never
/// dropped when the terminal is too narrow to show them all: it is the way to
/// reach the full shortcut list, so losing it would strand the user.
const HINTS: &[(&str, &str)] = &[
    ("i", "info"),
    ("x", "kill"),
    ("o", "open"),
    ("y", "copy"),
    ("/", "search"),
    ("?", "help"),
    ("q", "quit"),
];
/// Index into `HINTS` of the entry that must always remain visible.
const PINNED_HINT: usize = 5; // ("?", "help")
const SEP: &str = " · ";

/// Render the status bar at the bottom: key hints on the left, compact stats on
/// the right.
pub fn render(
    f: &mut Frame,
    area: Rect,
    snapshot: &Snapshot,
    search_active: bool,
    error: Option<&str>,
) {
    // An effect failure — notably the TOCTOU guard refusing to signal a recycled
    // PID — takes over the whole bar until the next keypress clears it. It must
    // not be swallowed by hints or stats.
    if let Some(err) = error {
        let line = Line::from(Span::styled(
            format!("⚠ {err}"),
            Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
        ));
        f.render_widget(line, area);
        return;
    }

    let right = stats_spans(snapshot, search_active);
    let right_width = spans_width(&right);

    let area_width = area.width as usize;
    let indices = visible_hints(area_width, right_width);

    // One space of margin from the left edge, then the hints that fit.
    let mut spans = vec![Span::raw(" ")];
    spans.extend(hint_spans(&indices));
    let left_width = spans_width(&spans);

    // Push the stats flush right. If nothing fits, a single space keeps the two
    // clusters from touching and ratatui clips the overflow.
    let padding = area_width.saturating_sub(left_width + right_width);
    spans.push(Span::raw(" ".repeat(padding.max(1))));
    spans.extend(right);

    f.render_widget(Line::from(spans), area);
}

/// Compact right-aligned stats: `12 svc · 3 proj · 8ms`, with container and
/// degradation counts folded in only when non-zero.
fn stats_spans(snapshot: &Snapshot, search_active: bool) -> Vec<Span<'static>> {
    let gray = Style::default().fg(Color::Gray);
    let sep = || Span::styled(SEP, Style::default().fg(Color::DarkGray));

    let services = snapshot.services.len();
    let projects = snapshot
        .services
        .iter()
        .filter(|s| s.project.is_some())
        .count();
    let containers = snapshot
        .services
        .iter()
        .filter(|s| s.container.is_some())
        .count();

    let mut spans = Vec::new();

    // A live filter is worth a glanceable marker, kept next to the counts it
    // narrows.
    if search_active {
        spans.push(Span::styled(
            "/",
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        ));
        spans.push(Span::raw(" "));
    }

    spans.push(Span::styled(format!("{services} svc"), gray));
    spans.push(sep());
    spans.push(Span::styled(format!("{projects} proj"), gray));

    if containers > 0 {
        spans.push(sep());
        spans.push(Span::styled(
            format!("{containers} cont"),
            Style::default().fg(Color::Blue),
        ));
    }

    if !snapshot.degradations.is_empty() {
        spans.push(sep());
        spans.push(Span::styled(
            format!("⚠{}", snapshot.degradations.len()),
            Style::default().fg(Color::Yellow),
        ));
    }

    spans.push(sep());
    spans.push(Span::styled(
        format!("{}ms", snapshot.scan_duration_ms),
        Style::default().fg(Color::DarkGray),
    ));

    spans
}

/// Choose which hints fit the available width, dropping the lowest-priority
/// (rightmost) ones first but never the pinned `?` entry. Returns indices into
/// `HINTS` in display order.
fn visible_hints(area_width: usize, right_width: usize) -> Vec<usize> {
    // Reserve the left margin (1) and at least one space between the two
    // clusters, on top of the stats the right side needs.
    let reserve = right_width + 2;
    let mut indices: Vec<usize> = (0..HINTS.len()).collect();

    while spans_width(&hint_spans(&indices)) + reserve > area_width && indices.len() > 1 {
        match indices.iter().rposition(|&i| i != PINNED_HINT) {
            Some(pos) => {
                indices.remove(pos);
            }
            None => break,
        }
    }

    indices
}

/// Render a set of hint indices as `key label` pairs joined by ` · `.
fn hint_spans(indices: &[usize]) -> Vec<Span<'static>> {
    let key_style = Style::default()
        .fg(Color::Yellow)
        .add_modifier(Modifier::BOLD);
    let label_style = Style::default().fg(Color::Gray);
    let sep_style = Style::default().fg(Color::DarkGray);

    let mut spans = Vec::with_capacity(indices.len() * 3);
    for (i, &idx) in indices.iter().enumerate() {
        if i > 0 {
            spans.push(Span::styled(SEP, sep_style));
        }
        let (key, label) = HINTS[idx];
        spans.push(Span::styled(key, key_style));
        spans.push(Span::styled(format!(" {label}"), label_style));
    }
    spans
}

/// Display width of a span run, counting columns rather than bytes so the
/// multibyte glyphs (`↵`, `·`, `⚠`) do not throw the layout off.
fn spans_width(spans: &[Span]) -> usize {
    spans.iter().map(|s| s.content.chars().count()).sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_wide_terminal_shows_every_hint() {
        assert_eq!(visible_hints(200, 20).len(), HINTS.len());
    }

    #[test]
    fn test_narrow_terminal_drops_from_the_right() {
        // Room for a few hints but not all: the rightmost non-pinned ones go
        // first, so `q quit` is dropped before `↵ details`.
        let indices = visible_hints(40, 20);
        assert!(indices.len() < HINTS.len());
        assert_eq!(indices.first(), Some(&0), "highest-priority hint kept");
        assert!(
            !indices.contains(&(HINTS.len() - 1)),
            "lowest-priority hint dropped"
        );
    }

    /// However tight the bar gets, `?` survives — it is the only path to the
    /// full shortcut list.
    #[test]
    fn test_help_hint_is_never_dropped() {
        for width in [0, 1, 5, 12, 25] {
            assert!(
                visible_hints(width, 30).contains(&PINNED_HINT),
                "? dropped at width {width}"
            );
        }
    }
}
