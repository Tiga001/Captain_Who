use serde_json::{Map, Value};

pub(super) fn project_checkpoint_activity(
    projection: &mut Map<String, Value>,
    safe_event: &Value,
    now: i64,
) -> bool {
    mycopilot_core::storage::service::project_checkpoint_activity(projection, safe_event, now)
}
