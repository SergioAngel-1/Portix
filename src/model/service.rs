use crate::model::{Container, Framework, Process, Project, Socket, SocketId};
use serde::Serialize;

/// The central entity of Portix: a listening socket + its process + project + framework.
#[derive(Debug, Clone, Serialize)]
pub struct Service {
    /// Unique identifier
    #[serde(skip)]
    pub id: ServiceId,
    /// The listening socket
    pub socket: Socket,
    /// Process info (may be None if can't resolve)
    pub process: Option<Process>,
    /// Detected project (may be None)
    pub project: Option<Project>,
    /// Detected framework (may be None)
    pub framework: Option<Framework>,
    /// Container info (if running in a container)
    pub container: Option<Container>,
    /// If true, this row was rewritten from a `docker-proxy` process into the
    /// destination container. The original process/cmdline are preserved on
    /// `process` so the UI can still show what really runs on the host.
    #[serde(default)]
    pub docker_proxy_rewritten: bool,
}

/// Unique identifier for a service.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
pub struct ServiceId {
    #[serde(skip)]
    pub socket_id: SocketId,
}

impl ServiceId {
    pub fn from_socket(socket: &Socket) -> Self {
        Self {
            socket_id: SocketId::from(socket),
        }
    }
}

impl Service {
    /// Get the display name for the PROJECT column.
    ///
    /// Java-style package names (`com.mycompany.backend`) are stripped to their
    /// last segment (`backend`) so the meaningful part survives in the table's
    /// limited width. This is a display-only transform; the raw name is still
    /// available on the `Project` struct.
    pub fn project_display(&self) -> String {
        if let Some(ref container) = self.container {
            return container.display();
        }
        if let Some(ref project) = self.project {
            return strip_package_prefix(project.display_name()).to_string();
        }
        "—".to_string()
    }

    /// Get the display name for the FRAMEWORK column.
    ///
    /// A process name is not a framework. When detection finds nothing we say
    /// so; putting `chrome` or `exe` in this column makes the tool look like it
    /// detected something it did not.
    pub fn framework_display(&self) -> String {
        match &self.framework {
            Some(fw) => fw.display(),
            None => "—".to_string(),
        }
    }

    /// Get the display name for the COMMAND column.
    pub fn command_display(&self) -> String {
        match &self.process {
            Some(p) => p.binary_name().to_string(),
            None => "—".to_string(),
        }
    }
}

/// Strip a Java/Android-style package prefix from a project name.
///
/// `com.mycompany.backend` → `backend`
/// `io.github.user.my-app` → `my-app`
///
/// Only fires when there are 3+ dot-separated segments where every segment
/// is purely alphanumeric (plus `-`), so normal names like `my-app` or
/// `my-app.core` are untouched.
fn strip_package_prefix(name: &str) -> &str {
    let dot_count = name.chars().filter(|&c| c == '.').count();
    if dot_count < 3 {
        return name;
    }
    let is_package_like = name
        .split('.')
        .all(|seg| !seg.is_empty() && seg.chars().all(|c| c.is_ascii_alphanumeric() || c == '-'));
    if is_package_like {
        name.rsplit_once('.').map(|(_, tail)| tail).unwrap_or(name)
    } else {
        name
    }
}
