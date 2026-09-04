// The validation tree stays private so domain-specific helpers do not become repository APIs.
mod validation;

pub(crate) use validation::{
    canonical_agent_run_lifecycle_projection, current_agent_run_projection_is_safe,
    current_agent_run_projection_is_safe_for_trace_rebuild,
};
#[cfg(test)]
pub(super) use validation::{
    current_persisted_approval_is_safe, current_uuid_is_safe, MAX_JS_SAFE_INTEGER,
};
