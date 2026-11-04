//! Conversation storage tests with shared projection fixtures and SQL tracing.

use super::*;
use std::cell::RefCell;

mod automation_parent_deletion;
mod checked_project_membership;
mod running_summaries;

mod support;
pub(super) use support::trace_storage_selects;
use support::*;

mod composer_drafts;
mod deletion;
mod durable_activity;
mod fork_active_commands;
mod fork_history;
mod fork_reopen;
mod projection_consistency;
mod snapshot_fences;
mod turn_admission;
