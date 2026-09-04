use super::*;

mod execution_context;
mod file_change_authorization;

pub(super) use execution_context::AutoApprovedActionContext;
pub(super) use file_change_authorization::{
    authorize_file_change_action, file_change_authorization_source,
    validate_file_change_run_grant_at_effect_boundary,
};

mod runners;
mod support;

pub(super) use support::ManualFileEffectSettlement;
use support::*;

include!("action_execution/manual_file_effects.rs");
include!("action_execution/auto_approved.rs");
include!("action_execution/recovery_status.rs");
