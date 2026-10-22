use serde::Deserialize;
use std::path::PathBuf;

/// CPU display scale: "system" (0-100%) or "core" (0-100%×ncpu).
///
/// `#[serde(other)]` on `System` keeps parsing lenient: an unknown value
/// (`cpu_scale = "weird"`) falls back to the system scale — today's behavior —
/// instead of rejecting the whole config file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CpuScale {
    Core,
    #[serde(other)]
    System,
}

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

    /// CPU display scale
    #[serde(default = "default_cpu_scale")]
    pub cpu_scale: CpuScale,

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
fn default_cpu_scale() -> CpuScale {
    CpuScale::System
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

/// Home of the user who invoked `sudo portix`, when running under sudo.
///
/// sudo resets `$HOME` to the root user's (PRD §13.3): without this, the
/// invoking user's config would be ignored, a theme save would land in
/// `/root/.config`, and `~/…` path shortening would use the wrong prefix.
/// `None` when there is no `$SUDO_USER` to honor, or it does not resolve.
fn sudo_user_home() -> Option<PathBuf> {
    let user = std::env::var("SUDO_USER").ok()?;
    if user.is_empty() {
        return None;
    }
    crate::platform::linux::proc::home_dir_for_user(&user)
}

/// The home directory display paths are shortened against (`~/…`).
///
/// Same `$SUDO_USER` logic as [`sudo_user_home`] so the info panel and the
/// config loader agree under `sudo portix`; falls back to `$HOME`. Lives in
/// `core` because the passwd lookup goes through `platform`, which `ui` must
/// not import.
pub fn display_home() -> Option<PathBuf> {
    sudo_user_home().or_else(|| std::env::var_os("HOME").map(PathBuf::from))
}

/// The directory holding `config.toml`, honoring XDG.
///
/// Under `sudo portix` (PRD §13.3), sudo resets `$HOME` to the root user's:
/// the invoking user's config would be ignored, and a theme save would land
/// in `/root/.config`. When `$SUDO_USER` is set, resolve that user's home via
/// passwd instead. Falls back to the XDG resolution (which itself falls back
/// to `~/.config/portix`) when there is no `$SUDO_USER` to honor.
///
/// `None` when no home directory resolves at all. The old fallback was
/// `PathBuf::from("~/.config/portix")` — a *relative* path with a literal
/// `~`, so a theme save without a resolvable home created
/// `./~/.config/portix/config.toml` in the current directory.
fn config_dir() -> Option<PathBuf> {
    if let Some(home) = sudo_user_home() {
        return Some(home.join(".config/portix"));
    }
    directories::ProjectDirs::from("", "", "portix").map(|d| d.config_dir().to_path_buf())
}

impl Config {
    /// Load configuration from the default location.
    ///
    /// `read_to_string` is the existence check: a `NotFound` means "no
    /// config", and any other error is reported. The previous `exists()` +
    /// read was both racy and one extra syscall.
    pub fn load() -> Self {
        let Some(config_file) = config_dir().map(|dir| dir.join("config.toml")) else {
            return Self::default();
        };

        let content = match std::fs::read_to_string(&config_file) {
            Ok(content) => content,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Self::default(),
            Err(e) => {
                eprintln!("portix: config.toml exists but could not be read ({e}); using defaults");
                return Self::default();
            }
        };

        match toml::from_str(&content) {
            Ok(config) => config,
            Err(e) => {
                eprintln!(
                    "portix: config.toml exists but is invalid TOML ({e}); using defaults. Fix or remove {}",
                    config_file.display()
                );
                Self::default()
            }
        }
    }

    /// Get the minimum refresh interval.
    pub fn min_refresh_ms(&self) -> u64 {
        self.refresh_ms.max(200)
    }

    /// Path to the config file, honoring XDG. `None` when there is no home.
    fn config_path() -> Option<PathBuf> {
        config_dir().map(|dir| dir.join("config.toml"))
    }

    /// Persist the current theme back to `config.toml`, surgically: only the
    /// `theme` line is rewritten, so hand-written comments and other settings
    /// survive. The file (and its directory) are created if absent.
    ///
    /// An unreadable-but-existing file is an error and propagates to the
    /// caller: silently overwriting it with a one-line file would destroy the
    /// user's other settings. A *missing* file is not an error — it is the
    /// first save. With no resolvable home there is nowhere sensible to write,
    /// so this fails instead of creating a `~` directory in the cwd.
    pub fn save_theme(&self) -> std::io::Result<()> {
        Self::save_theme_in(Self::config_path(), &self.theme)
    }

    /// [`save_theme`] against an already-resolved config directory.
    ///
    /// Split out so tests can exercise the no-home error without mutating the
    /// process environment.
    fn save_theme_in(config_dir: Option<PathBuf>, theme: &str) -> std::io::Result<()> {
        let Some(dir) = config_dir else {
            return Err(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "no home directory to save config.toml",
            ));
        };
        let path = dir.join("config.toml");
        Self::save_theme_to(path, theme)
    }

    /// Same as [`save_theme`], but writes to an explicit path. Split out so
    /// tests can exercise the write without touching the real XDG config.
    ///
    /// The write is atomic: content goes to a sibling temp file which is
    /// renamed over the target. A crash mid-write can no longer leave a
    /// truncated `config.toml` that silently resets every setting on the next
    /// load.
    fn save_theme_to(path: PathBuf, theme: &str) -> std::io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let existing = match std::fs::read_to_string(&path) {
            Ok(s) => s,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
            Err(e) => return Err(e),
        };
        let content = upsert_theme_line(&existing, theme);
        let tmp = path.with_extension("toml.tmp");
        std::fs::write(&tmp, content)?;
        match std::fs::rename(&tmp, &path) {
            Ok(()) => Ok(()),
            Err(e) => {
                let _ = std::fs::remove_file(&tmp);
                Err(e)
            }
        }
    }
}

