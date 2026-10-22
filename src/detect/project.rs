use crate::model::{PackageManager, Project};
use std::collections::HashSet;
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};

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

/// The names of the entries in `dir`, from a single `read_dir`.
///
/// Detection used to probe each candidate marker with `Path::exists`, i.e. one
/// `stat` per marker per directory: 18 for the package markers, 5 more for the
/// workspace markers, and ~25 for the lockfile/language checks. One `read_dir`
/// answers all of them for a directory. `None` when the directory cannot be
/// listed (EACCES on a traversable but unreadable dir); callers then fall back
/// to `exists()` for that directory, so detection never regresses.
///
/// Also `None` past [`MAX_LISTED_ENTRIES`]: a process whose cwd is `/tmp` or a
/// download folder would otherwise pay a listing of thousands of entries where
/// the ~50 `stat`s it replaces are the cheaper option.
fn entry_names(dir: &Path) -> Option<HashSet<OsString>> {
    let mut names = HashSet::new();
    let entries = std::fs::read_dir(dir).ok()?;
    // One entry can vanish or be unreadable mid-iteration; that must not
    // throw away the names already collected from the rest of the dir.
    for entry in entries.flatten() {
        if names.len() >= MAX_LISTED_ENTRIES {
            return None;
        }
        names.insert(entry.file_name());
    }
    Some(names)
}

/// Directory size beyond which listing it costs more than probing each marker.
const MAX_LISTED_ENTRIES: usize = 512;

/// Is `name` present in `dir`? Uses the pre-read entry set when the directory
/// was listable and falls back to a `stat` when it was not.
fn has_entry(dir: &Path, names: Option<&HashSet<OsString>>, name: &str) -> bool {
    match names {
        Some(names) => names.contains(OsStr::new(name)),
        None => dir.join(name).exists(),
    }
}

