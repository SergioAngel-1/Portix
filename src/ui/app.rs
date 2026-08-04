use crate::core::query::Query;
use crate::core::Config;
use crate::model::{Service, SharedSnapshot, Signal, Snapshot};
use crate::ui::theme::Theme;
use crossterm::event::{KeyCode, KeyEvent};
use std::sync::Arc;

/// Application mode.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Mode {
    Normal,
    Search,
    Help,
    /// Waiting for the user to confirm a kill, remembering which signal.
    ConfirmKill(Signal),
    /// Browsing themes with live preview; Enter saves, Esc reverts.
    ThemePicker,
}

/// The main application state.
pub struct App {
    pub mode: Mode,
    pub snapshot: SharedSnapshot,
    pub selected: usize,
    pub filtered_indices: Vec<usize>,
    pub query: Query,
    pub search_buffer: String,
    pub detail_open: bool,
    pub config: Config,
    /// Active color palette, resolved from config + the `--no-color` flag.
    pub theme: Theme,
    /// Whether color is enabled at all; a live theme switch respects it.
    pub color_enabled: bool,
    /// Name of the active theme, kept in sync with `config.theme`.
    pub theme_name: String,
    /// Cursor within `Theme::NAMES` while the picker is open.
    pub theme_cursor: usize,
    /// Theme (name + palette) to restore if the picker is cancelled.
    theme_saved: Option<(String, Theme)>,
    /// Set when an effect failed, shown in the status bar until the next action.
    pub error: Option<String>,
}

impl App {
    /// Create a new application.
    pub fn new(config: Config, theme: Theme, color_enabled: bool) -> Self {
        let theme_name = config.theme.clone();
        Self {
            mode: Mode::Normal,
            snapshot: Arc::new(Snapshot {
                services: Vec::new(),
                taken_at: std::time::Instant::now(),
                taken_at_epoch_secs: 0,
                scan_duration_ms: 0,
                degradations: Vec::new(),
            }),
            selected: 0,
            filtered_indices: Vec::new(),
            query: Query::new(),
            search_buffer: String::new(),
            detail_open: false,
            config,
            theme,
            color_enabled,
            theme_name,
            theme_cursor: 0,
            theme_saved: None,
            error: None,
        }
    }

    pub fn show_help(&self) -> bool {
        self.mode == Mode::Help
    }

    pub fn pending_kill(&self) -> Option<Signal> {
        match self.mode {
            Mode::ConfirmKill(sig) => Some(sig),
            _ => None,
        }
    }

