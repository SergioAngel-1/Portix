use crate::detect::signals::{kinds, Signal};
use crate::model::{Framework, Language};
use std::path::Path;

/// (framework id, executable name under node_modules/.bin)
const NODE_BINARIES: &[(&str, &str)] = &[
    ("nextjs", "next"),
    ("vite", "vite"),
    ("astro", "astro"),
    ("nuxt", "nuxt"),
    ("remix", "remix"),
    ("nestjs", "nest"),
];

/// (framework id, dependency key in package.json)
const NODE_DEPENDENCIES: &[(&str, &str)] = &[
    ("nextjs", "next"),
    ("vite", "vite"),
    ("astro", "astro"),
    ("nuxt", "nuxt"),
    ("remix", "@remix-run/node"),
    ("nestjs", "@nestjs/core"),
    ("express", "express"),
];

/// (framework id, files whose presence implies it)
const PROJECT_FILES: &[(&str, &[&str])] = &[
    (
        "nextjs",
        &["next.config.js", "next.config.ts", "next.config.mjs"],
    ),
    (
        "vite",
        &["vite.config.js", "vite.config.ts", "vite.config.mjs"],
    ),
    ("astro", &["astro.config.mjs", "astro.config.ts"]),
    ("nuxt", &["nuxt.config.ts", "nuxt.config.js"]),
    ("nestjs", &["nest-cli.json"]),
    ("django", &["manage.py"]),
    ("go", &["go.mod"]),
    ("rust", &["Cargo.toml"]),
];

/// The port a framework is conventionally served on.
fn conventional_port(port: u16) -> Option<&'static str> {
    match port {
        3000 => Some("nextjs"),
        5173 => Some("vite"),
        4321 => Some("astro"),
        8000 => Some("django"),
        _ => None,
    }
}

/// Is `arg` an ASGI app target like `app:app` or `pkg.main:app` — a Python
/// module path and an attribute, separated by a single colon?
///
/// Requires a letter on each side so it can't fire on `0.0.0.0:8000` or `[::]`.
fn is_asgi_app_target(arg: &str) -> bool {
    let Some((module, attr)) = arg.split_once(':') else {
        return false;
    };
    let is_ident = |s: &str, dot_ok: bool| {
        !s.is_empty()
            && s.chars().any(|c| c.is_ascii_alphabetic())
            && s.chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || (dot_ok && c == '.'))
    };
    is_ident(module, true) && is_ident(attr, false)
}

/// Does `root/app.py` exist and construct a Flask app (`Flask(`)?
fn flask_app_py(root: &Path) -> bool {
    let app_py = root.join("app.py");
    match std::fs::metadata(&app_py) {
        Ok(meta) if meta.len() <= 256 * 1024 => std::fs::read_to_string(&app_py)
            .map(|c| c.contains("Flask("))
            .unwrap_or(false),
        _ => false,
    }
}

/// Read the merged dependency map from a package.json, if there is one.
fn read_dependencies(root: &Path) -> Option<serde_json::Map<String, serde_json::Value>> {
    let content = std::fs::read_to_string(root.join("package.json")).ok()?;
    let json: serde_json::Value = serde_json::from_str(&content).ok()?;

    let mut merged = serde_json::Map::new();
    for key in ["dependencies", "devDependencies"] {
        if let Some(obj) = json.get(key).and_then(|d| d.as_object()) {
            merged.extend(obj.clone());
        }
    }
    Some(merged)
}

/// Framework relationship types.
#[derive(Debug, Clone)]
pub enum Relation {
    /// If both score high, show only the supersedes one
    Supersedes {
        supersedes: String,
        superseded: String,
    },
    /// Show the implies one as suffix
    Implies { primary: String, secondary: String },
}

/// Registry of framework detectors.
pub struct DetectorRegistry {
    relations: Vec<Relation>,
}

impl Default for DetectorRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl DetectorRegistry {
    pub fn new() -> Self {
        Self {
            relations: vec![
                Relation::Supersedes {
                    supersedes: "nextjs".to_string(),
                    superseded: "react".to_string(),
                },
                Relation::Supersedes {
                    supersedes: "nestjs".to_string(),
                    superseded: "express".to_string(),
                },
                Relation::Supersedes {
                    supersedes: "astro".to_string(),
                    superseded: "vite".to_string(),
                },
                Relation::Implies {
                    primary: "fastapi".to_string(),
                    secondary: "uvicorn".to_string(),
                },
                Relation::Implies {
                    primary: "gunicorn".to_string(),
                    secondary: "wsgi".to_string(),
                },
            ],
        }
    }

