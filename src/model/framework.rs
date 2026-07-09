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

impl Language {
    pub fn display(&self) -> &str {
        match self {
            Language::Node => "Node",
            Language::Python => "Python",
            Language::Go => "Go",
            Language::Rust => "Rust",
            Language::Php => "PHP",
            Language::Ruby => "Ruby",
            Language::Java => "Java",
            Language::Elixir => "Elixir",
            Language::Dart => "Dart",
            Language::Unknown => "—",
        }
    }
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
    /// Get the display string for the framework column.
    pub fn display(&self) -> String {
        let version = self
            .version
            .as_ref()
            .map(|v| format!(" {}", v))
            .unwrap_or_default();
        format!(
            "{} {}{}{}",
            self.confidence.glyph(),
            self.display_name,
            version,
            self.confidence.suffix()
        )
    }
}

/// Relationship between frameworks.
#[derive(Debug, Clone)]
pub enum FrameworkRelation {
    /// If both score high, show only the supersedes one
    Supersedes { supersedes: String, superseded: String },
    /// Show the implies one as suffix
    Implies { primary: String, secondary: String },
}
