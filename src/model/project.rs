use serde::Serialize;
use std::path::PathBuf;

/// Package manager detected for the project.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum PackageManager {
    Npm,
    Yarn,
    Pnpm,
    Bun,
    Cargo,
    Go,
    Pip,
    Poetry,
    Uv,
    Composer,
    Gem,
    Mix,
    Pub,
}

impl PackageManager {
    pub fn display(&self) -> &str {
        match self {
            PackageManager::Npm => "npm",
            PackageManager::Yarn => "yarn",
            PackageManager::Pnpm => "pnpm",
            PackageManager::Bun => "bun",
            PackageManager::Cargo => "cargo",
            PackageManager::Go => "go",
            PackageManager::Pip => "pip",
            PackageManager::Poetry => "poetry",
            PackageManager::Uv => "uv",
            PackageManager::Composer => "composer",
            PackageManager::Gem => "gem",
            PackageManager::Mix => "mix",
            PackageManager::Pub => "pub",
        }
    }
}

/// A detected project.
#[derive(Debug, Clone, Serialize)]
pub struct Project {
    /// Root directory of the project (the closest directory with a manifest)
    #[serde(skip)]
    pub root: PathBuf,
    /// Root of the workspace (if in a monorepo)
    #[serde(skip)]
    pub workspace_root: Option<PathBuf>,
    /// Display name derived from the manifest
    pub name: String,
    /// Technical name from the manifest (e.g., @acme/web)
    pub technical_name: Option<String>,
    /// Programming language
    pub language: Option<String>,
    /// Package manager
    pub package_manager: Option<PackageManager>,
}

impl Project {
    /// Get the display name for UI.
    pub fn display_name(&self) -> &str {
        &self.name
    }

    /// Get the short path relative to workspace or home.
    pub fn short_path(&self, home: Option<&std::path::Path>) -> String {
        let root_str = self.root.to_string_lossy();
        if let Some(home) = home {
            let home_str = home.to_string_lossy();
            if let Some(rest) = root_str.strip_prefix(home_str.as_ref()) {
                return format!("~{}", rest);
            }
        }
        root_str.to_string()
    }
}
