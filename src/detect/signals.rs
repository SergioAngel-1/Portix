/// A signal is a piece of evidence that points to a framework.
#[derive(Debug, Clone)]
pub struct Signal {
    /// Framework this signal points to
    pub framework_id: String,
    /// Weight of this signal (0.0 - 1.0)
    pub weight: f64,
    /// Human-readable evidence description
    pub evidence: String,
}

impl Signal {
    pub fn new(framework_id: impl Into<String>, weight: f64, evidence: impl Into<String>) -> Self {
        Self {
            framework_id: framework_id.into(),
            weight,
            evidence: evidence.into(),
        }
    }
}

/// Known signal types with their typical weights.
pub mod kinds {
    pub const EXE_BASENAME: f64 = 0.3;
    pub const CMDLINE_BINARY: f64 = 0.8;
    pub const CMDLINE_ARG: f64 = 0.7;
    pub const CMDLINE_SCRIPT_PATH: f64 = 0.85;
    pub const CMDLINE_MODULE: f64 = 0.9;
    pub const MANIFEST_DEP: f64 = 0.9;
    pub const PROJECT_FILE: f64 = 0.7;
    pub const PORT_CONVENTION: f64 = 0.1; // Never decides alone
}
