use ratatui::style::{Color, Style};

/// Semantic color palette.
pub struct Theme {
    pub accent: Color,
    pub ok: Color,
    pub warn: Color,
    pub danger: Color,
    pub container: Color,
    pub muted: Color,
    pub text: Color,
    pub bg: Color,
}

impl Theme {
    /// Create the default dark theme.
    pub fn default_dark() -> Self {
        Self {
            accent: Color::Cyan,
            ok: Color::Green,
            warn: Color::Yellow,
            danger: Color::Red,
            container: Color::Blue,
            muted: Color::DarkGray,
            text: Color::White,
            bg: Color::Black,
        }
    }

    /// Create a theme based on terminal capabilities.
    pub fn from_terminal() -> Self {
        // Check for NO_COLOR
        if std::env::var("NO_COLOR").is_ok() {
            return Self::monochrome();
        }

        // Check for truecolor support
        if let Ok(term) = std::env::var("COLORTERM") {
            if term == "truecolor" || term == "24bit" {
                return Self::default_dark();
            }
        }

        // Check for 256 color
        if let Ok(term) = std::env::var("TERM") {
            if term.ends_with("-256color") {
                return Self::default_dark();
            }
        }

        // Fall back to 16 colors
        Self::default_dark()
    }

    /// Monochrome theme for terminals without color support.
    pub fn monochrome() -> Self {
        Self {
            accent: Color::White,
            ok: Color::White,
            warn: Color::Gray,
            danger: Color::Gray,
            container: Color::Gray,
            muted: Color::DarkGray,
            text: Color::White,
            bg: Color::Black,
        }
    }

    /// Get style for a specific semantic role.
    pub fn style(&self, role: Role) -> Style {
        match role {
            Role::Accent => Style::default().fg(self.accent),
            Role::Ok => Style::default().fg(self.ok),
            Role::Warn => Style::default().fg(self.warn),
            Role::Danger => Style::default().fg(self.danger),
            Role::Container => Style::default().fg(self.container),
            Role::Muted => Style::default().fg(self.muted),
            Role::Text => Style::default().fg(self.text),
        }
    }
}

/// Semantic roles for colors.
pub enum Role {
    Accent,
    Ok,
    Warn,
    Danger,
    Container,
    Muted,
    Text,
}