    /// Run all detectors and return the best match.
    pub fn detect(
        &self,
        cwd: Option<&Path>,
        exe: Option<&Path>,
        cmdline: &[String],
        port: u16,
    ) -> Option<Framework> {
        let signals = self.collect_signals(cwd, exe, cmdline, port);
        let scores = crate::detect::scoring::score_signals(&signals);

        // Iterating a HashMap is not deterministic, so ties must be broken by
        // something stable or the same process detects differently across runs.
        let (best_id, best_score) = scores
            .iter()
            .filter(|(_, score)| **score >= 0.3)
            .max_by(|a, b| {
                a.1.partial_cmp(b.1)
                    .unwrap_or(std::cmp::Ordering::Equal)
                    .then_with(|| b.0.cmp(a.0))
            })
            .map(|(id, score)| (id.as_str(), *score))?;

        // Check for supersedes relationships
        let display_id = self.resolve_supersedes(best_id, &scores);

        // Build framework from signals
        let framework_signals: Vec<&Signal> = signals
            .iter()
            .filter(|s| s.framework_id == display_id)
            .collect();

        let confidence = crate::detect::scoring::score_to_confidence(best_score);

        Some(Framework {
            id: display_id.to_string(),
            display_name: framework_display_name(display_id),
            version: None,
            language: framework_language(display_id),
            confidence,
            matched_signals: framework_signals
                .iter()
                .map(|s| s.evidence.clone())
                .collect(),
        })
    }

    /// Collect all signals from various sources.
    fn collect_signals(
        &self,
        root: Option<&Path>,
        exe: Option<&Path>,
        cmdline: &[String],
        port: u16,
    ) -> Vec<Signal> {
        let mut signals = Vec::new();
        let cmdline_str = cmdline.join(" ");

        // Interpreter: weak on its own, but it is what separates "some Python
        // thing" from "no idea at all". Distros ship `python3.13`, not `python`.
        if let Some(name) = exe.and_then(|e| e.file_name()).and_then(|n| n.to_str()) {
            let runtime = if matches!(name, "node" | "bun" | "deno") {
                Some("node")
            } else if name.starts_with("python") {
                Some("python")
            } else {
                None
            };

            if let Some(runtime) = runtime {
                signals.push(Signal::new(
                    runtime,
                    kinds::EXE_BASENAME,
                    format!("exe:{name}"),
                ));
            }
        }

        // A script path under node_modules/.bin is near-proof: unlike a bare
        // substring match on "next", it cannot fire on a user named `next`.
        for (id, bin) in NODE_BINARIES {
            let needle = format!("node_modules/.bin/{bin}");
            if cmdline_str.contains(&needle) {
                signals.push(Signal::new(
                    *id,
                    kinds::CMDLINE_SCRIPT_PATH,
                    format!("cmdline_script_path:{needle}"),
                ));
            }
        }

        if cmdline_str.contains("-m uvicorn") || cmdline_str.contains("uvicorn ") {
            // `uvicorn app:app` — an ASGI app target — is the canonical FastAPI
            // dev command. A bare uvicorn with no target is the weaker fallback.
            if cmdline.iter().any(|arg| is_asgi_app_target(arg)) {
                signals.push(Signal::new(
                    "fastapi",
                    kinds::CMDLINE_MODULE,
                    "cmdline:uvicorn asgi-app",
                ));
            } else {
                signals.push(Signal::new(
                    "uvicorn",
                    kinds::CMDLINE_MODULE,
                    "cmdline:uvicorn",
                ));
            }
        }
        if cmdline_str.contains("gunicorn") {
            signals.push(Signal::new(
                "gunicorn",
                kinds::CMDLINE_BINARY,
                "cmdline:gunicorn",
            ));
        }
        if cmdline_str.contains("manage.py") && cmdline_str.contains("runserver") {
            signals.push(Signal::new(
                "django",
                kinds::CMDLINE_ARG,
                "cmdline:manage.py runserver",
            ));
        }
        if cmdline_str.contains("flask") {
            signals.push(Signal::new("flask", kinds::CMDLINE_BINARY, "cmdline:flask"));
        }

        let Some(root) = root else {
            return signals;
        };

        for (id, files) in PROJECT_FILES {
            if let Some(found) = files.iter().find(|f| root.join(f).exists()) {
                signals.push(Signal::new(
                    *id,
                    kinds::PROJECT_FILE,
                    format!("file:{found}"),
                ));
            }
        }

        // Flask is commonly run as `python app.py`, whose cmdline carries no
        // "flask". The tell is an app.py that constructs a Flask app. Guard the
        // read: a project file could be arbitrarily large.
        if !signals.iter().any(|s| s.framework_id == "flask") && flask_app_py(root) {
            signals.push(Signal::new(
                "flask",
                kinds::PROJECT_FILE,
                "file:app.py Flask(",
            ));
        }

        // A declared dependency is the strongest evidence short of the binary.
        if let Some(deps) = read_dependencies(root) {
            for (id, dep) in NODE_DEPENDENCIES {
                if deps.get(*dep).is_some() {
                    signals.push(Signal::new(*id, kinds::MANIFEST_DEP, format!("dep:{dep}")));
                }
            }
        }

        // Port conventions never decide alone; they only break ties.
        if let Some(id) = conventional_port(port) {
            if signals.iter().any(|s| s.framework_id == id) {
                signals.push(Signal::new(
                    id,
                    kinds::PORT_CONVENTION,
                    format!("port:{port}"),
                ));
            }
        }

        signals
    }