    /// Handle a key event: pure state transition, returns effects to run.
    ///
    /// Note the keymap: `j`/`k` navigate (vim), so killing is bound to `x` and
    /// `X`. Overloading `k` for both "up" and "kill" is not a keymap, it is a
    /// trap — and it made the kill binding unreachable code.
    pub fn handle_key(&mut self, key: KeyEvent) -> Vec<Effect> {
        let mut effects = Vec::new();
        self.error = None;

        match self.mode {
            Mode::Normal => match key.code {
                KeyCode::Char('q') => effects.push(Effect::Quit),
                KeyCode::Char('?') => self.mode = Mode::Help,
                KeyCode::Char('/') => {
                    self.mode = Mode::Search;
                    self.search_buffer.clear();
                }
                KeyCode::Esc => {
                    if self.detail_open {
                        self.detail_open = false;
                    } else if !self.query.query().is_empty() {
                        self.set_query("");
                    }
                }
                KeyCode::Char('i') | KeyCode::Enter | KeyCode::Char('l') => {
                    self.detail_open = !self.detail_open
                }
                KeyCode::Char('h') => self.detail_open = false,
                KeyCode::Up | KeyCode::Char('k') => self.move_up(),
                KeyCode::Down | KeyCode::Char('j') => self.move_down(),
                KeyCode::Char('g') => self.selected = 0,
                KeyCode::Char('G') => self.selected = self.filtered_indices.len().saturating_sub(1),
                KeyCode::Char('r') => effects.push(Effect::Refresh),
                KeyCode::Char('o') => {
                    if let Some(service) = self.selected_service() {
                        effects.push(Effect::OpenBrowser(service.socket.url()));
                    }
                }
                KeyCode::Char('y') => {
                    if let Some(service) = self.selected_service() {
                        effects.push(Effect::CopyToClipboard(service.socket.url()));
                    }
                }
                KeyCode::Char('x') => self.request_kill(Signal::Term, &mut effects),
                KeyCode::Char('X') => self.request_kill(Signal::Kill, &mut effects),
                KeyCode::Char('t') => self.open_theme_picker(),
                _ => {}
            },

            Mode::Search => match key.code {
                KeyCode::Esc => {
                    self.mode = Mode::Normal;
                    self.search_buffer.clear();
                    self.set_query("");
                }
                KeyCode::Enter => self.mode = Mode::Normal,
                KeyCode::Char(c) => {
                    self.search_buffer.push(c);
                    let q = self.search_buffer.clone();
                    self.set_query(&q);
                }
                KeyCode::Backspace => {
                    self.search_buffer.pop();
                    let q = self.search_buffer.clone();
                    self.set_query(&q);
                }
                _ => {}
            },

            Mode::Help => {
                if matches!(
                    key.code,
                    KeyCode::Esc | KeyCode::Char('?') | KeyCode::Char('q')
                ) {
                    self.mode = Mode::Normal;
                }
            }

            Mode::ConfirmKill(signal) => match key.code {
                KeyCode::Esc | KeyCode::Char('n') => self.mode = Mode::Normal,
                KeyCode::Enter | KeyCode::Char('y') => {
                    if let Some(service) = self.selected_service() {
                        effects.push(Effect::Kill(service.socket.port, signal));
                    }
                    self.mode = Mode::Normal;
                }
                _ => {}
            },

            Mode::ThemePicker => match key.code {
                KeyCode::Up | KeyCode::Char('k') => {
                    self.theme_cursor = self.theme_cursor.saturating_sub(1);
                    self.apply_theme_at_cursor();
                }
                KeyCode::Down | KeyCode::Char('j') => {
                    if self.theme_cursor + 1 < Theme::NAMES.len() {
                        self.theme_cursor += 1;
                    }
                    self.apply_theme_at_cursor();
                }
                // Tapping `t` cycles, so the same key that opened the picker also
                // walks through the options.
                KeyCode::Char('t') => {
                    self.theme_cursor = (self.theme_cursor + 1) % Theme::NAMES.len();
                    self.apply_theme_at_cursor();
                }
                KeyCode::Enter => {
                    self.theme_saved = None;
                    self.mode = Mode::Normal;
                    effects.push(Effect::PersistConfig);
                }
                KeyCode::Esc => {
                    if let Some((name, theme)) = self.theme_saved.take() {
                        self.theme = theme;
                        self.config.theme = name.clone();
                        self.theme_name = name;
                    }
                    self.mode = Mode::Normal;
                }
                _ => {}
            },
        }

        effects
    }

    /// Open the theme picker, remembering the current theme so Esc can revert.
    fn open_theme_picker(&mut self) {
        self.theme_saved = Some((self.theme_name.clone(), self.theme));
        self.theme_cursor = Theme::name_index(&self.theme_name);
        self.mode = Mode::ThemePicker;
    }

    /// Apply the theme under the picker cursor immediately, for live preview.
    fn apply_theme_at_cursor(&mut self) {
        let name = Theme::NAMES[self.theme_cursor];
        self.theme = Theme::select(name, self.color_enabled);
        self.theme_name = name.to_string();
        self.config.theme = self.theme_name.clone();
    }

    fn request_kill(&mut self, signal: Signal, effects: &mut Vec<Effect>) {
        let Some(service) = self.selected_service() else {
            return;
        };
        if self.config.kill_confirm {
            self.mode = Mode::ConfirmKill(signal);
        } else {
            effects.push(Effect::Kill(service.socket.port, signal));
        }
    }

