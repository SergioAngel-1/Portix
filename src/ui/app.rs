use crate::core::{Config, Engine};
use crate::core::query::Query;
use crate::model::{Service, SharedSnapshot, Snapshot};
use anyhow::Result;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

/// Application mode.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Mode {
    Normal,
    Search,
    Filter,
    Help,
    ConfirmKill,
}

/// Events that can occur in the application.
#[derive(Debug)]
pub enum Event {
    /// Terminal tick (for animations)
    Tick,
    /// New snapshot from collector
    Snapshot(SharedSnapshot),
    /// Quit
    Quit,
}

/// The main application state.
pub struct App {
    /// Current mode
    pub mode: Mode,
    /// Current snapshot
    pub snapshot: SharedSnapshot,
    /// Selected index in the table
    pub selected: usize,
    /// Filtered indices
    pub filtered_indices: Vec<usize>,
    /// Search query
    pub query: Query,
    /// Search input buffer
    pub search_buffer: String,
    /// Detail panel open
    pub detail_open: bool,
    /// Config
    pub config: Config,
    /// Whether to show help
    pub show_help: bool,
    /// Scroll offset for detail panel
    pub detail_scroll: u16,
}

impl App {
    /// Create a new application.
    pub fn new(config: Config) -> Self {
        let empty_snapshot: SharedSnapshot = Arc::new(Snapshot {
            services: Vec::new(),
            taken_at: std::time::Instant::now(),
            scan_duration_ms: 0,
            degredations: Vec::new(),
        });

        Self {
            mode: Mode::Normal,
            snapshot: empty_snapshot,
            selected: 0,
            filtered_indices: Vec::new(),
            query: Query::new(),
            search_buffer: String::new(),
            detail_open: false,
            config,
            show_help: false,
            detail_scroll: 0,
        }
    }

    /// Handle a key event.
    pub fn handle_key(&mut self, key: crossterm::event::KeyEvent) -> Vec<Effect> {
        let mut effects = Vec::new();

        match self.mode {
            Mode::Normal => {
                match key.code {
                    crossterm::event::KeyCode::Char('q') => {
                        effects.push(Effect::Quit);
                    }
                    crossterm::event::KeyCode::Char('?') => {
                        self.show_help = !self.show_help;
                    }
                    crossterm::event::KeyCode::Char('/') => {
                        self.mode = Mode::Search;
                        self.search_buffer.clear();
                    }
                    crossterm::event::KeyCode::Char('f') => {
                        self.mode = Mode::Filter;
                    }
                    crossterm::event::KeyCode::Enter | crossterm::event::KeyCode::Char('l') => {
                        self.detail_open = !self.detail_open;
                    }
                    crossterm::event::KeyCode::Up | crossterm::event::KeyCode::Char('k') => {
                        self.move_up();
                    }
                    crossterm::event::KeyCode::Down | crossterm::event::KeyCode::Char('j') => {
                        self.move_down();
                    }
                    crossterm::event::KeyCode::Char('g') => {
                        self.selected = 0;
                    }
                    crossterm::event::KeyCode::Char('G') => {
                        self.selected = self.filtered_indices.len().saturating_sub(1);
                    }
                    crossterm::event::KeyCode::Char('o') => {
                        if let Some(service) = self.selected_service() {
                            effects.push(Effect::OpenBrowser(service.socket.url()));
                        }
                    }
                    crossterm::event::KeyCode::Char('k') => {
                        if let Some(service) = self.selected_service() {
                            if self.config.kill_confirm {
                                self.mode = Mode::ConfirmKill;
                            } else {
                                effects.push(Effect::Kill(service.id.clone(), false));
                            }
                        }
                    }
                    crossterm::event::KeyCode::Char('K') => {
                        if let Some(service) = self.selected_service() {
                            if self.config.kill_confirm {
                                self.mode = Mode::ConfirmKill;
                            } else {
                                effects.push(Effect::Kill(service.id.clone(), true));
                            }
                        }
                    }
                    _ => {}
                }
            }
            Mode::Search => {
                match key.code {
                    crossterm::event::KeyCode::Esc => {
                        self.mode = Mode::Normal;
                        self.search_buffer.clear();
                        self.query.set_query("");
                        self.update_filtered();
                    }
                    crossterm::event::KeyCode::Enter => {
                        self.mode = Mode::Normal;
                        self.query.set_query(&self.search_buffer);
                        self.update_filtered();
                    }
                    crossterm::event::KeyCode::Char(c) => {
                        self.search_buffer.push(c);
                        self.query.set_query(&self.search_buffer);
                        self.update_filtered();
                    }
                    crossterm::event::KeyCode::Backspace => {
                        self.search_buffer.pop();
                        self.query.set_query(&self.search_buffer);
                        self.update_filtered();
                    }
                    _ => {}
                }
            }
            Mode::Help => {
                if key.code == crossterm::event::KeyCode::Esc
                    || key.code == crossterm::event::KeyCode::Char('?')
                {
                    self.show_help = false;
                }
            }
            Mode::ConfirmKill => {
                match key.code {
                    crossterm::event::KeyCode::Esc => {
                        self.mode = Mode::Normal;
                    }
                    crossterm::event::KeyCode::Enter => {
                        if let Some(service) = self.selected_service() {
                            effects.push(Effect::Kill(service.id.clone(), false));
                        }
                        self.mode = Mode::Normal;
                    }
                    _ => {}
                }
            }
            Mode::Filter => {
                // TODO: Implement filter presets
                if key.code == crossterm::event::KeyCode::Esc {
                    self.mode = Mode::Normal;
                }
            }
        }

        effects
    }

    /// Update the filtered indices based on the current query.
    fn update_filtered(&mut self) {
        let results = self.query.filter(&self.snapshot.services);
        self.filtered_indices = results.into_iter().map(|(idx, _)| idx).collect();
        if self.selected >= self.filtered_indices.len() {
            self.selected = self.filtered_indices.len().saturating_sub(1);
        }
    }

    /// Move selection up.
    fn move_up(&mut self) {
        if self.selected > 0 {
            self.selected -= 1;
        }
    }

    /// Move selection down.
    fn move_down(&mut self) {
        if self.selected + 1 < self.filtered_indices.len() {
            self.selected += 1;
        }
    }

    /// Get the currently selected service.
    pub fn selected_service(&self) -> Option<&Service> {
        self.filtered_indices
            .get(self.selected)
            .and_then(|&idx| self.snapshot.services.get(idx))
    }

    /// Update the snapshot.
    pub fn update_snapshot(&mut self, snapshot: SharedSnapshot) {
        self.snapshot = snapshot;
        self.update_filtered();
    }
}

/// Effects that can be triggered by the application.
pub enum Effect {
    Quit,
    OpenBrowser(String),
    Kill(crate::model::ServiceId, bool), // (id, force)
    CopyToClipboard(String),
    Refresh,
}
