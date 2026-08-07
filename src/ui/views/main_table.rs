use crate::model::Service;
use crate::ui::format::{cell, format_bytes_compact, format_uptime, truncate_center};
use crate::ui::theme::Theme;
use ratatui::{
    layout::{Constraint, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Cell, Row, Table, TableState},
    Frame,
};

const DASH: &str = "—";

/// Render the main service table.
pub fn render(f: &mut Frame, area: Rect, services: &[&Service], selected: usize, theme: &Theme) {
    let header = Row::new(vec![
        "PORT",
        "PROTO",
        "SCOPE",
        "PROJECT",
        "FRAMEWORK",
        "COMMAND",
        "PID",
        "CPU",
        "MEM",
        "UPTIME",
    ])
    .style(
        Style::default()
            .fg(theme.label)
            .add_modifier(Modifier::BOLD),
    );

    // Compute column widths once; the framework column width drives the
    // icon-vs-name decision in the row builder.
    let content_width = area.width.saturating_sub(9); // Table spacing
    let widths = column_widths(content_width);
    let fw_col_width = match widths[4] {
        Constraint::Length(n) => n,
        _ => 16,
    };

    let rows: Vec<Row> = services
        .iter()
        .enumerate()
        .map(|(idx, service)| {
            let process = service.process.as_ref();

            // Every one of these strings can carry attacker-controlled bytes
            // (project names, argv). `cell` is the sanitizing chokepoint.

            // PORT — accent color to make it the primary visual anchor.
            let port = Cell::from(Span::styled(
                format!("{:>5}", service.socket.port),
                Style::default()
                    .fg(theme.accent)
                    .add_modifier(Modifier::BOLD),
            ));

            // PROTO — muted; protocol is secondary context.
            let proto = Cell::from(Span::styled(
                service.socket.proto_display(),
                Style::default().fg(theme.muted),
            ));

            // SCOPE — icon + color by bind scope.
            let (scope_icon, scope_color) = match service.socket.bind_scope {
                crate::model::BindScope::Loopback => ("● ", theme.ok),
                crate::model::BindScope::AllInterfaces => ("○ ", theme.warn),
                crate::model::BindScope::Specific(_) => ("◆ ", theme.warn),
            };
            let scope = Cell::from(Line::from(vec![
                Span::styled(scope_icon, Style::default().fg(scope_color)),
                Span::styled(
                    service.socket.scope_label(),
                    Style::default().fg(scope_color).add_modifier(
                        if service.socket.is_network_bound() {
                            Modifier::BOLD
                        } else {
                            Modifier::empty()
                        },
                    ),
                ),
            ]));

            // PROJECT — text normally, container color when in a container.
            let project_text = cell(&truncate_center(&service.project_display(), 20));
            let project_fg = theme.text;
            let project = Cell::from(Span::styled(project_text, Style::default().fg(project_fg)));

            // FRAMEWORK — tech icon + name, colored by confidence level.
            // Responsive: when the column is too narrow for icon+name, show
            // the icon alone (2 cols). The icon is recognizable on its own.
            // When the column is < 3 cols, ratatui clips whatever we render.
            let (framework_text, framework_fg) = if let Some(runtime) = service.runtime_display() {
                (
                    cell(&truncate_center(&runtime, fw_col_width as usize)),
                    theme.container,
                )
            } else if let Some(fw) = &service.framework {
                let icon = fw.tech_icon().unwrap_or("");
                let icon_width = unicode_width::UnicodeWidthStr::width(icon);
                let name_max = (fw_col_width as usize).saturating_sub(icon_width);
                let color = match fw.confidence {
                    crate::model::FrameworkConfidence::High => theme.ok,
                    crate::model::FrameworkConfidence::Medium => theme.warn,
                    crate::model::FrameworkConfidence::Low => theme.muted,
                };
                if name_max >= 3 {
                    // Enough room for icon + at least a 3-char name.
                    let name = cell(&truncate_center(&fw.display(), name_max));
                    (format!("{icon}{name}"), color)
                } else if icon_width > 0 && fw_col_width as usize >= icon_width {
                    // Too narrow for a name — show icon only.
                    (icon.to_string(), color)
                } else {
                    // Very narrow: just the name, truncated.
                    (
                        cell(&truncate_center(&fw.display(), fw_col_width as usize)),
                        color,
                    )
                }
            } else {
                (
                    cell(&truncate_center(
                        &service.framework_display(),
                        fw_col_width as usize,
                    )),
                    theme.muted,
                )
            };
            let framework = Cell::from(Line::from(vec![Span::styled(
                framework_text,
                Style::default().fg(framework_fg),
            )]));

            // COMMAND — text color.
            let command = Cell::from(Span::styled(
                cell(&truncate_center(&service.command_display(), 16)),
                Style::default().fg(theme.text),
            ));

            // PID — muted; it's a detail, not the primary identifier.
            let pid = Cell::from(Span::styled(
                process
                    .map(|p| format!("{:>6}", p.pid))
                    .unwrap_or_else(|| format!("{:>6}", DASH)),
                Style::default().fg(theme.muted),
            ));

            // CPU — threshold coloring: danger >90%, warn >50%, muted otherwise.
            let cpu_val = process.and_then(|p| p.metrics.cpu_percent);
            let cpu_str = cpu_val
                .map(|c| format!("{:>5.1}%", c))
                .unwrap_or_else(|| format!("{:>6}", DASH));
            let cpu_style = match cpu_val {
                Some(c) if c > 90.0 => Style::default()
                    .fg(theme.danger)
                    .add_modifier(Modifier::BOLD),
                Some(c) if c > 50.0 => Style::default().fg(theme.warn),
                _ => Style::default().fg(theme.muted),
            };
            let cpu = Cell::from(Span::styled(cpu_str, cpu_style));

            // MEM — muted.
            let mem = Cell::from(Span::styled(
                process
                    .and_then(|p| p.metrics.rss_bytes)
                    .map(format_bytes_compact)
                    .unwrap_or_else(|| DASH.to_string()),
                Style::default().fg(theme.muted),
            ));

            // UPTIME — muted.
            let uptime = Cell::from(Span::styled(
                process
                    .and_then(|p| p.metrics.uptime)
                    .map(format_uptime)
                    .unwrap_or_else(|| DASH.to_string()),
                Style::default().fg(theme.muted),
            ));

            // Zebra striping: even rows get a subtle background.
            let row_bg = if idx % 2 == 0 {
                Style::default()
            } else {
                Style::default().bg(theme.alt_row)
            };

            Row::new(vec![
                port, proto, scope, project, framework, command, pid, cpu, mem, uptime,
            ])
            .style(row_bg)
        })
        .collect();

    // Column widths calculated dynamically (see column_widths fn).
    // The highlight symbol ("▸ ") is inside the first column's area, not extra.

    let table = Table::new(rows, widths)
        .header(header)
        .row_highlight_style(
            Style::default()
                .bg(theme.selection)
                .fg(theme.highlight)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("▸ ");

    let mut state = TableState::default();
    if !services.is_empty() {
        state.select(Some(selected));
    }

    f.render_stateful_widget(table, area, &mut state);
}

/// Calculate column widths dynamically based on available content width.
///
/// `content_width` = terminal width minus Table overhead (column_spacing × 9 gaps).
/// The highlight symbol ("▸ ") is inside the first column's area, not extra.
///
/// Fixed columns (PORT, PROTO, SCOPE) always get their declared space.
/// Detail columns (FRAMEWORK..UPTIME) are capped at their max useful width.
/// PROJECT (the flex column) absorbs all remaining space.
///
/// This replaces the static `[Constraint; 10]` that starved right-side columns
/// on narrow terminals — ratatui's `Length` + `Max` both allocate greedily, so
/// a static array with `Length` first always short-changed the last columns.
fn column_widths(content_width: u16) -> [Constraint; 10] {
    // Fixed: PORT(7) + PROTO(5) + SCOPE(9) = 21
    const FIXED: u16 = 21;

    // Max useful widths for the detail columns.
    const FW_MAX: u16 = 18; // "▲ Next.js 15.1.0" / "⚡ FastAPI 0.110"
    const CMD_MAX: u16 = 16; // binary names
    const PID_MAX: u16 = 7; // up to 9999999
    const CPU_MAX: u16 = 7; // "100.0%"
    const MEM_MAX: u16 = 6; // "2.0G"
    const UPT_MAX: u16 = 7; // "73h51m"
    const DETAIL_TOTAL: u16 = FW_MAX + CMD_MAX + PID_MAX + CPU_MAX + MEM_MAX + UPT_MAX; // 59

    let available = content_width.saturating_sub(FIXED);
    let project_min: u16 = 12;

    // If the terminal is wide enough, detail columns get their max and PROJECT
    // gets the rest. On narrow terminals (< FIXED + DETAIL_TOTAL + project_min),
    // detail columns shrink proportionally.
    let (fw, cmd, pid, cpu, mem, upt, proj) = if available >= DETAIL_TOTAL + project_min {
        // Wide: detail columns at max, PROJECT gets the surplus.
        (
            FW_MAX,
            CMD_MAX,
            PID_MAX,
            CPU_MAX,
            MEM_MAX,
            UPT_MAX,
            available - DETAIL_TOTAL,
        )
    } else {
        // Narrow: distribute available space between detail columns and PROJECT.
        let for_detail = available.saturating_sub(project_min);
        let scale = |max: u16| -> u16 {
            // Proportional shrink, at least 1.
            ((max as u32 * for_detail as u32) / DETAIL_TOTAL as u32).max(1) as u16
        };
        let fw = scale(FW_MAX);
        let cmd = scale(CMD_MAX);
        let pid = scale(PID_MAX);
        let cpu = scale(CPU_MAX);
        let mem = scale(MEM_MAX);
        let upt = scale(UPT_MAX);
        let used = fw + cmd + pid + cpu + mem + upt;
        let proj = available.saturating_sub(used).max(project_min);
        (fw, cmd, pid, cpu, mem, upt, proj)
    };

    [
        Constraint::Length(7),    // PORT: 5-digit port + highlight symbol
        Constraint::Length(5),    // PROTO: "tcp6" / "udp6"
        Constraint::Length(9),    // SCOPE: "● local" / "● address"
        Constraint::Length(proj), // PROJECT: flex — computed dynamically
        Constraint::Length(fw),   // FRAMEWORK
        Constraint::Length(cmd),  // COMMAND
        Constraint::Length(pid),  // PID
        Constraint::Length(cpu),  // CPU
        Constraint::Length(mem),  // MEM
        Constraint::Length(upt),  // UPTIME
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::layout::{Direction, Layout};

    /// Verify that column constraints produce viable widths at every realistic
    /// terminal width. Each column must get at least 1 cell; PROJECT (the flex
    /// column) must get at least 12.
    ///
    /// Note: this tests the raw constraints through Layout (no Table spacing).
    /// The real Table adds 9 cols of spacing, so actual column widths at 80 cols
    /// are slightly smaller. The render() function subtracts spacing before
    /// calling column_widths(), so both paths are consistent.
    #[test]
    fn test_responsive_column_widths() {
        for term_width in [40, 60, 70, 80, 100, 120, 140, 160] {
            let area = Rect::new(0, 0, term_width, 1);
            let widths = column_widths(term_width);
            let chunks = Layout::default()
                .direction(Direction::Horizontal)
                .constraints(widths)
                .split(area);

            let names = [
                "PORT", "PROTO", "SCOPE", "PROJECT", "FRAMEW", "COMAND", "PID", "CPU", "MEM",
                "UPTIME",
            ];

            let total: u16 = chunks.iter().map(|c| c.width).sum();
            assert!(
                total <= term_width,
                "at {term_width} cols: total {total} exceeds terminal width"
            );

            // Every column must get at least 1 cell.
            for (i, chunk) in chunks.iter().enumerate() {
                assert!(
                    chunk.width >= 1,
                    "at {term_width} cols: {} got 0 width",
                    names[i]
                );
            }

            // PROJECT (index 3) must get at least 12.
            let project_width = chunks[3].width;
            assert!(
                project_width >= 12,
                "at {term_width} cols: PROJECT got only {project_width} (need 12)"
            );

            // At 100+ cols, FRAMEW and COMMAND should reach their max.
            if term_width >= 100 {
                assert!(
                    chunks[4].width >= 14,
                    "at {term_width}: FRAMEW={} too narrow (need 14)",
                    chunks[4].width
                );
                assert!(
                    chunks[5].width >= 14,
                    "at {term_width}: COMMAND={} too narrow (need 14)",
                    chunks[5].width
                );
            }

            // At 80 cols, FRAMEW must be at least 10 (tight but usable).
            if term_width >= 80 {
                assert!(
                    chunks[4].width >= 10,
                    "at {term_width}: FRAMEW={} too narrow (need 10)",
                    chunks[4].width
                );
            }

            // Print for visual inspection.
            let mut parts = Vec::new();
            for (i, chunk) in chunks.iter().enumerate() {
                parts.push(format!("{}={}", names[i], chunk.width));
            }
            eprintln!(
                "  width={term_width:>3}: {} | project={project_width} | total={total}",
                parts.join(" ")
            );
        }
    }
}
