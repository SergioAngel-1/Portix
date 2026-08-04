use crate::model::{PackageManager, Project};
use std::path::Path;

/// Manifest files that mark a project root, in priority order.
const PACKAGE_MARKERS: &[&str] = &[
    "package.json",
    "deno.json",
    "deno.jsonc",
    "bunfig.toml",
    "pyproject.toml",
    "setup.py",
    "setup.cfg",
    "requirements.txt",
    "Pipfile",
    "go.mod",
    "Cargo.toml",
    "composer.json",
    "Gemfile",
    "pom.xml",
    "build.gradle",
    "build.gradle.kts",
    "mix.exs",
    "pubspec.yaml",
];

/// Which package manager owns a project root.
///
/// For the JavaScript ecosystem the manifest name says nothing — every one of
/// them ships a `package.json`. The lockfile is what actually distinguishes
/// them, so it is consulted first.
fn detect_package_manager(root: &Path, marker: &str) -> Option<PackageManager> {
    const LOCKFILES: &[(&str, PackageManager)] = &[
        ("pnpm-lock.yaml", PackageManager::Pnpm),
        ("yarn.lock", PackageManager::Yarn),
        ("bun.lockb", PackageManager::Bun),
        ("bun.lock", PackageManager::Bun),
        ("package-lock.json", PackageManager::Npm),
        ("uv.lock", PackageManager::Uv),
        ("poetry.lock", PackageManager::Poetry),
    ];

    for (lockfile, pm) in LOCKFILES {
        if root.join(lockfile).exists() {
            return Some(*pm);
        }
    }

    match marker {
        "package.json" => Some(PackageManager::Npm),
        "bunfig.toml" => Some(PackageManager::Bun),
        "pyproject.toml" | "setup.py" | "setup.cfg" | "requirements.txt" | "Pipfile" => {
            Some(PackageManager::Pip)
        }
        "go.mod" => Some(PackageManager::Go),
        "Cargo.toml" => Some(PackageManager::Cargo),
        "composer.json" => Some(PackageManager::Composer),
        "Gemfile" => Some(PackageManager::Gem),
        "mix.exs" => Some(PackageManager::Mix),
        "pubspec.yaml" => Some(PackageManager::Pub),
        _ => None,
    }
}

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
        for marker in PACKAGE_MARKERS {
            if current.join(marker).exists() {
                package_manager = detect_package_manager(&current, marker);
                project_root = Some(current.clone());
                break;
            }
        }

        if project_root.is_some() {
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
    // A project at the filesystem root has no parent to search; that is not a
    // reason to discard the project we already found.
    let mut current = match root.parent() {
        Some(parent) => parent.to_path_buf(),
        None => root.clone(),
    };

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
            if let Some(content) =
                crate::detect::project::read_manifest(&current.join("Cargo.toml"))
            {
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
    let technical_name = read_package_name(&root);
    let language = detect_language(&root);

    Some(Project {
        root,
        workspace_root,
        name,
        technical_name,
        language,
        package_manager,
    })
}

/// Derive a display name for the project, with workspace context.
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

/// The mtime of the project's manifest, used as a cache-invalidation key.
///
/// When `pnpm add next@16` rewrites package.json, its mtime changes, the L3
/// framework key changes, and the displayed version refreshes on the next scan.
/// No polling, no inotify.
pub fn manifest_mtime(root: &Path) -> Option<u64> {
    PACKAGE_MARKERS
        .iter()
        .map(|marker| root.join(marker))
        .find(|p| p.exists())
        .and_then(|p| crate::cache::keys::mtime(&p))
}

/// Read a manifest file with a hard cap and a regular-file requirement
/// (PRD §13.2: 1 MiB).
///
/// A `cwd` that points at a FIFO or device node must not block the scan
/// forever on `open()`, and an oversized manifest must not be slurped whole.
/// A missing file is `None`, as is anything that is not a regular file.
pub(crate) fn read_manifest(path: &Path) -> Option<String> {
    const MAX_MANIFEST_BYTES: u64 = 1024 * 1024;
    let meta = std::fs::metadata(path).ok()?;
    if !meta.is_file() || meta.len() > MAX_MANIFEST_BYTES {
        return None;
    }
    std::fs::read_to_string(path).ok()
}

/// Try to read the package name from a manifest file.
fn read_package_name(root: &Path) -> Option<String> {
    // package.json
    if let Some(content) = read_manifest(&root.join("package.json")) {
        if let Ok(json) = serde_json::from_str::<serde_json::Value>(&content) {
            if let Some(name) = json.get("name").and_then(|n| n.as_str()) {
                return Some(name.to_string());
            }
        }
    }

    // Cargo.toml: only the name under [package]. A bare `name =` scan picks up
    // the first dependency entry in a workspace manifest.
    if let Some(content) = read_manifest(&root.join("Cargo.toml")) {
        let mut in_package = false;
        for line in content.lines() {
            let line = line.trim();
            if line.starts_with('[') {
                in_package = line == "[package]";
                continue;
            }
            if in_package {
                if let Some((key, value)) = line.split_once('=') {
                    if key.trim() == "name" {
                        return Some(value.trim().trim_matches('"').to_string());
                    }
                }
            }
        }
    }

    // go.mod: the last path component of the module path.
    if let Some(content) = read_manifest(&root.join("go.mod")) {
        for line in content.lines() {
            if let Some(module) = line.strip_prefix("module ") {
                if let Some(name) = module.trim().rsplit('/').next() {
                    return Some(name.to_string());
                }
            }
        }
    }

    None
}

/// Detect the programming language from the manifest files present at root.
fn detect_language(root: &Path) -> Option<String> {
    // Check in priority order, consulting the same PACKAGE_MARKERS files.
    if root.join("Cargo.toml").exists() {
        return Some("Rust".to_string());
    }
    if root.join("go.mod").exists() {
        return Some("Go".to_string());
    }
    if root.join("package.json").exists()
        || root.join("deno.json").exists()
        || root.join("deno.jsonc").exists()
        || root.join("bunfig.toml").exists()
    {
        return Some("JavaScript".to_string());
    }
    if root.join("pyproject.toml").exists()
        || root.join("setup.py").exists()
        || root.join("setup.cfg").exists()
        || root.join("requirements.txt").exists()
        || root.join("Pipfile").exists()
    {
        return Some("Python".to_string());
    }
    if root.join("Gemfile").exists() {
        return Some("Ruby".to_string());
    }
    if root.join("composer.json").exists() {
        return Some("PHP".to_string());
    }
    if root.join("mix.exs").exists() {
        return Some("Elixir".to_string());
    }
    if root.join("pubspec.yaml").exists() {
        return Some("Dart".to_string());
    }
    if root.join("pom.xml").exists()
        || root.join("build.gradle").exists()
        || root.join("build.gradle.kts").exists()
    {
        return Some("Java".to_string());
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project(files: &[(&str, &str)]) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        for (name, content) in files {
            std::fs::write(dir.path().join(name), content).unwrap();
        }
        dir
    }

    #[test]
    fn test_lockfile_beats_manifest_for_package_manager() {
        let dir = project(&[("package.json", "{}"), ("pnpm-lock.yaml", "")]);
        assert_eq!(
            detect_package_manager(dir.path(), "package.json"),
            Some(PackageManager::Pnpm)
        );
    }

    #[test]
    fn test_package_json_without_lockfile_defaults_to_npm() {
        let dir = project(&[("package.json", "{}")]);
        assert_eq!(
            detect_package_manager(dir.path(), "package.json"),
            Some(PackageManager::Npm)
        );
    }

    #[test]
    fn test_cargo_name_is_read_from_package_section_only() {
        let dir = project(&[(
            "Cargo.toml",
            "[workspace]\nmembers = []\n\n[package]\nname = \"portix\"\n\n[dependencies]\nname = \"wrong\"\n",
        )]);
        assert_eq!(read_package_name(dir.path()), Some("portix".to_string()));
    }

    #[test]
    fn test_go_module_name_is_last_path_component() {
        let dir = project(&[("go.mod", "module github.com/acme/ml-scoring\n")]);
        assert_eq!(
            read_package_name(dir.path()),
            Some("ml-scoring".to_string())
        );
    }

    #[test]
    fn test_detect_project_ascends_to_the_nearest_manifest() {
        let dir = project(&[("package.json", r#"{"name":"@acme/web"}"#)]);
        let nested = dir.path().join("src/routes");
        std::fs::create_dir_all(&nested).unwrap();

        let project = detect_project(&nested).unwrap();
        assert_eq!(project.name, "@acme/web");
        assert_eq!(project.root, dir.path());
    }

    #[test]
    fn test_no_manifest_yields_no_project() {
        let dir = tempfile::tempdir().unwrap();
        assert!(detect_project(dir.path()).is_none());
    }

    #[test]
    fn test_language_detected_from_cargo_toml() {
        let dir = project(&[("Cargo.toml", "[package]\nname = \"portix\"\n")]);
        let project = detect_project(dir.path()).unwrap();
        assert_eq!(project.language.as_deref(), Some("Rust"));
    }

    #[test]
    fn test_language_detected_from_package_json() {
        let dir = project(&[("package.json", "{}")]);
        let project = detect_project(dir.path()).unwrap();
        assert_eq!(project.language.as_deref(), Some("JavaScript"));
    }

    #[test]
    fn test_technical_name_set_from_manifest() {
        let dir = project(&[("package.json", r#"{"name":"@acme/web"}"#)]);
        let project = detect_project(dir.path()).unwrap();
        assert_eq!(project.technical_name.as_deref(), Some("@acme/web"));
    }

    #[test]
    fn test_technical_name_none_when_no_manifest_name() {
        let dir = project(&[("Cargo.toml", "[package]\n")]);
        let project = detect_project(dir.path()).unwrap();
        assert!(project.technical_name.is_none());
    }

    /// A manifest over the 1 MiB cap is not slurped: it yields no name, no
    /// dependencies, nothing — and the scan does not hang on it.
    #[test]
    fn test_read_manifest_rejects_oversized_files() {
        let dir = project(&[]);
        let big = dir.path().join("package.json");
        std::fs::write(&big, " ".repeat(1024 * 1024 + 1)).unwrap();
        assert!(read_manifest(&big).is_none());
    }

    /// A directory (or FIFO/device) planted where a manifest should be must
    /// not block the scan: `read_manifest` requires a regular file.
    #[test]
    fn test_read_manifest_ignores_non_regular_files() {
        let dir = project(&[]);
        let d = dir.path().join("package.json");
        std::fs::create_dir(&d).unwrap();
        assert!(read_manifest(&d).is_none());
    }
}
