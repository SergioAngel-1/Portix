use crate::model::{Snapshot, Service};
use std::sync::Arc;

/// Build a snapshot from collected data.
pub fn build_snapshot(
    services: Vec<Service>,
    scan_duration: std::time::Duration,
) -> Arc<Snapshot> {
    Arc::new(Snapshot {
        services,
        taken_at: std::time::Instant::now(),
        scan_duration_ms: scan_duration.as_millis() as u64,
        degredations: Vec::new(),
    })
}
