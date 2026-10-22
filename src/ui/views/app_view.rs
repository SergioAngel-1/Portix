//! The top-level frame: table, status bar and every overlay, in draw order.

use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::Frame;

use crate::model::Service;
use crate::ui::app::{App, Mode};
use crate::ui::views::{
    confirm_modal, detail_panel, help, main_table, search_bar, status_bar, theme_picker,
};

/// Render the whole application.
///
/// Pure rendering of `App`: the terminal backend and the event loop that owns
/// it live in the binary. Every view is selected by the current mode/overlay
/// state, so a theme change or a snapshot swap is just another call to this.
pub fn render_app(f: &mut Frame, app: &App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(3), Constraint::Length(1)])
        .split(f.area());

    let services: Vec<&Service> = app
        .filtered_indices
        .iter()
        .filter_map(|&idx| app.snapshot.services.get(idx))
        .collect();

    main_table::render(f, chunks[0], &services, app.selected, &app.theme);
    status_bar::render(
        f,
        chunks[1],
        &app.snapshot,
        !app.query.query().is_empty(),
        app.error.as_deref(),
        &app.theme,
    );

    if app.mode == Mode::Search {
        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Min(3),
                Constraint::Length(3),
                Constraint::Length(1),
            ])
            .split(f.area());
        search_bar::render(
            f,
            rows[1],
            &app.search_buffer,
            app.snapshot.services.len(),
            app.filtered_indices.len(),
            &app.theme,
        );
    }

    if app.detail_open {
        detail_panel::render(f, f.area(), app.selected_service(), &app.theme);
    }

    if let Some(signal) = app.pending_kill() {
        confirm_modal::render(
            f,
            f.area(),
            // The freshly re-resolved service (Effect::KillRequest), never the
            // possibly-stale selection.
            app.pending_kill_service(),
            signal,
            &app.theme,
        );
    }

    if app.mode == Mode::ThemePicker {
        theme_picker::render(f, f.area(), app.theme_cursor, &app.theme);
    }

    if app.show_help() {
        help::render(f, f.area(), &app.snapshot.degradations, &app.theme);
    }
}
