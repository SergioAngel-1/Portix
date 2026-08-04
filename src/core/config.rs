use serde::Deserialize;
use std::path::PathBuf;

/// Configuration for Portix.
///
/// Unknown keys in `config.toml` are ignored by serde, so keys reserved for
/// v1 features (see PRD §12.1) can be present in user files without breaking
/// parsing — they just have no effect yet.
#[derive(Debug, Clone, Deserialize)]
pub struct Config {
    /// Refresh interval in milliseconds
    #[serde(default = "default_refresh_ms")]
    pub refresh_ms: u64,

    /// Theme name
    #[serde(default = "default_theme")]
    pub theme: String,

    /// Show system ports (below 1024)
    #[serde(default)]
    pub show_system_ports: bool,

    /// Show UDP sockets
    #[serde(default = "default_true")]
    pub show_udp: bool,

    /// Show only loopback services
    #[serde(default)]
    pub show_loopback_only: bool,

    /// CPU display scale: "system" (0-100%) or "core" (0-100%×ncpu)
    #[serde(default = "default_cpu_scale")]
    pub cpu_scale: String,

    /// Require confirmation for kill
    #[serde(default = "default_true")]
    pub kill_confirm: bool,
}

fn default_refresh_ms() -> u64 {
    1000
}
fn default_theme() -> String {
    "default-dark".to_string()
}
fn default_true() -> bool {
    true
}
fn default_cpu_scale() -> String {
    "system".to_string()
}

impl Default for Config {
    fn default() -> Self {
        Self {
            refresh_ms: default_refresh_ms(),
            theme: default_theme(),
            show_system_ports: false,
            show_udp: default_true(),
            show_loopback_only: false,
            cpu_scale: default_cpu_scale(),
            kill_confirm: default_true(),
        }
    }
}

impl Config {
    /// Load configuration from the default location.
    pub fn load() -> Self {
        let config_dir = directories::ProjectDirs::from("", "", "portix")
            .map(|d| d.config_dir().to_path_buf())
            .unwrap_or_else(|| PathBuf::from("~/.config/portix"));

        let config_file = config_dir.join("config.toml");

        if config_file.exists() {
            if let Ok(content) = std::fs::read_to_string(&config_file) {
                if let Ok(config) = toml::from_str(&content) {
                    return config;
                }
                eprintln!(
                    "portix: config.toml exists but is invalid TOML; using defaults. Fix or remove {}",
                    config_file.display()
                );
            }
        }

        Self::default()
    }

    /// Get the minimum refresh interval.
    pub fn min_refresh_ms(&self) -> u64 {
        self.refresh_ms.max(200)
    }

    /// Path to the config file, honoring XDG.
    fn config_path() -> PathBuf {
        directories::ProjectDirs::from("", "", "portix")
            .map(|d| d.config_dir().to_path_buf())
            .unwrap_or_else(|| PathBuf::from("~/.config/portix"))
            .join("config.toml")
    }

    /// Persist the current theme back to `config.toml`, surgically: only the
    /// `theme` line is rewritten, so hand-written comments and other settings
    /// survive. The file (and its directory) are created if absent.
    ///
    /// An unreadable-but-existing file is an error and propagates to the
    /// caller: silently overwriting it with a one-line file would destroy the
    /// user's other settings. A *missing* file is not an error — it is the
    /// first save.
    pub fn save_theme(&self) -> std::io::Result<()> {
        self.save_theme_to(Self::config_path())
    }

    /// Same as [`save_theme`], but writes to an explicit path. Split out so
    /// tests can exercise the write without touching the real XDG config.
    fn save_theme_to(&self, path: PathBuf) -> std::io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let existing = match std::fs::read_to_string(&path) {
            Ok(s) => s,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
            Err(e) => return Err(e),
        };
        std::fs::write(&path, upsert_theme_line(&existing, &self.theme))
    }
}

/// Replace the `theme = "…"` line in a config file, or prepend it if absent.
/// Commented lines (`# theme = …`) are left untouched.
fn upsert_theme_line(content: &str, theme: &str) -> String {
    let new_line = format!("theme = \"{theme}\"");
    let mut replaced = false;

    let mut lines: Vec<String> = content
        .lines()
        .map(|line| {
            let is_theme_key = line.trim_start().split('=').next().map(str::trim) == Some("theme");
            if is_theme_key {
                replaced = true;
                new_line.clone()
            } else {
                line.to_string()
            }
        })
        .collect();

    if !replaced {
        lines.insert(0, new_line);
    }

    let mut out = lines.join("\n");
    out.push('\n');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_upsert_replaces_existing_theme_and_keeps_comments() {
        let original = "# my config\ntheme = \"default-dark\"\nrefresh_ms = 500\n";
        let updated = upsert_theme_line(original, "mono");
        assert!(updated.contains("theme = \"mono\""));
        assert!(!updated.contains("default-dark"));
        assert!(updated.contains("# my config"));
        assert!(updated.contains("refresh_ms = 500"));
    }

    #[test]
    fn test_upsert_leaves_commented_theme_alone_and_adds_a_real_one() {
        let updated = upsert_theme_line("# theme = \"dark\"\n", "mono");
        assert!(updated.contains("# theme = \"dark\""));
        assert!(updated.contains("theme = \"mono\""));
    }

    #[test]
    fn test_upsert_into_empty_file() {
        assert_eq!(upsert_theme_line("", "auto"), "theme = \"auto\"\n");
    }

    /// The first theme save on a machine with no config.toml must create the
    /// file. The previous implementation treated a missing file as a read
    /// error and the save silently failed ("could not save theme").
    #[test]
    fn test_save_theme_creates_file_when_absent() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let config = Config {
            theme: "mono".to_string(),
            ..Config::default()
        };

        config.save_theme_to(path.clone()).unwrap();

        let content = std::fs::read_to_string(&path).unwrap();
        assert_eq!(content, "theme = \"mono\"\n");
    }

    /// An existing-but-unreadable file must propagate the error, never be
    /// silently replaced by a one-line file.
    #[test]
    fn test_save_theme_propagates_unreadable_existing_file() {
        let dir = tempfile::tempdir().unwrap();
        // A directory is not a readable file: read_to_string fails with
        // IsADirectory, which is not NotFound.
        let path = dir.path().join("config.toml");
        std::fs::create_dir(&path).unwrap();

        let config = Config::default();
        assert!(config.save_theme_to(path).is_err());
    }
}
