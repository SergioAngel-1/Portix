use serde::Serialize;

/// Programming language.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum Language {
    Node,
    Python,
    Go,
    Rust,
    Php,
    Ruby,
    Java,
    Elixir,
    Dart,
    Unknown,
}

/// Confidence level for framework detection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub enum FrameworkConfidence {
    Low,
    Medium,
    High,
}

impl FrameworkConfidence {
    pub fn display(&self) -> &str {
        match self {
            FrameworkConfidence::Low => "Low",
            FrameworkConfidence::Medium => "Medium",
            FrameworkConfidence::High => "High",
        }
    }

    pub fn glyph(&self) -> &str {
        match self {
            FrameworkConfidence::High => "◆",
            FrameworkConfidence::Medium => "◇",
            FrameworkConfidence::Low => "·",
        }
    }

    pub fn suffix(&self) -> &str {
        match self {
            FrameworkConfidence::High => "",
            FrameworkConfidence::Medium => "~",
            FrameworkConfidence::Low => "?",
        }
    }
}

/// A detected framework.
#[derive(Debug, Clone, Serialize)]
pub struct Framework {
    /// Unique identifier (e.g., "nextjs", "fastapi")
    pub id: String,
    /// Display name (e.g., "Next.js", "FastAPI")
    pub display_name: String,
    /// Version if detected (e.g., "15.1.0")
    pub version: Option<String>,
    /// Programming language
    pub language: Language,
    /// Confidence level
    pub confidence: FrameworkConfidence,
    /// Evidence that led to this detection
    pub matched_signals: Vec<String>,
}

impl Framework {
    /// A technology-specific icon for this framework. Used in the table and
    /// detail panel to make frameworks instantly recognizable at a glance.
    ///
    /// Returns `None` for runtimes that have no widely-recognized icon (e.g.,
    /// `wsgi`, `node` as a bare runtime). The icon is a double-width emoji
    /// rendered as a separate `Span` in the table — it is not embedded in
    /// `display()` so the display string stays clean for truncation.
    pub fn tech_icon(&self) -> Option<&'static str> {
        match self.id.as_str() {
            "nextjs" => Some("▲ "),
            "vite" => Some("⚡ "),
            "astro" => Some("🚀 "),
            "nuxt" => Some("💚 "),
            "remix" => Some("💿 "),
            "nestjs" => Some("🐱 "),
            "express" => Some("🚂 "),
            "react" => Some("⚛ "),
            "fastapi" => Some("⚡ "),
            "django" => Some("🌿 "),
            "flask" => Some("🧪 "),
            "uvicorn" => Some("🦄 "),
            "gunicorn" => Some("🦄 "),
            "go" => Some("🐹 "),
            "rust" => Some("🦀 "),
            "python" => Some("🐍 "),
            _ => None,
        }
    }

    /// Clean display name for the framework — no icon, no confidence suffix.
    ///
    /// The icon is rendered as a separate styled `Span` by the table/detail
    /// views. The confidence suffix was removed because glyphs like `·` or `?`
    /// appended to the name were confused for technology indicators; confidence
    /// is now communicated through color (ok / warn / muted) instead.
    ///
    /// `fw.version` is read from `package.json` dependencies — attacker-controlled
    /// text that passes through `format::cell` in the renderers.
    pub fn display(&self) -> String {
        let version = self
            .version
            .as_ref()
            .map(|v| format!(" {v}"))
            .unwrap_or_default();
        format!("{}{version}", self.display_name)
    }
}
