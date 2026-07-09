use crate::model::{PackageManager, Project};
use std::path::{Path, PathBuf};

/// Package marker files and their priority order.
const PACKAGE_MARKERS: &[(&str, fn(&str) -> Option<PackageManager>)] = &[
    ("package.json", |_| Some(PackageManager::Npm)),
    ("deno.json", |_| None),
    ("deno.jsonc", |_| None),
    ("bunfig.toml", |_| Some(PackageManager::Bun)),
    ("pyproject.toml", |_| Some(PackageManager::Pip)),
    ("setup.py", |_| Some(PackageManager::Pip)),
    ("setup.cfg", |_| Some(PackageManager::Pip)),
    ("requirements.txt", |_| Some(PackageManager::Pip)),
    ("Pipfile", |_| Some(PackageManager::Pip)),
    ("go.mod", |_| Some(PackageManager::Go)),
    ("Cargo.toml", |_| Some(PackageManager::Cargo)),
    ("composer.json", |_| Some(PackageManager::Composer)),
    ("Gemfile", |_| Some(PackageManager::Gem)),
    ("pom.xml", |_| None),
    ("build.gradle", |_| None),
    ("build.gradle.kts", |_| None),
    ("mix.exs", |_| Some(PackageManager::Mix)),
    ("pubspec.yaml", |_| Some(PackageManager::Pub)),
];

/// Workspace marker files.
const WORKSPACE_MARKERS: &[&str] = &[
    "pnpm-workspace.yaml",
    "turbo.json",
    "nx.json",
    "lerna.json",
    "go.work",
];

/// Detect the project from a process's working directory.
pub fn detect_project(cwd: &Path) -> Option<Project> {
    if !cwd.exists() {
        return None;
    }

    let mut current = cwd.to_path_buf();
    let home = directories::UserDirs::new().map(|u| u.home_dir().to_path_buf());

    // Phase 1: Find the package marker (project root)
    let mut project_root = None;
    let mut package_manager = None;
    let mut depth = 0;

    loop {
        // Check for package markers
        for (marker, pm_fn) in PACKAGE_MARKERS {
            if current.join(marker).exists() {
                project_root = Some(current.clone());
                package_manager = pm_fn(marker);
                break;
            }
        }

        if project_root.is_some() {
            break;
        }

        // Check for workspace markers (might be workspace root)
        for marker in WORKSPACE_MARKERS {
            if current.join(marker).exists() {
                // Found workspace marker, but keep looking for package marker
            }
        }

        // Stop at home directory or root
        if let Some(ref home) = home {
            if &current == home {
                break;
            }
        }
        if current == Path::new("/") {
            break;
        }

        // Go up one directory
        if !current.pop() {
            break;
        }

        depth += 1;
        if depth >= 24 {
            break; // Safety limit
        }
    }

    let root = project_root?;

    // Phase 2: Find workspace root
    let mut workspace_root = None;
    let mut current = root.parent()?.to_path_buf();

    loop {
        // Check for workspace markers
        for marker in WORKSPACE_MARKERS {
            if current.join(marker).exists() {
                workspace_root = Some(current.clone());
                break;
            }
        }

        // Check for Cargo.toml with [workspace]
        if current.join("Cargo.toml").exists() {
            if let Ok(content) = std::fs::read_to_string(current.join("Cargo.toml")) {
                if content.contains("[workspace]") {
                    workspace_root = Some(current.clone());
                    break;
                }
            }
        }

        if workspace_root.is_some() {
            break;
        }

        // Stop at home directory or root
        if let Some(ref home) = home {
            if &current == home {
                break;
            }
        }
        if current == Path::new("/") {
            break;
        }

        if !current.pop() {
            break;
        }

        depth += 1;
        if depth >= 24 {
            break;
        }
    }

    // Phase 3: Derive project name
    let name = derive_project_name(&root, workspace_root.as_deref());

    Some(Project {
        root,
        workspace_root,
        name,
        technical_name: None, // TODO: Read from manifest
        language: None,       // TODO: Detect from manifest/process
        package_manager,
    })
}

/// Derive a display name for the project.
fn derive_project_name(root: &Path, workspace_root: Option<&Path>) -> String {
    // Try to read package name from manifest
    if let Some(name) = read_package_name(root) {
        // Skip generic names
        if !matches!(name.as_str(), "app" | "server" | "main" | "test" | "") {
            return name;
        }
    }

    // If in a workspace, show workspace/member
    if let Some(ws_root) = workspace_root {
        let ws_name = ws_root
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("workspace");
        let member_name = root
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("project");
        return format!("{}/{}", ws_name, member_name);
    }

    // Fall back to directory name
    root.file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("project")
        .to_string()
}

/// Try to read the package name from a manifest file.
fn read_package_name(root: &Path) -> Option<String> {
    // package.json
    if let Ok(content) = std::fs::read_to_string(root.join("package.json")) {
        if let Ok(json) = serde_json::from_str::<serde_json::Value>(&content) {
            if let Some(name) = json.get("name").and_then(|n| n.as_str()) {
                return Some(name.to_string());
            }
        }
    }

    // Cargo.toml
    if let Ok(content) = std::fs::read_to_string(root.join("Cargo.toml")) {
        for line in content.lines() {
            if let Some(name) = line.strip_prefix("name") {
                let name = name.trim().strip_prefix('=').unwrap_or(name).trim();
                let name = name.trim_matches('"');
                return Some(name.to_string());
            }
        }
    }

    // go.mod
    if let Ok(content) = std::fs::read_to_string(root.join("go.mod")) {
        for line in content.lines() {
            if let Some(module) = line.strip_prefix("module") {
                let module = module.trim();
                // Return the last path component
                if let Some(name) = module.rsplit('/').next() {
                    return Some(name.to_string());
                }
            }
        }
    }

    None
}
