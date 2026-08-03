use ratatui::style::{Color, Style};

/// Semantic color palette. Views ask for a role (`accent`, `warn`, …) instead of
/// hardcoding a `Color`, so a single theme swap recolors the whole UI — and the
/// monochrome variant makes `--no-color` / `NO_COLOR` actually take effect.
#[derive(Debug, Clone, Copy)]
pub struct Theme {
    pub accent: Color,
    pub ok: Color,
    pub warn: Color,
    pub danger: Color,
    pub container: Color,
    /// Mid gray for field labels and table headers.
    pub label: Color,
    /// Dim gray for de-emphasized text (hints, separators, scan time).
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
            label: Color::Gray,
            muted: Color::DarkGray,
            text: Color::White,
            bg: Color::Black,
        }
    }

    /// Monochrome theme for terminals without color, or `--no-color`. Everything
    /// collapses onto intensity (white / gray / dark gray); callers keep their
    /// bold modifiers, so importance still reads without hue.
    pub fn monochrome() -> Self {
        Self {
            accent: Color::White,
            ok: Color::White,
            warn: Color::Gray,
            danger: Color::White,
            container: Color::Gray,
            label: Color::Gray,
            muted: Color::DarkGray,
            text: Color::White,
            bg: Color::Black,
        }
    }

    /// Create a theme based on terminal capabilities.
    pub fn from_terminal() -> Self {
        if std::env::var("NO_COLOR").is_ok() {
            return Self::monochrome();
        }
        if let Ok(term) = std::env::var("COLORTERM") {
            if term == "truecolor" || term == "24bit" {
                return Self::default_dark();
            }
        }
        if let Ok(term) = std::env::var("TERM") {
            if term.ends_with("-256color") {
                return Self::default_dark();
            }
        }
        Self::default_dark()
    }

    /// Resolve the active theme from the `theme` config value and whether color
    /// is enabled at all. A disabled color pipeline (`--no-color`/`NO_COLOR`)
    /// wins over any named theme.
    pub fn select(name: &str, use_color: bool) -> Self {
        if !use_color {
            return Self::monochrome();
        }
        match name {
            "mono" | "monochrome" | "none" => Self::monochrome(),
            "auto" | "terminal" => Self::from_terminal(),
            _ => Self::default_dark(),
        }
    }

    /// Get style for a specific semantic role.
    pub fn style(&self, role: Role) -> Style {
        Style::default().fg(self.color(role))
    }

    /// The bare color for a role, for callers that add their own modifiers.
    pub fn color(&self, role: Role) -> Color {
        match role {
            Role::Accent => self.accent,
            Role::Ok => self.ok,
            Role::Warn => self.warn,
            Role::Danger => self.danger,
            Role::Container => self.container,
            Role::Label => self.label,
            Role::Muted => self.muted,
            Role::Text => self.text,
        }
    }
}

/// Semantic roles for colors.
#[derive(Debug, Clone, Copy)]
pub enum Role {
    Accent,
    Ok,
    Warn,
    Danger,
    Container,
    Label,
    Muted,
    Text,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_named_theme_selection() {
        assert_eq!(Theme::select("dark", true).accent, Color::Cyan);
        assert_eq!(Theme::select("mono", true).accent, Color::White);
        // An unknown name falls back to the default dark theme, not a panic.
        assert_eq!(Theme::select("weird", true).accent, Color::Cyan);
    }

    /// `--no-color` / `NO_COLOR` collapses to monochrome regardless of the named
    /// theme — this is the fix that makes the flag actually do something.
    #[test]
    fn test_no_color_forces_monochrome_over_any_name() {
        assert_eq!(Theme::select("dark", false).accent, Theme::monochrome().accent);
        assert_eq!(Theme::select("auto", false).warn, Theme::monochrome().warn);
    }
}
