use serde::Serialize;

/// Container runtime detected.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum Runtime {
    Docker,
    Podman,
    Containerd,
    SystemdNspawn,
    Unknown,
}

impl Runtime {
    pub fn display(&self) -> &str {
        match self {
            Runtime::Docker => "Docker",
            Runtime::Podman => "Podman",
            Runtime::Containerd => "containerd",
            Runtime::SystemdNspawn => "systemd-nspawn",
            Runtime::Unknown => "container",
        }
    }

    pub fn glyph(&self) -> &str {
        // Docker gets its whale; the other runtimes share a neutral box.
        match self {
            Runtime::Docker => "🐳",
            _ => "📦",
        }
    }
}

/// A detected container.
#[derive(Debug, Clone, Serialize)]
pub struct Container {
    /// Container runtime
    pub runtime: Runtime,
    /// Container ID (short, 12 chars)
    pub id: String,
    /// Container name (if available).
    pub name: Option<String>,
    /// Docker Compose project name (if available).
    pub compose_project: Option<String>,
    /// Command reported by Docker Desktop for the container.
    pub command: Option<String>,
    /// Host-side process responsible for the published-port forwarder.
    pub host_process: Option<String>,
}

impl Container {
    /// A container with no optional metadata.
    pub fn new(runtime: Runtime, id: String) -> Self {
        Self {
            runtime,
            id,
            name: None,
            compose_project: None,
            command: None,
            host_process: None,
        }
    }

    /// Get display string for the container column.
    pub fn display(&self) -> String {
        match &self.name {
            Some(name) => format!("{} {}", self.runtime.glyph(), name),
            // Char-based truncation: byte slicing could panic if the id ever
            // carried a multi-byte character (today it is hex or an IP).
            None => format!(
                "{} {}",
                self.runtime.glyph(),
                self.id.chars().take(12).collect::<String>()
            ),
        }
    }
}
