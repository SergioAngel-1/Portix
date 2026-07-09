use crate::model::{Framework, FrameworkConfidence, Language};
use std::path::Path;

/// Detect the framework from process and project context.
pub fn detect_framework(
    cwd: Option<&Path>,
    exe: Option<&Path>,
    cmdline: &[String],
) -> Option<Framework> {
    let binary = exe
        .and_then(|e| e.file_name())
        .and_then(|n| n.to_str())
        .or_else(|| cmdline.first().map(|s| s.as_str()))?;

    match binary {
        // Node.js ecosystem
        "node" | "bun" | "deno" => detect_node_framework(cwd, cmdline),

        // Python ecosystem
        "python" | "python3" | "uvicorn" | "gunicorn" => {
            detect_python_framework(cwd, cmdline)
        }

        // Go binaries - can't detect framework from binary alone
        _ => {
            // Check if there's a go.mod or Cargo.toml in the project
            if let Some(cwd) = cwd {
                if cwd.join("go.mod").exists() {
                    return Some(Framework {
                        id: "go".to_string(),
                        display_name: "Go".to_string(),
                        version: None,
                        language: Language::Go,
                        confidence: FrameworkConfidence::Medium,
                        matched_signals: vec!["file:go.mod".to_string()],
                    });
                }
                if cwd.join("Cargo.toml").exists() {
                    return Some(Framework {
                        id: "rust".to_string(),
                        display_name: "Rust".to_string(),
                        version: None,
                        language: Language::Rust,
                        confidence: FrameworkConfidence::Medium,
                        matched_signals: vec!["file:Cargo.toml".to_string()],
                    });
                }
            }
            None
        }
    }
}

/// Detect Node.js framework from cmdline and project files.
fn detect_node_framework(cwd: Option<&Path>, cmdline: &[String]) -> Option<Framework> {
    let cmdline_str = cmdline.join(" ");
    let cwd = cwd?;

    // Check for common Node.js frameworks
    let frameworks = vec![
        ("next", "Next.js", "next.config"),
        ("vite", "Vite", "vite.config"),
        ("astro", "Astro", "astro.config"),
        ("nuxt", "Nuxt", "nuxt.config"),
        ("remix", "Remix", "remix.config"),
        ("nest", "NestJS", "nest-cli.json"),
        ("express", "Express", "package.json"),
    ];

    let mut best_match: Option<Framework> = None;
    let mut best_score = 0.0;

    for (cmd_pattern, name, config_pattern) in &frameworks {
        let mut score = 0.0;
        let mut signals = Vec::new();

        // Check cmdline
        if cmdline_str.contains(cmd_pattern) {
            score += 0.8;
            signals.push(format!("cmdline:{}", cmd_pattern));
        }

        // Check for .bin/ in cmdline (very reliable for Node)
        if cmdline_str.contains(&format!("node_modules/.bin/{}", cmd_pattern)) {
            score += 0.85;
            signals.push(format!("cmdline_script_path:node_modules/.bin/{}", cmd_pattern));
        }

        // Check config file
        if !config_pattern.ends_with("json") {
            // Check for config files with various extensions
            for ext in &[".js", ".ts", ".mjs", ".cjs"] {
                let config_file = format!("{}{}", config_pattern, ext);
                if cwd.join(&config_file).exists() {
                    score += 0.7;
                    signals.push(format!("file:{}", config_file));
                    break;
                }
            }
        } else {
            if cwd.join(config_pattern).exists() {
                score += 0.7;
                signals.push(format!("file:{}", config_pattern));
            }
        }

        // Check package.json for dependency
        if *name == "Express" || *name == "NestJS" {
            if let Ok(content) = std::fs::read_to_string(cwd.join("package.json")) {
                if let Ok(json) = serde_json::from_str::<serde_json::Value>(&content) {
                    let deps_key = match *name {
                        "Express" => "express",
                        "NestJS" => "@nestjs/core",
                        _ => "",
                    };
                    if !deps_key.is_empty() {
                        if let Some(deps) = json.get("dependencies") {
                            if deps.get(deps_key).is_some() {
                                score += 0.9;
                                signals.push(format!("dep:{}", deps_key));
                            }
                        }
                    }
                }
            }
        }

        if score > best_score && score >= 0.3 {
            let confidence = if score >= 0.9 {
                FrameworkConfidence::High
            } else if score >= 0.6 {
                FrameworkConfidence::Medium
            } else {
                FrameworkConfidence::Low
            };

            best_score = score;
            best_match = Some(Framework {
                id: name.to_lowercase().replace(' ', ""),
                display_name: name.to_string(),
                version: None, // TODO: Read from package.json or lockfile
                language: Language::Node,
                confidence,
                matched_signals: signals,
            });
        }
    }

    best_match
}

/// Detect Python framework from cmdline and project files.
fn detect_python_framework(cwd: Option<&Path>, cmdline: &[String]) -> Option<Framework> {
    let cmdline_str = cmdline.join(" ");
    let cwd = cwd?;

    // FastAPI
    if cmdline_str.contains("uvicorn") && cmdline_str.contains("app:app") {
        return Some(Framework {
            id: "fastapi".to_string(),
            display_name: "FastAPI".to_string(),
            version: None,
            language: Language::Python,
            confidence: FrameworkConfidence::High,
            matched_signals: vec!["cmdline:-m uvicorn".to_string()],
        });
    }

    // Django
    if cmdline_str.contains("runserver") || cwd.join("manage.py").exists() {
        return Some(Framework {
            id: "django".to_string(),
            display_name: "Django".to_string(),
            version: None,
            language: Language::Python,
            confidence: FrameworkConfidence::Medium,
            matched_signals: vec!["file:manage.py".to_string()],
        });
    }

    // Flask
    if cmdline_str.contains("flask") || cwd.join("app.py").exists() {
        if let Ok(content) = std::fs::read_to_string(cwd.join("app.py")) {
            if content.contains("Flask(") {
                return Some(Framework {
                    id: "flask".to_string(),
                    display_name: "Flask".to_string(),
                    version: None,
                    language: Language::Python,
                    confidence: FrameworkConfidence::Medium,
                    matched_signals: vec!["file:app.py".to_string()],
                });
            }
        }
    }

    // Generic Python
    Some(Framework {
        id: "python".to_string(),
        display_name: "Python".to_string(),
        version: None,
        language: Language::Python,
        confidence: FrameworkConfidence::Low,
        matched_signals: vec!["exe:python".to_string()],
    })
}
