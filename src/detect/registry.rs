use crate::detect::signals::Signal;
use crate::model::{Framework, FrameworkConfidence, Language};
use std::path::Path;

/// Framework relationship types.
#[derive(Debug, Clone)]
pub enum Relation {
    /// If both score high, show only the supersedes one
    Supersedes { supersedes: String, superseded: String },
    /// Show the implies one as suffix
    Implies { primary: String, secondary: String },
}

/// Registry of framework detectors.
pub struct DetectorRegistry {
    relations: Vec<Relation>,
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

        if scores.is_empty() {
            return None;
        }

        // Find the best scoring framework
        let mut best: Option<(&str, f64)> = None;
        for (fw_id, score) in &scores {
            if *score >= 0.3 {
                if best.is_none() || *score > best.unwrap().1 {
                    best = Some((fw_id.as_str(), *score));
                }
            }
        }

        let (best_id, best_score) = best?;

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
            matched_signals: framework_signals.iter().map(|s| s.evidence.clone()).collect(),
        })
    }

    /// Collect all signals from various sources.
    fn collect_signals(
        &self,
        cwd: Option<&Path>,
        exe: Option<&Path>,
        cmdline: &[String],
        _port: u16,
    ) -> Vec<Signal> {
        let mut signals = Vec::new();
        let cmdline_str = cmdline.join(" ");

        // Binary name signals
        if let Some(exe) = exe {
            if let Some(name) = exe.file_name().and_then(|n| n.to_str()) {
                match name {
                    "node" | "bun" | "deno" => {
                        signals.push(Signal::new("node", 0.3, format!("exe:{}", name)));
                    }
                    "python" | "python3" => {
                        signals.push(Signal::new("python", 0.3, format!("exe:{}", name)));
                    }
                    _ => {}
                }
            }
        }

        // Cmdline signals for Node.js
        if cmdline_str.contains("node_modules/.bin/next") {
            signals.push(Signal::new(
                "nextjs",
                0.85,
                "cmdline_script_path:node_modules/.bin/next",
            ));
        }
        if cmdline_str.contains("node_modules/.bin/vite") {
            signals.push(Signal::new(
                "vite",
                0.85,
                "cmdline_script_path:node_modules/.bin/vite",
            ));
        }
        if cmdline_str.contains("node_modules/.bin/astro") {
            signals.push(Signal::new(
                "astro",
                0.85,
                "cmdline_script_path:node_modules/.bin/astro",
            ));
        }
        if cmdline_str.contains("node_modules/.bin/nest") {
            signals.push(Signal::new(
                "nestjs",
                0.85,
                "cmdline_script_path:node_modules/.bin/nest",
            ));
        }
        if cmdline_str.contains("-m uvicorn") {
            signals.push(Signal::new(
                "fastapi",
                0.9,
                "cmdline:-m uvicorn",
            ));
        }
        if cmdline_str.contains("manage.py") && cmdline_str.contains("runserver") {
            signals.push(Signal::new(
                "django",
                0.8,
                "cmdline:manage.py runserver",
            ));
        }

        // Project file signals
        if let Some(cwd) = cwd {
            if cwd.join("next.config.js").exists() || cwd.join("next.config.ts").exists() {
                signals.push(Signal::new(
                    "nextjs",
                    0.7,
                    "file:next.config.{js,ts}",
                ));
            }
            if cwd.join("vite.config.js").exists() || cwd.join("vite.config.ts").exists() {
                signals.push(Signal::new(
                    "vite",
                    0.7,
                    "file:vite.config.{js,ts}",
                ));
            }
            if cwd.join("astro.config.mjs").exists() {
                signals.push(Signal::new("astro", 0.7, "file:astro.config.mjs"));
            }
            if cwd.join("manage.py").exists() {
                signals.push(Signal::new("django", 0.7, "file:manage.py"));
            }

            // Dependency signals
            if let Ok(content) = std::fs::read_to_string(cwd.join("package.json")) {
                if let Ok(json) = serde_json::from_str::<serde_json::Value>(&content) {
                    if let Some(deps) = json.get("dependencies") {
                        if deps.get("next").is_some() {
                            signals.push(Signal::new(
                                "nextjs",
                                0.9,
                                "dep:next",
                            ));
                        }
                        if deps.get("vite").is_some() {
                            signals.push(Signal::new("vite", 0.9, "dep:vite"));
                        }
                        if deps.get("astro").is_some() {
                            signals.push(Signal::new("astro", 0.9, "dep:astro"));
                        }
                    }
                }
            }
        }

        signals
    }

    /// Resolve supersedes relationships.
    fn resolve_supersedes<'a>(&'a self, best_id: &'a str, scores: &std::collections::HashMap<String, f64>) -> &'a str {
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
