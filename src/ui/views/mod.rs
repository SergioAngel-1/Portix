pub mod app_view;
pub mod confirm_modal;
pub mod detail_panel;
pub mod help;
pub mod main_table;
pub mod search_bar;
pub mod status_bar;
pub mod theme_picker;

pub use app_view::render_app;

use ratatui::layout::{Constraint, Direction, Layout, Rect};

/// Centered overlay covering `percent_x` × `percent_y` of `r`. Percentage
/// family: modals whose size should track the terminal (detail panel, help).
pub(crate) fn centered_percent(percent_x: u16, percent_y: u16, r: Rect) -> Rect {
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(r);

    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(vertical[1])[1]
}

/// Centered overlay of a fixed size, clamped to `r`. Fixed family: modals
/// whose height is content-defined (kill confirmation, theme picker).
pub(crate) fn centered_fixed(width: u16, height: u16, r: Rect) -> Rect {
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
