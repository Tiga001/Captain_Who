use super::*;

#[cfg(test)]
mod mcp_lifecycle_tests;

include!("runners/support.rs");
include!("runners/dispatch.rs");
include!("runners/builtin_execution.rs");
include!("runners/mcp_dispatch.rs");
include!("runners/builtin_auto.rs");
include!("runners/mcp_execution.rs");
include!("runners/queue_skill_office.rs");
include!("runners/skill_supervision.rs");
include!("runners/skill_execution.rs");
include!("runners/command_execution.rs");
include!("runners/continuation_failures.rs");
include!("runners/action_continuation.rs");