/// Replace the `theme = "…"` line in a config file, or prepend it if absent.
/// Commented lines (`# theme = …`) are left untouched.
fn upsert_theme_line(content: &str, theme: &str) -> String {
    let new_line = format!("theme = {}", toml_basic_string(theme));
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

/// Quote a value as a TOML basic string.
///
/// The theme name normally comes from the fixed `Theme::NAMES`, but it can
/// also be read back from a hand-edited `config.toml` (`theme = 'ra"ro'` is
/// valid TOML). Writing such a name back with `format!("theme = \"{name}\"")`
/// would corrupt the file on the next load. Rust's `escape_default` is not
/// enough either: it leaves control characters raw, which TOML forbids in
/// basic strings, and it renders `\u{...}` forms that TOML does not accept.
/// This writes exactly the escapes TOML defines.
fn toml_basic_string(value: &str) -> String {
    use std::fmt::Write;

    let mut out = String::with_capacity(value.len() + 2);
    out.push('"');
    for c in value.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\t' => out.push_str("\\t"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\u{8}' => out.push_str("\\b"),
            '\u{C}' => out.push_str("\\f"),
            c if c.is_control() => {
                let _ = write!(out, "\\u{:04X}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
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

    /// A theme name read back from a hand-edited config can contain quotes,
    /// backslashes or even newlines. Writing it raw corrupts the file; the
    /// escaped form must round-trip through a real TOML parse.
    #[test]
    fn test_theme_with_hostile_name_round_trips_through_toml() {
        let hostile = "ra\"ro\\\n\t\x01";
        let updated = upsert_theme_line("refresh_ms = 500\n", hostile);
        let parsed: toml::Value = toml::from_str(&updated).unwrap();
        assert_eq!(parsed["theme"].as_str(), Some(hostile));
        assert_eq!(parsed["refresh_ms"].as_integer(), Some(500));

        // And the file must still parse on the next load (Config::load path).
        let config: Config = toml::from_str(&updated).unwrap();
        assert_eq!(config.theme, hostile);
    }

    /// The escaped line must still be recognised as the theme key by a
    /// second upsert (the value is read back, modified, written again).
    #[test]
    fn test_escaped_theme_line_is_recognised_by_next_upsert() {
        let first = upsert_theme_line("", "ra\"ro");
        let second = upsert_theme_line(&first, "mono");
        assert_eq!(second, "theme = \"mono\"\n");
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

        Config::save_theme_to(path.clone(), &config.theme).unwrap();

        let content = std::fs::read_to_string(&path).unwrap();
        assert_eq!(content, "theme = \"mono\"\n");
        // The atomic rename must not leave the temp file behind.
        assert!(
            !path.with_extension("toml.tmp").exists(),
            "temp file must be renamed away"
        );
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
        assert!(Config::save_theme_to(path, &config.theme).is_err());
    }

    #[test]
    fn test_cpu_scale_parses_known_values() {
        let config: Config = toml::from_str("cpu_scale = \"core\"").unwrap();
        assert_eq!(config.cpu_scale, CpuScale::Core);

        let config: Config = toml::from_str("cpu_scale = \"system\"").unwrap();
        assert_eq!(config.cpu_scale, CpuScale::System);
    }

    /// An unknown scale is not a typo worth losing the whole config over: it
    /// keeps the old lenient behavior and falls back to the system scale.
    #[test]
    fn test_cpu_scale_unknown_value_falls_back_to_system() {
        let config: Config = toml::from_str("cpu_scale = \"weird\"\nrefresh_ms = 500").unwrap();
        assert_eq!(config.cpu_scale, CpuScale::System);
        assert_eq!(config.refresh_ms, 500, "the rest of the config still loads");
    }

    #[test]
    fn test_cpu_scale_defaults_to_system() {
        let config: Config = toml::from_str("").unwrap();
        assert_eq!(config.cpu_scale, CpuScale::System);
    }

    /// Without a resolvable home there is nowhere to save: erroring beats the
    /// old fallback, which wrote to the *relative* path `./~/.config/portix`.
    #[test]
    fn test_save_theme_without_home_returns_not_found() {
        let error = Config::save_theme_in(None, "mono").expect_err("no home is an error");
        assert_eq!(error.kind(), std::io::ErrorKind::NotFound);
        assert!(error.to_string().contains("no home directory"));
    }
}
