use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{JobQueue, worker::JobError};

pub const KIND: &str = "maintenance";

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct MaintenancePayload {
    pub retention_days: u32,
}

pub(crate) async fn handle(queue: &dyn JobQueue, payload: &Value) -> Result<(), JobError> {
    let payload: MaintenancePayload =
        serde_json::from_value(payload.clone()).map_err(|_| JobError::InvalidPayload)?;
    if !(1..=3_650).contains(&payload.retention_days) {
        return Err(JobError::InvalidPayload);
    }
    let deleted = queue
        .prune_completed(payload.retention_days)
        .await
        .map_err(|_| JobError::Maintenance)?;
    tracing::info!(
        deleted,
        retention_days = payload.retention_days,
        "completed job retention applied"
    );
    Ok(())
}
