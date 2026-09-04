use super::*;
use mycopilot_protocol_rs::{AgentApprovalScopeDto, AgentFileChangeHistoryDiffRequest};
use serde::Serialize;
use std::sync::{LazyLock, Weak};

const PROJECTED_APPROVAL_UNAVAILABLE_MESSAGE: &str = "Approval is unavailable.";
const UNSETTLED_APPROVAL_PREDECESSOR_MESSAGE: &str =
    "前置工具结果尚未完成持久化结算；已拒绝继续该审批，请等待恢复后重试。";

/// Process-local single-flight for one exact durable approval identity.
///
/// The Pending Action and audit rows remain authoritative across restart. This lock only closes
/// the same-process gap in which two identical RPC requests could both observe `pending` before
/// either one acquires the durable decision CAS. Weak entries avoid retaining completed actions.
static APPROVAL_RETRY_LOCKS: LazyLock<Mutex<HashMap<String, Weak<Mutex<()>>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

enum FileChangeApprovalRetry {
    Proceed,
    Replay(Box<AgentActionExecutionOutput>),
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct InternalActionCancellationOutcome {
    affected: bool,
    turn_termination_confirmed: bool,
}

fn file_change_run_grant_decision_error(
    error: mycopilot_core::storage::service::FileChangeRunGrantServiceError,
) -> AgentServiceError {
    AgentServiceError::structured(
        error.safe_message(),
        serde_json::json!({
            "type": "fileChangeDecision",
            "code": error.code(),
            "outcome": "definitelyNotExecuted",
            "recovery": "retryApproval",
        }),
    )
}

#[cfg(test)]
type ApprovalDecisionBarrierHook = Arc<dyn Fn() + Send + Sync>;

#[cfg(test)]
static APPROVAL_DECISION_BARRIER_HOOKS: Mutex<
    Vec<(
        String,
        AgentApprovalDecisionStatus,
        ApprovalDecisionBarrierHook,
    )>,
> = Mutex::new(Vec::new());

#[cfg(test)]
pub(super) fn install_approval_decision_barrier_hook(
    action_id: &str,
    decision: AgentApprovalDecisionStatus,
    hook: ApprovalDecisionBarrierHook,
) {
    APPROVAL_DECISION_BARRIER_HOOKS
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .push((action_id.to_string(), decision, hook));
}

#[cfg(test)]
fn run_approval_decision_barrier_hook(action_id: &str, decision: AgentApprovalDecisionStatus) {
    let hook = {
        let mut hooks = APPROVAL_DECISION_BARRIER_HOOKS
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        hooks
            .iter()
            .position(|(candidate, candidate_decision, _)| {
                candidate == action_id && *candidate_decision == decision
            })
            .map(|index| hooks.swap_remove(index).2)
    };
    if let Some(hook) = hook {
        hook();
    }
}

struct BuiltinActivationSettlementGuard {
    runtime: BuiltinCapabilityRuntime,
    activation_id: CapabilityActivationId,
    action_id: String,
    committed: bool,
}

impl BuiltinActivationSettlementGuard {
    fn new(
        runtime: BuiltinCapabilityRuntime,
        activation_id: CapabilityActivationId,
        action_id: String,
    ) -> Self {
        Self {
            runtime,
            activation_id,
            action_id,
            committed: false,
        }
    }

    fn commit(mut self) {
        self.committed = true;
    }
}

impl Drop for BuiltinActivationSettlementGuard {
    fn drop(&mut self) {
        if !self.committed
            && self
                .runtime
                .revoke_activation(&self.activation_id, &self.action_id)
                .is_err()
        {
            eprintln!("built-in capability activation rollback failed safely");
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ProjectedAgentApproval {
    /// Backend-framed pending-action identity. This is globally stable and is the only decision
    /// selector accepted from the root card.
    pub(crate) approval_id: String,
    pub(crate) root_agent_id: String,
    pub(crate) root_conversation_id: String,
    pub(crate) source_agent_id: String,
    pub(crate) source_task_name: String,
    pub(crate) source_task_path: String,
    pub(crate) source_conversation_id: String,
    pub(crate) action: PendingAgentActionSnapshot,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ProjectedApprovalDecision {
    Approve,
    Reject,
    Cancel,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ProjectedApprovalDecisionResult {
    pub(crate) approval_id: String,
    pub(crate) source_agent_id: String,
    pub(crate) run_id: String,
    pub(crate) action_id: String,
    pub(crate) status: String,
    /// False means an identical/stale click observed a durable non-pending state. It never starts
    /// a second continuation.
    pub(crate) accepted: bool,
}

pub(super) fn publish_inline_file_change_tool_result(
    notifications: &CoreServerNotificationSender,
    run_id: &str,
    action: &AgentProposedAction,
    decision_status: AgentApprovalDecisionStatus,
    tool_result: &AgentToolResult,
) -> bool {
    let should_publish = matches!(action, AgentProposedAction::FileChange { .. })
        || (decision_status == AgentApprovalDecisionStatus::Rejected
            && matches!(action, AgentProposedAction::OfficeOperation { .. }));
    if !should_publish {
        return false;
    }
    let _ = notifications.send(agent_event_notification(AgentEvent::ToolResult {
        run_id: run_id.to_string(),
        result: tool_result.clone(),
    }));
    true
}