/// Which package manager owns a project root.
///
/// For the JavaScript ecosystem the manifest name says nothing — every one of
/// them ships a `package.json`. The lockfile is what actually distinguishes
/// them, so it is consulted first. `names` is the already-read entry set for
/// `root`, when it was listable.
fn detect_package_manager(
    root: &Path,
    marker: &str,
    names: Option<&HashSet<OsString>>,
) -> Option<PackageManager> {
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
        if has_entry(root, names, lockfile) {
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
    let mut project_root: Option<(PathBuf, PathBuf)> = None;
    let mut root_names = None;
    let mut package_manager = None;
    let mut depth = 0;

    loop {
        // One `read_dir` for every marker in this directory (see `entry_names`).
        let names = entry_names(&current);

        // Check for package markers, in priority order: `PACKAGE_MARKERS`
        // still decides which marker wins when several exist.
        for marker in PACKAGE_MARKERS {
            if has_entry(&current, names.as_ref(), marker) {
                package_manager = detect_package_manager(&current, marker, names.as_ref());
                root_names = names;
                project_root = Some((current.clone(), current.join(marker)));
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

    let (root, manifest) = project_root?;

    // Phase 2: Find workspace root
    let mut workspace_root = None;
    // A project at the filesystem root has no parent to search; that is not a
    // reason to discard the project we already found.
    let mut current = match root.parent() {
        Some(parent) => parent.to_path_buf(),
        None => root.clone(),
    };

    loop {
        let names = entry_names(&current);

        // Check for workspace markers
        for marker in WORKSPACE_MARKERS {
            if has_entry(&current, names.as_ref(), marker) {
                workspace_root = Some(current.clone());
                break;
            }
        }

        // Check for Cargo.toml with [workspace]
        if workspace_root.is_none() && has_entry(&current, names.as_ref(), "Cargo.toml") {
            if let Some(content) =
                crate::detect::project::read_manifest(&current.join("Cargo.toml"))
            {
                if content.contains("[workspace]") {
                    workspace_root = Some(current.clone());
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

    // Phase 3: Derive project name. The manifest is read once and shared by
    // the display name and the technical name — reading package.json twice on
    // every L2 cache miss is a doubled read + parse for no benefit.
    let package_name = read_package_name(&root);
    let name = derive_project_name(&root, workspace_root.as_deref(), package_name.as_deref());
    let technical_name = package_name;
    let language = detect_language(&root, root_names.as_ref());

    Some(Project {
        root,
        manifest,
        workspace_root,
        name,
        technical_name,
        language,
        package_manager,
    })
}

/// Derive a display name for the project, with workspace context.
fn derive_project_name(
    root: &Path,
    workspace_root: Option<&Path>,
    package_name: Option<&str>,
) -> String {
    // Try to read package name from manifest
    if let Some(name) = package_name {
        // Skip generic names
        if !matches!(name, "app" | "server" | "main" | "test" | "") {
            return name.to_string();
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
/// No polling, no inotify. The path is the matched manifest recorded on the
/// [`Project`], so the hot path is exactly one `stat` — probing the marker list
/// again would be up to 18 for every (pid, port) on every scan.
pub fn manifest_mtime(manifest: &Path) -> Option<u64> {
    crate::cache::keys::mtime(manifest)
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
    // the first dependency entry in a workspace manifest, and a line-based
    // parser cannot even see that `name = "x" # comment` (legal TOML) is a
    // comment — the old `trim_matches('"')` produced `x" # comment`. The
    // `toml` crate parses the real grammar: inline comments, single quotes,
    // escapes.
    if let Some(content) = read_manifest(&root.join("Cargo.toml")) {
        if let Ok(value) = content.parse::<toml::Value>() {
            if let Some(name) = value
                .get("package")
                .and_then(|p| p.get("name"))
                .and_then(|n| n.as_str())
            {
                return Some(name.to_string());
            }
        }
    }

    // go.mod: the last path component of the module path. The module line can
    // carry a trailing comment (`module x // fork`), so take the first
    // whitespace token before splitting on `/`.
    if let Some(content) = read_manifest(&root.join("go.mod")) {
        for line in content.lines() {
            if let Some(rest) = line.strip_prefix("module") {
                let rest = rest.trim_start();
                if rest.is_empty() || rest.starts_with("//") {
                    continue;
                }
                if let Some(name) = rest
                    .split_whitespace()
                    .next()
                    .and_then(|m| m.rsplit('/').next())
                {
                    return Some(name.to_string());
                }
            }
        }
    }

    None
}

/// Detect the programming language from the manifest files present at root.
///
/// `names` is the entry set already read while locating the root, so this does
/// not re-`stat` every marker.
fn detect_language(root: &Path, names: Option<&HashSet<OsString>>) -> Option<String> {
    // Check in priority order, consulting the same PACKAGE_MARKERS files.
    if has_entry(root, names, "Cargo.toml") {
        return Some("Rust".to_string());
    }
    if has_entry(root, names, "go.mod") {
        return Some("Go".to_string());
    }
    if has_entry(root, names, "package.json")
        || has_entry(root, names, "deno.json")
        || has_entry(root, names, "deno.jsonc")
        || has_entry(root, names, "bunfig.toml")
    {
        return Some("JavaScript".to_string());
    }
    if has_entry(root, names, "pyproject.toml")
        || has_entry(root, names, "setup.py")
        || has_entry(root, names, "setup.cfg")
        || has_entry(root, names, "requirements.txt")
        || has_entry(root, names, "Pipfile")
    {
        return Some("Python".to_string());
    }
    if has_entry(root, names, "Gemfile") {
        return Some("Ruby".to_string());
    }
    if has_entry(root, names, "composer.json") {
        return Some("PHP".to_string());
    }
    if has_entry(root, names, "mix.exs") {
        return Some("Elixir".to_string());
    }
    if has_entry(root, names, "pubspec.yaml") {
        return Some("Dart".to_string());
    }
    if has_entry(root, names, "pom.xml")
        || has_entry(root, names, "build.gradle")
        || has_entry(root, names, "build.gradle.kts")
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
            detect_package_manager(dir.path(), "package.json", entry_names(dir.path()).as_ref()),
            Some(PackageManager::Pnpm)
        );
    }

    #[test]
    fn test_package_json_without_lockfile_defaults_to_npm() {
        let dir = project(&[("package.json", "{}")]);
        assert_eq!(
            detect_package_manager(dir.path(), "package.json", entry_names(dir.path()).as_ref()),
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

    /// `name = "portix" # comment` is legal TOML. The old line-based parser
    /// returned `portix" # comment` — the trailing comment kept the closing
    /// quote from being trimmed.
    #[test]
    fn test_cargo_name_with_inline_comment_is_parsed_cleanly() {
        let dir = project(&[(
            "Cargo.toml",
            "[package]\nname = \"portix\" # the explorer\n",
        )]);
        assert_eq!(read_package_name(dir.path()), Some("portix".to_string()));
    }

    /// Single-quoted strings are legal TOML too; the old `trim_matches('"')`
    /// left the quotes in place.
    #[test]
    fn test_cargo_name_with_single_quotes_is_parsed_cleanly() {
        let dir = project(&[("Cargo.toml", "[package]\nname = 'portix'\n")]);
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

    /// A trailing comment on the module line must not leak into the name.
    #[test]
    fn test_go_module_name_ignores_trailing_comment() {
        let dir = project(&[(
            "go.mod",
            "module github.com/acme/ml-scoring // internal fork\n",
        )]);
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

    /// A huge directory is not listed (the per-marker `stat`s are cheaper),
    /// and detection still finds the manifest through the fallback.
    #[test]
    fn test_huge_directory_falls_back_to_marker_probing() {
        let dir = project(&[("package.json", r#"{"name":"big"}"#)]);
        for i in 0..MAX_LISTED_ENTRIES {
            std::fs::write(dir.path().join(format!("f{i}")), "").unwrap();
        }
        assert!(entry_names(dir.path()).is_none());
        assert_eq!(detect_project(dir.path()).unwrap().name, "big");
    }

    /// `PACKAGE_MARKERS` order still decides which marker wins when several
    /// coexist: `package.json` is first, so it marks the root (and its
    /// ecosystem's package manager) even though `Cargo.toml` is present.
    #[test]
    fn test_package_marker_priority_is_preserved() {
        let dir = project(&[
            ("package.json", r#"{"name":"web"}"#),
            ("Cargo.toml", "[package]\nname = \"web\"\n"),
        ]);

        let detected = detect_project(dir.path()).unwrap();
        assert_eq!(detected.package_manager, Some(PackageManager::Npm));
        assert_eq!(detected.manifest, dir.path().join("package.json"));
    }

    /// The matched manifest is recorded on the project so the framework cache
    /// key can `stat` it directly instead of probing every marker again.
    #[test]
    fn test_manifest_records_the_matched_marker() {
        let dir = project(&[("Cargo.toml", "[package]\nname = \"portix\"\n")]);

        let detected = detect_project(dir.path()).unwrap();
        assert_eq!(detected.manifest, dir.path().join("Cargo.toml"));
    }

    #[test]
    fn test_workspace_root_is_still_found_above_the_project() {
        let workspace = tempfile::tempdir().unwrap();
        std::fs::write(workspace.path().join("pnpm-workspace.yaml"), "").unwrap();
        let member = workspace.path().join("apps/web");
        std::fs::create_dir_all(&member).unwrap();
        std::fs::write(member.join("package.json"), "{}").unwrap();

        let detected = detect_project(&member).unwrap();
        assert_eq!(detected.root, member);
        assert_eq!(detected.workspace_root.as_deref(), Some(workspace.path()));
    }

    /// The recorded manifest is what the L3 key follows: rewriting it must
    /// move the mtime even within the same second.
    #[test]
    fn test_manifest_mtime_tracks_the_recorded_manifest() {
        let dir = project(&[("package.json", r#"{"name":"a"}"#)]);
        let detected = detect_project(dir.path()).unwrap();

        let first = manifest_mtime(&detected.manifest).expect("first mtime");
        std::thread::sleep(std::time::Duration::from_millis(2));
        std::fs::write(&detected.manifest, r#"{"name":"b"}"#).unwrap();
        let second = manifest_mtime(&detected.manifest).expect("second mtime");

        assert_ne!(first, second, "a manifest rewrite must change the key");
    }
}