    /// Resolve supersedes relationships.
    fn resolve_supersedes<'a>(
        &'a self,
        best_id: &'a str,
        scores: &std::collections::HashMap<String, f64>,
    ) -> &'a str {
        for relation in &self.relations {
            if let Relation::Supersedes {
                supersedes,
                superseded,
            } = relation
            {
                if best_id == superseded.as_str() {
                    // Check if the supersedes framework also has a score
                    if let Some(superseder_score) = scores.get(supersedes) {
                        if *superseder_score >= 0.3 {
                            return supersedes.as_str();
                        }
                    }
                }
            }
        }
        best_id
    }
}

fn framework_display_name(id: &str) -> String {
    match id {
        "nextjs" => "Next.js".to_string(),
        "vite" => "Vite".to_string(),
        "astro" => "Astro".to_string(),
        "nuxt" => "Nuxt".to_string(),
        "remix" => "Remix".to_string(),
        "nestjs" => "NestJS".to_string(),
        "express" => "Express".to_string(),
        "fastapi" => "FastAPI".to_string(),
        "django" => "Django".to_string(),
        "flask" => "Flask".to_string(),
        "uvicorn" => "Uvicorn".to_string(),
        "gunicorn" => "Gunicorn".to_string(),
        "go" => "Go".to_string(),
        "rust" => "Rust".to_string(),
        "node" => "Node".to_string(),
        "python" => "Python".to_string(),
        _ => id.to_string(),
    }
}

