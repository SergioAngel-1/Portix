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
    pub fn project_display(&self) -> String {
        if let Some(ref container) = self.container {
            return container.display();
        }
        if let Some(ref project) = self.project {
            return project.display_name().to_string();
        }
        "—".to_string()
    }

    /// Get the display name for the FRAMEWORK column.
    pub fn framework_display(&self) -> String {
        match &self.framework {
            Some(fw) => fw.display(),
            None => {
                if let Some(ref process) = self.process {
                    process.binary_name().to_string()
                } else {
                    "—".to_string()
                }
            }
        }
    }

    /// Check if this service has degraded data.
    pub fn has_degradation(&self) -> bool {
        self.process.is_none() || self.project.is_none()
    }
}
