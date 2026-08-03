use serde::Deserialize;
use std::path::PathBuf;

/// Configuration for Portix.
#[derive(Debug, Clone, Deserialize)]
pub struct Config {
    /// Refresh interval in milliseconds
    #[serde(default = "default_refresh_ms")]
    pub refresh_ms: u64,

    /// Theme name
    #[serde(default = "default_theme")]
    pub theme: String,

    /// Force ASCII characters
    #[serde(default)]
    pub ascii: bool,

    /// Show system ports (below 1024)
    #[serde(default)]
    pub show_system_ports: bool,

    /// Show UDP sockets
    #[serde(default = "default_true")]
    pub show_udp: bool,

    /// Show only loopback services
    #[serde(default)]
    pub show_loopback_only: bool,

    /// Default sort column
    #[serde(default = "default_sort")]
    pub default_sort: String,

    /// CPU display scale
    #[serde(default = "default_cpu_scale")]
    pub cpu_scale: String,

    /// Memory metric to display
    #[serde(default = "default_memory_metric")]
    pub memory_metric: String,

    /// Require confirmation for kill
    #[serde(default = "default_true")]
    pub kill_confirm: bool,

    /// Browser command
    #[serde(default = "default_browser")]
    pub browser: String,

    /// Editor command
    #[serde(default = "default_editor")]
    pub editor: String,

    /// Docker socket path
    #[serde(default = "default_docker_socket")]
    pub docker_socket: String,
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
fn default_sort() -> String {
    "port".to_string()
}
fn default_cpu_scale() -> String {
    "system".to_string()
}
fn default_memory_metric() -> String {
    "rss".to_string()
}
fn default_browser() -> String {
    std::env::var("BROWSER").unwrap_or_else(|_| "xdg-open".to_string())
}
fn default_editor() -> String {
    std::env::var("VISUAL")
        .or_else(|_| std::env::var("EDITOR"))
        .unwrap_or_else(|_| "vi".to_string())
}
fn default_docker_socket() -> String {
    "/var/run/docker.sock".to_string()
}

impl Default for Config {
    fn default() -> Self {
        Self {
            refresh_ms: default_refresh_ms(),
            theme: default_theme(),
            ascii: false,
            show_system_ports: false,
            show_udp: default_true(),
            show_loopback_only: false,
            default_sort: default_sort(),
            cpu_scale: default_cpu_scale(),
            memory_metric: default_memory_metric(),
            kill_confirm: default_true(),
            browser: default_browser(),
            editor: default_editor(),
            docker_socket: default_docker_socket(),
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
    pub fn save_theme(&self) -> std::io::Result<()> {
        let path = Self::config_path();
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let existing = std::fs::read_to_string(&path).unwrap_or_default();
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
            let is_theme_key = line
                .trim_start()
                .split('=')
                .next()
                .map(str::trim)
                == Some("theme");
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
}