fn framework_language(id: &str) -> Language {
    match id {
        "nextjs" | "vite" | "astro" | "nuxt" | "remix" | "nestjs" | "express" | "node" => {
            Language::Node
        }
        "fastapi" | "django" | "flask" | "uvicorn" | "gunicorn" | "python" => Language::Python,
        "go" => Language::Go,
        "rust" => Language::Rust,
        _ => Language::Unknown,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn project(files: &[(&str, &str)]) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        for (name, content) in files {
            std::fs::write(dir.path().join(name), content).unwrap();
        }
        dir
    }

    fn detect(root: &Path, cmdline: &[&str], port: u16) -> Option<Framework> {
        let cmdline: Vec<String> = cmdline.iter().map(|s| s.to_string()).collect();
        let exe = PathBuf::from("/usr/bin/node");
        DetectorRegistry::new().detect(Some(root), Some(&exe), &cmdline, port)
    }

    #[test]
    fn test_next_dev_is_detected_with_high_confidence() {
        let dir = project(&[
            ("package.json", r#"{"dependencies":{"next":"^15.1.0"}}"#),
            ("next.config.ts", ""),
        ]);
        let fw = detect(
            dir.path(),
            &["node", "/w/node_modules/.bin/next", "dev"],
            3000,
        )
        .unwrap();

        assert_eq!(fw.id, "nextjs");
        assert_eq!(fw.confidence, crate::model::FrameworkConfidence::High);
    }

    /// Next.js pulls in React. Both score, but only one is the answer.
    #[test]
    fn test_nextjs_supersedes_react() {
        let mut scores = std::collections::HashMap::new();
        scores.insert("react".to_string(), 0.9);
        scores.insert("nextjs".to_string(), 0.85);

        let registry = DetectorRegistry::new();
        assert_eq!(registry.resolve_supersedes("react", &scores), "nextjs");
    }

    /// The bug this replaces: `cmdline.contains("next")` fired on any path with
    /// "next" in it. Only a real node_modules/.bin path or dep counts.
    #[test]
    fn test_a_path_containing_next_does_not_imply_nextjs() {
        let dir = project(&[("package.json", r#"{"dependencies":{}}"#)]);
        let fw = detect(dir.path(), &["node", "/home/next/server.js"], 9999);
        assert!(fw.is_none() || fw.unwrap().id != "nextjs");
    }

    #[test]
    fn test_dev_dependencies_count_as_evidence() {
        // Vite lives in devDependencies in essentially every real project.
        let dir = project(&[("package.json", r#"{"devDependencies":{"vite":"^5"}}"#)]);
        let fw = detect(dir.path(), &["node", "server.js"], 5173).unwrap();
        assert_eq!(fw.id, "vite");
    }

    #[test]
    fn test_port_convention_never_decides_alone() {
        // Port 3000 with no other Next.js evidence falls back to the runtime,
        // never to Next.js.
        let dir = project(&[("package.json", r#"{"dependencies":{}}"#)]);
        let fw = detect(dir.path(), &["node", "server.js"], 3000).unwrap();
        assert_eq!(fw.id, "node");
        assert_eq!(fw.confidence, crate::model::FrameworkConfidence::Low);
    }

    #[test]
    fn test_detection_is_deterministic_across_runs() {
        let dir = project(&[(
            "package.json",
            r#"{"dependencies":{"vite":"^5","astro":"^4"}}"#,
        )]);
        let first = detect(dir.path(), &["node", "x"], 0).unwrap().id;
        for _ in 0..20 {
            assert_eq!(detect(dir.path(), &["node", "x"], 0).unwrap().id, first);
        }
    }

    /// Distros ship the interpreter as `python3.13`, never as bare `python`.
    #[test]
    fn test_versioned_python_interpreter_is_recognised() {
        let dir = project(&[]);
        let exe = PathBuf::from("/usr/bin/python3.13");
        let cmdline = vec!["python".to_string(), "app.py".to_string()];
        let fw = DetectorRegistry::new()
            .detect(Some(dir.path()), Some(&exe), &cmdline, 8777)
            .unwrap();
        assert_eq!(fw.id, "python");
    }

    /// `uvicorn app:app --reload` is the canonical FastAPI dev command.
    #[test]
    fn test_uvicorn_app_target_is_fastapi() {
        let dir = project(&[]);
        let exe = PathBuf::from("/usr/bin/python3.13");
        let cmdline: Vec<String> = ["python", "-m", "uvicorn", "app:app", "--reload"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let fw = DetectorRegistry::new()
            .detect(Some(dir.path()), Some(&exe), &cmdline, 8000)
            .unwrap();
        assert_eq!(fw.id, "fastapi");
    }

    /// A bare uvicorn with no app target stays generic, and a host:port arg must
    /// not be mistaken for an ASGI target.
    #[test]
    fn test_bare_uvicorn_is_not_fastapi() {
        let dir = project(&[]);
        let exe = PathBuf::from("/usr/bin/python3.13");
        let cmdline: Vec<String> = ["python", "-m", "uvicorn", "--host", "0.0.0.0:8000"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let fw = DetectorRegistry::new()
            .detect(Some(dir.path()), Some(&exe), &cmdline, 8000)
            .unwrap();
        assert_eq!(fw.id, "uvicorn");
    }

    /// Flask run as `python app.py` has no "flask" in its cmdline; the app.py
    /// content is the only tell.
    #[test]
    fn test_flask_detected_from_app_py_contents() {
        let dir = project(&[("app.py", "from flask import Flask\napp = Flask(__name__)\n")]);
        let exe = PathBuf::from("/usr/bin/python3.13");
        let cmdline = vec!["python".to_string(), "app.py".to_string()];
        let fw = DetectorRegistry::new()
            .detect(Some(dir.path()), Some(&exe), &cmdline, 5000)
            .unwrap();
        assert_eq!(fw.id, "flask");
    }

    #[test]
    fn test_unknown_binary_yields_no_framework() {
        let dir = project(&[]);
        let cmdline = vec!["/usr/sbin/sshd".to_string()];
        let exe = PathBuf::from("/usr/sbin/sshd");
        assert!(DetectorRegistry::new()
            .detect(Some(dir.path()), Some(&exe), &cmdline, 22)
            .is_none());
    }
}