    fn set_query(&mut self, query: &str) {
        self.query.set_query(query);
        self.update_filtered();
    }

    /// Update the filtered indices based on the current query.
    fn update_filtered(&mut self) {
        let results = self.query.filter(&self.snapshot.services);
        self.filtered_indices = results.into_iter().map(|(idx, _)| idx).collect();
        self.clamp_selection();
    }

    fn clamp_selection(&mut self) {
        let max = self.filtered_indices.len().saturating_sub(1);
        if self.selected > max {
            self.selected = max;
        }
    }

    fn move_up(&mut self) {
        self.selected = self.selected.saturating_sub(1);
    }

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

    pub fn update_snapshot(&mut self, snapshot: SharedSnapshot) {
        self.snapshot = snapshot;
        self.update_filtered();
    }
}

/// Effects that can be triggered by the application.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Effect {
    Quit,
    OpenBrowser(String),
    /// Kill the service currently listening on `port`.
    Kill(u16, Signal),
    CopyToClipboard(String),
    Refresh,
    /// Persist the current config (theme) to disk.
    PersistConfig,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{BindScope, Process, ProcessState, Protocol, ServiceId, Socket, TcpState};

    fn key(c: char) -> KeyEvent {
        KeyEvent::from(KeyCode::Char(c))
    }

    fn app_with_services(count: usize) -> App {
        let services: Vec<Service> = (0..count)
            .map(|i| {
                let socket = Socket {
                    protocol: Protocol::Tcp,
                    local_addr: std::net::Ipv4Addr::LOCALHOST.into(),
                    port: 3000 + i as u16,
                    state: TcpState::Listen,
                    bind_scope: BindScope::Loopback,
                    inode: i as u64,
                    uid: 1000,
                    dual_stack: false,
                };
                Service {
                    id: ServiceId::from_socket(&socket),
                    socket,
                    process: Some(Process {
                        pid: 100 + i as i32,
                        ppid: 1,
                        name: "node".into(),
                        cmdline: vec!["node".into()],
                        exe: None,
                        cwd: None,
                        user: None,
                        started_at: None,
                        starttime_jiffies: 7,
                        cpu_jiffies: 0,
                        metrics: Default::default(),
                        state: ProcessState::Sleeping,
                        threads: 1,
                    }),
                    project: None,
                    framework: None,
                    container: None,
                    docker_proxy_rewritten: false,
                }
            })
            .collect();

        let mut app = App::new(Config::default(), Theme::default_dark(), true);
        app.update_snapshot(Arc::new(Snapshot {
            services,
            taken_at: std::time::Instant::now(),
            taken_at_epoch_secs: 0,
            scan_duration_ms: 1,
            degradations: Vec::new(),
        }));
        app
    }

    #[test]
    fn test_q_quits() {
        let mut app = app_with_services(1);
        assert_eq!(app.handle_key(key('q')), vec![Effect::Quit]);
    }

    /// `k` moves up. It used to shadow the kill binding entirely.
    #[test]
    fn test_k_navigates_and_does_not_kill() {
        let mut app = app_with_services(3);
        app.selected = 2;
        assert!(app.handle_key(key('k')).is_empty());
        assert_eq!(app.selected, 1);
        assert_eq!(app.mode, Mode::Normal);
    }

    #[test]
    fn test_x_requests_confirmation_then_sends_sigterm() {
        let mut app = app_with_services(1);
        assert!(app.handle_key(key('x')).is_empty());
        assert_eq!(app.mode, Mode::ConfirmKill(Signal::Term));

        let effects = app.handle_key(KeyEvent::from(KeyCode::Enter));
        assert!(matches!(effects[0], Effect::Kill(_, Signal::Term)));
        assert_eq!(app.mode, Mode::Normal);
    }

    /// Confirming a force-kill must still force-kill, not silently downgrade.
    #[test]
    fn test_capital_x_confirms_to_sigkill() {
        let mut app = app_with_services(1);
        app.handle_key(key('X'));
        assert_eq!(app.mode, Mode::ConfirmKill(Signal::Kill));

        let effects = app.handle_key(KeyEvent::from(KeyCode::Enter));
        assert!(matches!(effects[0], Effect::Kill(_, Signal::Kill)));
    }

    #[test]
    fn test_kill_confirmation_can_be_cancelled() {
        let mut app = app_with_services(1);
        app.handle_key(key('x'));
        let effects = app.handle_key(KeyEvent::from(KeyCode::Esc));
        assert!(effects.is_empty());
        assert_eq!(app.mode, Mode::Normal);
    }

    #[test]
    fn test_kill_without_confirm_is_immediate() {
        let mut app = app_with_services(1);
        app.config.kill_confirm = false;
        let effects = app.handle_key(key('x'));
        assert!(matches!(effects[0], Effect::Kill(_, Signal::Term)));
    }

    #[test]
    fn test_i_toggles_the_info_panel() {
        let mut app = app_with_services(1);
        assert!(!app.detail_open);
        assert!(app.handle_key(key('i')).is_empty());
        assert!(app.detail_open, "i opens the info panel");
        app.handle_key(key('i'));
        assert!(!app.detail_open, "i again closes it");
    }

    #[test]
    fn test_esc_closes_the_info_panel_before_clearing_search() {
        let mut app = app_with_services(1);
        app.handle_key(key('i'));
        assert!(app.detail_open);
        app.handle_key(KeyEvent::from(KeyCode::Esc));
        assert!(!app.detail_open, "Esc closes the panel first");
    }

    #[test]
    fn test_t_opens_picker_and_enter_saves() {
        let mut app = app_with_services(1);
        app.handle_key(key('t'));
        assert_eq!(app.mode, Mode::ThemePicker);

        // Cycle to the next theme; the palette must actually change.
        let before = app.theme_name.clone();
        app.handle_key(key('t'));
        assert_ne!(app.theme_name, before, "tapping t previews the next theme");

        let effects = app.handle_key(KeyEvent::from(KeyCode::Enter));
        assert!(effects.contains(&Effect::PersistConfig), "Enter persists");
        assert_eq!(app.mode, Mode::Normal);
    }

    #[test]
    fn test_esc_reverts_theme_preview() {
        let mut app = app_with_services(1);
        let original = app.theme_name.clone();
        app.handle_key(key('t'));
        app.handle_key(key('j')); // preview a different theme
        assert_ne!(app.theme_name, original);
        app.handle_key(KeyEvent::from(KeyCode::Esc));
        assert_eq!(app.theme_name, original, "Esc reverts the preview");
        assert_eq!(app.config.theme, original);
        assert_eq!(app.mode, Mode::Normal);
    }

    #[test]
    fn test_help_mode_is_reachable_and_dismissable() {
        let mut app = app_with_services(1);
        app.handle_key(key('?'));
        assert!(app.show_help());
        app.handle_key(KeyEvent::from(KeyCode::Esc));
        assert!(!app.show_help());
    }

    #[test]
    fn test_search_narrows_and_clamps_selection() {
        let mut app = app_with_services(3);
        app.selected = 2;

        app.handle_key(key('/'));
        assert_eq!(app.mode, Mode::Search);
        app.handle_key(key('3'));
        app.handle_key(key('0'));
        app.handle_key(key('0'));
        app.handle_key(key('2'));

        assert_eq!(app.filtered_indices.len(), 1);
        assert_eq!(app.selected, 0, "selection must not dangle past the filter");
    }

    #[test]
    fn test_selection_survives_a_shrinking_snapshot() {
        let mut app = app_with_services(3);
        app.selected = 2;
        app.update_snapshot(Arc::new(Snapshot {
            services: Vec::new(),
            taken_at: std::time::Instant::now(),
            taken_at_epoch_secs: 0,
            scan_duration_ms: 1,
            degradations: Vec::new(),
        }));
        assert_eq!(app.selected, 0);
        assert!(app.selected_service().is_none());
    }
}
