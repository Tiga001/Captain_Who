use super::*;
use serde::Serialize;

const PROJECTED_APPROVAL_UNAVAILABLE_MESSAGE: &str = "Approval is unavailable.";
const UNSETTLED_APPROVAL_PREDECESSOR_MESSAGE: &str =
    "前置工具结果尚未完成持久化结算；已拒绝继续该审批，请等待恢复后重试。";

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

pub(super) fn publish_inline_file_write_tool_result(
    notifications: &CoreServerNotificationSender,
    run_id: &str,
    action: &AgentProposedAction,
    decision_status: AgentApprovalDecisionStatus,
    tool_result: &AgentToolResult,
) -> bool {
    let should_publish = matches!(action, AgentProposedAction::FileWrite { .. })
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

impl AgentService {
    fn ensure_approval_predecessors_settled(
        &self,
        successor: &PendingActionRecord,
    ) -> Result<(), String> {
        let frozen_result_call_ids = successor
            .agent_input
            .resume_checkpoint
            .as_ref()
            .map(|checkpoint| {
                checkpoint
                    .conversation_trace_items
                    .iter()
                    .filter_map(|item| match item {
                        ConversationTurnTraceItem::ToolResult { call_id, .. } => {
                            Some(call_id.clone())
                        }
                        _ => None,
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        if self
            .storage
            .pending_agent_action_has_unsettled_predecessor(
                &successor.storage_id,
                &frozen_result_call_ids,
            )?
        {
            return Err(UNSETTLED_APPROVAL_PREDECESSOR_MESSAGE.to_string());
        }
        Ok(())
    }

    pub(crate) fn list_root_projected_approvals(
        &self,
        root_conversation_id: &str,
    ) -> Result<Vec<ProjectedAgentApproval>, String> {
        let root = self
            .collaboration_authorizer
            .authorize_user_conversation_write(root_conversation_id)
            .map_err(|error| error.to_string())?;
        let Some(root) = root else {
            return Ok(Vec::new());
        };
        if root.parent_agent_id.is_some() {
            return Err("Approval projection owner must be a root Agent.".to_string());
        }
        let tree = self
            .collaboration_authorizer
            .visible_tree(&root.agent_id)
            .map_err(|error| error.to_string())?;
        let by_conversation = tree
            .into_iter()
            .filter(|node| node.parent_agent_id.is_some())
            .map(|node| (node.conversation_id.clone(), node))
            .collect::<HashMap<_, _>>();
        let startup_recoverable = self
            .startup_recoverable_mcp_approvals
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clone();
        let pending_actions = self
            .pending_actions
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let mut projected = pending_actions
            .values()
            .filter_map(|record| {
                if record.snapshot.status != PendingActionStatus::Pending
                    && !(record.snapshot.status == PendingActionStatus::Approved
                        && startup_recoverable.contains(&record.storage_id))
                {
                    return None;
                }
                let source = by_conversation.get(record.snapshot.conversation_id.as_deref()?)?;
                Some(ProjectedAgentApproval {
                    approval_id: record.storage_id.clone(),
                    root_agent_id: root.agent_id.clone(),
                    root_conversation_id: root.conversation_id.clone(),
                    source_agent_id: source.agent_id.clone(),
                    source_task_name: source.task_name.clone(),
                    source_task_path: source.task_path.clone(),
                    source_conversation_id: source.conversation_id.clone(),
                    action: record.snapshot.clone(),
                })
            })
            .collect::<Vec<_>>();
        projected.sort_by(|left, right| {
            left.action
                .created_at
                .cmp(&right.action.created_at)
                .then_with(|| left.approval_id.cmp(&right.approval_id))
        });
        Ok(projected)
    }

    /// Routes a root-card decision back to the exact child Run. The UI supplies no child/run/action
    /// identity; all of it is reloaded from the durable framed approval id.
    pub(crate) fn decide_root_projected_approval(
        &self,
        root_conversation_id: &str,
        approval_id: &str,
        decision: ProjectedApprovalDecision,
        message: Option<String>,
        notifications: CoreServerNotificationSender,
    ) -> Result<ProjectedApprovalDecisionResult, String> {
        let approval_id = approval_id.trim();
        if approval_id.is_empty() || approval_id.len() > 512 || approval_id.contains('\0') {
            return Err("Invalid approvalId.".to_string());
        }
        if message
            .as_ref()
            .is_some_and(|message| message.len() > 16 * 1024 || message.contains('\0'))
        {
            return Err("Approval message must be NUL-free and at most 16384 bytes.".to_string());
        }
        // Validate the root before looking up the opaque approval capability. Every miss and
        // tree/project mismatch below deliberately returns the same safe error so this endpoint
        // cannot be used to enumerate approval ids owned by another root.
        self.collaboration_authorizer
            .authorize_root_conversation(root_conversation_id)
            .map_err(|_| PROJECTED_APPROVAL_UNAVAILABLE_MESSAGE.to_string())?;
        let in_memory = self
            .pending_actions
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .get(approval_id)
            .cloned();
        let Some(record) = in_memory else {
            let durable = self
                .storage
                .get_pending_agent_action(approval_id)?
                .ok_or_else(|| PROJECTED_APPROVAL_UNAVAILABLE_MESSAGE.to_string())?;
            let source_conversation_id = durable
                .conversation_id
                .as_deref()
                .ok_or_else(|| PROJECTED_APPROVAL_UNAVAILABLE_MESSAGE.to_string())?;
            let (_, source) = self
                .collaboration_authorizer
                .authorize_root_projection(root_conversation_id, source_conversation_id)
                .map_err(|_| PROJECTED_APPROVAL_UNAVAILABLE_MESSAGE.to_string())?;
            let public_action_id =
                serde_json::from_str::<AgentProposedAction>(&durable.action_json)
                    .map(|action| action_id_for_action(&action))
                    .unwrap_or_else(|_| approval_id.to_string());
            return Ok(ProjectedApprovalDecisionResult {
                approval_id: approval_id.to_string(),
                source_agent_id: source.agent_id,
                run_id: durable.run_id,
                action_id: public_action_id,
                status: durable.status,
                accepted: false,
            });
        };

        let source_conversation_id = record
            .snapshot
            .conversation_id
            .as_deref()
            .ok_or_else(|| PROJECTED_APPROVAL_UNAVAILABLE_MESSAGE.to_string())?;
        let (_, source) = self
            .collaboration_authorizer
            .authorize_root_projection(root_conversation_id, source_conversation_id)
            .map_err(|_| PROJECTED_APPROVAL_UNAVAILABLE_MESSAGE.to_string())?;
        let run_id = record.snapshot.run_id.clone();
        let action_id = record.snapshot.action_id.clone();
        let current = record.snapshot.status;
        let actionable = current == PendingActionStatus::Pending
            || (current == PendingActionStatus::Approved
                && decision == ProjectedApprovalDecision::Approve
                && self
                    .startup_recoverable_mcp_approvals
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .contains(approval_id));
        if !actionable {
            return Ok(ProjectedApprovalDecisionResult {
                approval_id: approval_id.to_string(),
                source_agent_id: source.agent_id,
                run_id,
                action_id,
                status: pending_status_label(current).to_string(),
                accepted: false,
            });
        }

        let result = match decision {
            ProjectedApprovalDecision::Approve => self
                .queue_action_continuation(
                    &run_id,
                    &action_id,
                    AgentApprovalDecisionStatus::Approved,
                    None,
                    notifications,
                )
                .map(|output| (output.status, true)),
            ProjectedApprovalDecision::Reject => self
                .queue_action_continuation(
                    &run_id,
                    &action_id,
                    AgentApprovalDecisionStatus::Rejected,
                    message,
                    notifications,
                )
                .map(|output| (output.status, true)),
            ProjectedApprovalDecision::Cancel => self
                .cancel_action_internal(&run_id, &action_id)
                .map(|cancelled| {
                    (
                        if cancelled { "cancelled" } else { "unchanged" }.to_string(),
                        cancelled,
                    )
                }),
        };
        match result {
            Ok((status, accepted)) => Ok(ProjectedApprovalDecisionResult {
                approval_id: approval_id.to_string(),
                source_agent_id: source.agent_id,
                run_id,
                action_id,
                status,
                accepted,
            }),
            Err(error) => {
                // A simultaneous click may win between the in-memory check and the existing
                // pending-action CAS. Re-read durable state and return an idempotent snapshot.
                let durable = self.storage.get_pending_agent_action(approval_id)?;
                if let Some(durable) = durable.filter(|durable| durable.status != "pending") {
                    return Ok(ProjectedApprovalDecisionResult {
                        approval_id: approval_id.to_string(),
                        source_agent_id: source.agent_id,
                        run_id,
                        action_id,
                        status: durable.status,
                        accepted: false,
                    });
                }
                Err(error)
            }
        }
    }
    /// Host-internal cancellation for one trusted child Wake run. A live Runtime uses the normal
    /// run token; a durable approval has no worker, so its exact action is atomically cancelled
    /// through the existing pending-action settlement path.
    pub(crate) fn interrupt_agent_wake_run(&self, run_id: &str) -> Result<bool, String> {
        if self.cancel_run_internal(run_id) {
            return Ok(true);
        }
        let action_ids = self
            .pending_actions
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .values()
            .filter(|record| record.snapshot.run_id == run_id)
            .filter(|record| {
                matches!(
                    record.snapshot.status,
                    PendingActionStatus::Pending
                        | PendingActionStatus::Approved
                        | PendingActionStatus::Executing
                )
            })
            .map(|record| record.snapshot.action_id.clone())
            .collect::<Vec<_>>();
        for action_id in action_ids {
            if self.cancel_action_internal(run_id, &action_id)? {
                return Ok(true);
            }
        }
        Ok(false)
    }

    pub(super) fn validate_provider_continuations_before_dispatch(
        &self,
        record: &PendingActionRecord,
    ) -> Result<(), String> {
        let Some(checkpoint) = record.agent_input.resume_checkpoint.as_ref() else {
            return Ok(());
        };
        let has_provider_tool_calls = !checkpoint
            .assistant_turn_identity
            .tool_call_identities
            .is_empty();
        checkpoint
            .provider_protocol_key
            .validate_against_config(&checkpoint.provider_profile_config)
            .map_err(|error| {
                format!(
                    "provider_context_boundary_required: frozen Provider profile/key mismatch before approved action dispatch: {error}"
                )
            })?;
        let capabilities = mycopilot_core::resolve_provider_runtime_capabilities(
            &checkpoint.provider_protocol_key,
        )
        .map_err(|error| {
            format!(
                "provider_context_boundary_required: Provider runtime capability unavailable before approved action dispatch: {error}"
            )
        })?;
        let requires_provider_continuation = capabilities
            .classify_turn(
                has_provider_tool_calls,
                !checkpoint.provider_continuation_refs.is_empty(),
                checkpoint.provider_profile_config.reasoning.mode,
            )
            .requires_exact_approval_refs();
        if checkpoint.provider_continuation_refs.is_empty() {
            return if requires_provider_continuation {
                Err("provider_continuation_missing: approved action was not dispatched".to_string())
            } else {
                Ok(())
            };
        }
        let conversation_id = record.snapshot.conversation_id.as_deref().ok_or_else(|| {
            "provider_continuation.invalid_approval_scope: approved action was not dispatched"
                .to_string()
        })?;
        let assistant_message_id = record
            .snapshot
            .assistant_message_id
            .as_deref()
            .filter(|assistant_message_id| !assistant_message_id.trim().is_empty())
            .ok_or_else(|| {
                "provider_continuation.invalid_approval_scope: approved action was not dispatched"
                    .to_string()
            })?;
        let run_id = record.snapshot.run_id.trim();
        if run_id.is_empty() {
            return Err(
                "provider_continuation.invalid_approval_scope: approved action was not dispatched"
                    .to_string(),
            );
        }
        self.provider_continuation_vault
            .as_deref()
            .ok_or_else(|| {
                "provider_continuation.credential_unavailable: approved action was not dispatched"
                    .to_string()
            })?
            .validate_approval_checkpoint_refs(
                conversation_id,
                assistant_message_id,
                run_id,
                &checkpoint.provider_protocol_key,
                &checkpoint.provider_continuation_refs,
                &checkpoint.assistant_turn_identity,
            )
            .map_err(|error| format!("{}: approved action was not dispatched", error.code()))
    }

    pub fn list_pending_actions(&self) -> Vec<PendingAgentActionSnapshot> {
        let pending_actions = self
            .pending_actions
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let startup_recoverable_mcp_approvals = self
            .startup_recoverable_mcp_approvals
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let mut snapshots = pending_actions
            .iter()
            .filter(|(storage_id, record)| {
                record.snapshot.status == PendingActionStatus::Pending
                    || (record.snapshot.status == PendingActionStatus::Approved
                        && startup_recoverable_mcp_approvals.contains(*storage_id))
            })
            .map(|(_, record)| record.snapshot.clone())
            .collect::<Vec<_>>();
        snapshots.sort_by_key(|snapshot| snapshot.created_at);
        snapshots
    }

    /// Legacy renderer approval list. Child approvals are intentionally absent: they are exposed
    /// only through `list_root_projected_approvals`, which carries authenticated source identity.
    pub fn list_user_pending_actions(&self) -> Vec<PendingAgentActionSnapshot> {
        let mut snapshots = self.list_pending_actions();
        if let Some(coordinator) = self.browser_risk_coordinator.as_ref() {
            snapshots.extend(coordinator.list_pending());
            snapshots.sort_by_key(|snapshot| snapshot.created_at);
        }
        snapshots
            .into_iter()
            .filter(|snapshot| {
                let Some(conversation_id) = snapshot.conversation_id.as_deref() else {
                    return true;
                };
                self.collaboration_authorizer
                    .authorize_user_conversation_write(conversation_id)
                    .is_ok()
            })
            .collect()
    }

    pub fn read_file_draft(
        &self,
        draft_id: &str,
        observer_root_conversation_id: Option<&str>,
        offset: Option<usize>,
        max_chars: Option<usize>,
    ) -> Result<AgentFileDraftContentPage, String> {
        let draft = self
            .storage
            .get_agent_file_draft(draft_id)?
            .ok_or_else(|| format!("未找到文件草稿：{draft_id}"))?;
        self.authorize_file_draft_read(observer_root_conversation_id, &draft.conversation_id)?;
        let snapshot = file_draft_snapshot(&draft)?;
        let (content, offset, next_offset, truncated) =
            paginate_chars(&draft.content, offset, max_chars);
        Ok(AgentFileDraftContentPage {
            draft: snapshot,
            content,
            offset,
            next_offset,
            truncated,
        })
    }

    pub fn get_file_write_diff(
        &self,
        draft_id: &str,
        observer_root_conversation_id: Option<&str>,
        offset: Option<usize>,
        max_chars: Option<usize>,
    ) -> Result<AgentFileWriteDiffPage, String> {
        let draft = self
            .storage
            .get_agent_file_draft(draft_id)?
            .ok_or_else(|| format!("未找到文件草稿：{draft_id}"))?;
        self.authorize_file_draft_read(observer_root_conversation_id, &draft.conversation_id)?;
        let diff = file_write_diff(&draft);
        let (patch, offset, next_offset, truncated) = paginate_chars(&diff, offset, max_chars);
        Ok(AgentFileWriteDiffPage {
            draft_id: draft.id,
            patch,
            offset,
            next_offset,
            truncated,
        })
    }

    fn authorize_file_draft_read(
        &self,
        observer_root_conversation_id: Option<&str>,
        draft_conversation_id: &str,
    ) -> Result<(), String> {
        match observer_root_conversation_id {
            Some(root_conversation_id) => self
                .authorize_exact_child_observer_read(root_conversation_id, draft_conversation_id)
                .map(|_| ())
                .map_err(|error| error.to_string()),
            None => self
                .authorize_user_conversation_write(draft_conversation_id)
                .map_err(|error| error.to_string()),
        }
    }

    pub fn approve_action(
        &self,
        run_id: &str,
        action_id: &str,
        notifications: CoreServerNotificationSender,
    ) -> Result<AgentActionExecutionOutput, String> {
        if let Some(coordinator) = self.browser_risk_coordinator.as_ref() {
            if let Some(conversation_id) = coordinator.pending_conversation_id(run_id, action_id) {
                self.collaboration_authorizer
                    .authorize_user_conversation_write(&conversation_id)
                    .map_err(|error| error.to_string())?;
                return coordinator.approve(run_id, action_id)?.ok_or_else(|| {
                    "Browser risk approval changed while the decision was being committed."
                        .to_string()
                });
            }
        }
        self.authorize_user_pending_action(run_id, action_id)?;
        self.queue_action_continuation(
            run_id,
            action_id,
            AgentApprovalDecisionStatus::Approved,
            None,
            notifications,
        )
    }

    pub fn reject_action(
        &self,
        run_id: &str,
        action_id: &str,
        message: Option<String>,
        notifications: CoreServerNotificationSender,
    ) -> Result<AgentActionExecutionOutput, String> {
        if let Some(coordinator) = self.browser_risk_coordinator.as_ref() {
            if let Some(conversation_id) = coordinator.pending_conversation_id(run_id, action_id) {
                self.collaboration_authorizer
                    .authorize_user_conversation_write(&conversation_id)
                    .map_err(|error| error.to_string())?;
                return coordinator
                    .reject(run_id, action_id, message)?
                    .ok_or_else(|| {
                        "Browser risk approval changed while rejection was being committed."
                            .to_string()
                    });
            }
        }
        self.authorize_user_pending_action(run_id, action_id)?;
        self.queue_action_continuation(
            run_id,
            action_id,
            AgentApprovalDecisionStatus::Rejected,
            message,
            notifications,
        )
    }

    pub fn cancel_action(&self, run_id: &str, action_id: &str) -> Result<bool, String> {
        if let Some(coordinator) = self.browser_risk_coordinator.as_ref() {
            if let Some(conversation_id) = coordinator.pending_conversation_id(run_id, action_id) {
                self.collaboration_authorizer
                    .authorize_user_conversation_write(&conversation_id)
                    .map_err(|error| error.to_string())?;
                return coordinator.cancel(run_id, action_id)?.ok_or_else(|| {
                    "Browser risk approval changed while cancellation was being committed."
                        .to_string()
                });
            }
        }
        self.authorize_user_pending_action(run_id, action_id)?;
        self.cancel_action_internal(run_id, action_id)
    }

    fn cancel_action_internal(&self, run_id: &str, action_id: &str) -> Result<bool, String> {
        if self
            .try_cancel_recovered_approved_mcp_action(run_id, action_id)?
            .is_some()
        {
            return Ok(true);
        }
        let deletion_lifecycle = self
            .deletion_lifecycle
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let mut pending_actions = self
            .pending_actions
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let Some(storage_id) =
            resolve_pending_action_storage_id(&pending_actions, run_id, action_id)
        else {
            return Ok(false);
        };
        let record = pending_actions
            .get_mut(&storage_id)
            .expect("resolved pending action exists");
        if deletion_lifecycle.contains_input(&record.agent_input) {
            return Ok(false);
        }
        let is_cancellable_process = matches!(
            record.snapshot.action,
            AgentProposedAction::Command { .. }
                | AgentProposedAction::SkillScript { .. }
                | AgentProposedAction::OfficeOperation { .. }
                | AgentProposedAction::McpToolCall { .. }
                | AgentProposedAction::BuiltinMcpToolApproval { .. }
        );
        let is_mcp_dispatching = record.snapshot.status == PendingActionStatus::Executing
            && matches!(
                record.snapshot.action,
                AgentProposedAction::McpToolCall { .. }
                    | AgentProposedAction::BuiltinMcpToolApproval { .. }
            );
        if is_cancellable_process
            && (record.snapshot.status == PendingActionStatus::Approved || is_mcp_dispatching)
        {
            // Freeze the identity, then release approval/deletion locks before touching a process
            // Session. Session termination performs a bounded wait and must never hold the
            // pending-action or destructive-lifecycle mutexes while doing so.
            let record = record.clone();
            drop(pending_actions);
            drop(deletion_lifecycle);

            // A command which has already returned Running is no longer represented by the
            // legacy process guard. Arbitrate it through the same Session-state fence used by
            // `commit_handoff`; whichever transition wins is authoritative. The legacy flag
            // remains necessary for the approved-to-spawn gap and for non-command processes.
            let cancelled_sessions = self
                .command_sessions
                .cancel_pre_handoff_for_action(&record.snapshot.run_id, &record.snapshot.action_id);
            let cancelled_process = self.process_runs.cancel(&record.storage_id);
            let cancelled = cancelled_sessions > 0 || cancelled_process;
            if cancelled {
                if let Some(token) = self
                    .cancellations
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .get(&record.snapshot.run_id)
                {
                    token.cancel();
                }
                self.record_action_audit(
                    &record,
                    Some("cancelled"),
                    "cancellation_requested",
                    None,
                    None,
                    None,
                    Some("Process execution was cancelled by the user."),
                    Some(now_ms()),
                    None,
                );
            }
            return Ok(cancelled);
        }
        if record.snapshot.status != PendingActionStatus::Pending {
            return Ok(false);
        }
        let call = tool_call_for_pending_record(record)?;
        self.persist_pending_status(
            record,
            PendingActionStatus::Pending,
            PendingActionStatus::Executing,
        )?;
        record.snapshot.status = PendingActionStatus::Executing;
        if let Err(error) = self.storage.set_pending_agent_action_target_status(
            &record.storage_id,
            pending_status_label(PendingActionStatus::Executing),
            pending_status_label(PendingActionStatus::Cancelled),
            now_ms(),
        ) {
            let rollback = self.persist_pending_status(
                record,
                PendingActionStatus::Executing,
                PendingActionStatus::Pending,
            );
            if rollback.is_ok() {
                record.snapshot.status = PendingActionStatus::Pending;
            }
            return match rollback {
                Ok(()) => Err(format!(
                    "待审批操作无法写入 cancelled 目标终态，已安全回滚为 pending：{error}"
                )),
                Err(rollback_error) => Err(format!(
                    "待审批操作无法写入 cancelled 目标终态：{error}；且无法回滚 executing 状态：{rollback_error}"
                )),
            };
        }
        let record = record.clone();
        drop(pending_actions);
        drop(deletion_lifecycle);
        if let AgentProposedAction::SkillInstallation { installation } = &record.snapshot.action {
            let conversation_id = record.snapshot.conversation_id.as_deref().ok_or_else(|| {
                "Skill installation approval has no conversation identity.".to_string()
            })?;
            let service = self
                .skill_installation
                .as_ref()
                .ok_or_else(|| "Skill installation Host is unavailable.".to_string())?;
            service
                .reject_action(installation, conversation_id, &record.snapshot.run_id)
                .map_err(|error| error.to_string())?;
        }
        if let Err(error) = self.finalize_cancelled_pending_action(&record, call) {
            if cancelled_file_write_outcome_is_durable_or_unknown(&self.storage, &record) {
                return Err(format!(
                    "{error} 文件草稿的拒绝结果已经持久化或无法安全判定；待审批操作保持 executing 并保留 cancelled 目标终态，等待启动对账，禁止重新执行。"
                ));
            }
            if let Err(rollback_error) =
                self.transition_pending_status(&record, PendingActionStatus::Pending)
            {
                return Err(format!(
                    "{error} 此外，待审批操作无法回滚为 pending；已保持 executing 状态并保留 cancelled 目标终态，等待启动对账：{rollback_error}"
                ));
            }
            return Err(error);
        }
        self.invalidate_mcp_pending_payload(&record.snapshot.action);
        self.transition_pending_status(&record, PendingActionStatus::Cancelled)?;
        // `finalize_cancelled_pending_action` committed the assistant/trace terminal state and
        // the pending-action CAS above committed the matching action terminal state. Only after
        // both durable facts exist may the in-memory accelerator release this logical Turn.
        if let Some(conversation_id) = record.snapshot.conversation_id.as_deref() {
            self.release_conversation_turn_if_current(conversation_id, &record.snapshot.run_id);
            self.release_turn_concurrency_permit(&record.snapshot.run_id);
        }
        Ok(true)
    }

    /// Atomically cancels an MCP approval which was durable `approved` across a process restart
    /// but has not crossed the `executing` dispatch boundary.
    ///
    /// The pending-action mutex serializes decisions inside this Host, while the SQLite status CAS
    /// is authoritative across stale/restarted Host instances. Rejection takes the normal
    /// ToolResult continuation path; cancellation intentionally terminates the run here.
    fn try_cancel_recovered_approved_mcp_action(
        &self,
        run_id: &str,
        action_id: &str,
    ) -> Result<Option<AgentActionExecutionOutput>, String> {
        let deletion_lifecycle = self
            .deletion_lifecycle
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let mut pending_actions = self
            .pending_actions
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let Some(storage_id) =
            resolve_pending_action_storage_id(&pending_actions, run_id, action_id)
        else {
            return Ok(None);
        };
        let Some(record) = pending_actions.get(&storage_id) else {
            return Ok(None);
        };
        let is_recovered_approved = record.snapshot.status == PendingActionStatus::Approved
            && matches!(
                record.snapshot.action,
                AgentProposedAction::McpToolCall { .. }
            )
            && self
                .startup_recoverable_mcp_approvals
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .contains(&storage_id);
        if !is_recovered_approved {
            return Ok(None);
        }
        if deletion_lifecycle.contains_input(&record.agent_input) {
            return Err("项目或会话正在移除，无法处理恢复的 MCP 审批。".to_string());
        }
        let record = record.clone();
        let changed = self
            .storage
            .terminalize_mcp_agent_action_on_startup(
                &storage_id,
                pending_status_label(PendingActionStatus::Approved),
                McpStartupActionTerminalOutcome::Cancelled,
                self.mcp_approval_now_ms(),
            )
            .map_err(|_| "Recovered MCP approval could not be terminalized safely.".to_string())?;
        if !changed {
            return Err(
                "Recovered MCP approval changed while the decision was being committed."
                    .to_string(),
            );
        }
        pending_actions.remove(&storage_id);
        self.startup_recoverable_mcp_approvals
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .remove(&storage_id);
        drop(pending_actions);
        drop(deletion_lifecycle);

        self.invalidate_mcp_pending_payload(&record.snapshot.action);
        if let Some(conversation_id) = record.snapshot.conversation_id.as_deref() {
            self.release_conversation_turn_if_current(conversation_id, &record.snapshot.run_id);
            self.release_turn_concurrency_permit(&record.snapshot.run_id);
        }
        Ok(Some(AgentActionExecutionOutput {
            action_id: record.snapshot.action_id,
            action_type: record.snapshot.action_type,
            tool_name: record.snapshot.tool_name,
            status: pending_status_label(PendingActionStatus::Cancelled).to_string(),
            patch_result: None,
            file_write_result: None,
            command_result: None,
            // Renderer learns the terminal MCP state from the typed lifecycle event above and
            // the approval status. A generic ToolResult would create a second result channel
            // whose payload contract is neither necessary nor safe for an external Server.
            tool_result: None,
            agent_output: AgentChatOutput {
                content: String::new(),
                status: AgentRunStatus::Cancelled,
                run_id: record.snapshot.run_id,
                events: Vec::new(),
                tool_definitions: Vec::new(),
                todo: None,
                usage: None,
                finish_reason: None,
                proposed_actions: Vec::new(),
                conversation_turn_trace: None,
            },
        }))
    }

    pub(super) fn finalize_cancelled_pending_action(
        &self,
        record: &PendingActionRecord,
        mut call: AgentToolCall,
    ) -> Result<(), String> {
        const REASON: &str = "Pending action was cancelled by the user.";
        call.approval_status = AgentApprovalStatus::Rejected;
        let execution = if let AgentProposedAction::BuiltinMcpToolApproval { approval } =
            &record.snapshot.action
        {
            ActionExecutionDecision {
                status: "cancelled".to_string(),
                final_pending_status: PendingActionStatus::Cancelled,
                patch_result: None,
                file_write_result: None,
                file_change: None,
                tool_result: mycopilot_core::builtin_mcp_tool_cancelled_result(approval),
            }
        } else {
            action_execution_for_decision(
                &self.storage,
                record,
                &call,
                AgentApprovalDecisionStatus::Rejected,
                Some(REASON),
            )
        };
        let completed_at = now_ms();
        if let (Some(conversation_id), Some(assistant_message_id), Some(checkpoint)) = (
            record.snapshot.conversation_id.as_deref(),
            record.snapshot.assistant_message_id.as_deref(),
            record.agent_input.resume_checkpoint.as_ref(),
        ) {
            let model_observation = project_persisted_continuation_observation(
                &record.agent_input.model,
                &record.agent_input.api_url,
                record.agent_input.api_style,
                &execution.tool_result,
                &Default::default(),
            )
            .map_err(|error| error.to_string())?;
            let terminal = cancelled_conversation_trace_from_checkpoint(
                checkpoint,
                conversation_id,
                assistant_message_id,
                &call,
                &execution.tool_result,
                &model_observation,
                REASON,
            )?;
            let usage_record = self.prepare_run_usage_record(
                &record.snapshot.run_id,
                AgentRunStatus::Cancelled,
                None,
                Some(REASON.to_string()),
            );
            self.storage
                .finalize_chat_message_with_conversation_trace_model_context_and_usage(
                    conversation_id,
                    assistant_message_id,
                    "",
                    status_for_run(AgentRunStatus::Cancelled),
                    run_status_label(AgentRunStatus::Cancelled),
                    &terminal.trace,
                    Some(&terminal.model_context_items),
                    completed_at,
                    completed_at,
                    usage_record.as_ref(),
                )?;
            self.finish_persisted_run_usage(&record.snapshot.run_id, AgentRunStatus::Cancelled);
            self.invalidate_conversation_context_state(conversation_id);
            self.discard_trace_snapshot(&record.snapshot.run_id);
        }
        self.record_action_audit(
            record,
            Some("cancelled"),
            "cancelled",
            execution.patch_result.as_ref(),
            None,
            Some(&execution.tool_result),
            Some(REASON),
            Some(completed_at),
            Some(completed_at),
        );
        Ok(())
    }

    /// Commits the exact pre-dispatch rejection receipt and resolves a commit-unknown response
    /// from durable state. Every retry and inspection uses the same timestamp so the audit and
    /// trace identity remain byte-for-byte stable.
    pub(super) fn commit_rejected_mcp_receipt(
        &self,
        record: &PendingActionRecord,
        persisted_agent_input: &AgentChatInput,
        completed_at: i64,
        notifications: &CoreServerNotificationSender,
    ) -> Result<(), String> {
        for _ in 0..2 {
            if self
                .commit_rejected_mcp_result_trace_with_continuation(
                    record,
                    persisted_agent_input,
                    completed_at,
                    notifications,
                )
                .is_ok()
            {
                return Ok(());
            }
        }

        match self.inspect_rejected_mcp_result_trace_with_continuation(
            record,
            persisted_agent_input,
            completed_at,
        ) {
            Ok(AgentPendingActionSettlementInspection::CommittedAtBoundary) => Ok(()),
            Ok(AgentPendingActionSettlementInspection::CommittedAndAdvanced) => Err(
                "MCP rejection receipt was already advanced by another continuation; duplicate continuation was stopped."
                    .to_string(),
            ),
            Ok(AgentPendingActionSettlementInspection::DefinitelyUncommitted) => Err(
                "MCP rejection receipt was not committed; continuation was stopped.".to_string(),
            ),
            Ok(AgentPendingActionSettlementInspection::Diverged { component, reason }) => Err(
                format!(
                    "MCP rejection receipt diverged at {component}; continuation was stopped: {reason}"
                ),
            ),
            Err(_) => Err(
                "MCP rejection receipt could not be inspected safely; continuation was stopped."
                    .to_string(),
            ),
        }
    }

    /// Elects the single Host continuation after the durable rejection receipt exists.
    ///
    /// The receipt transaction already owns `target_status=rejected`. This final status CAS is
    /// therefore both the same-process single-flight guard and the cross-Host arbitration point:
    /// only its winner may clear the payload, publish lifecycle, or spawn a model continuation.
    fn claim_rejected_mcp_continuation(&self, record: &PendingActionRecord) -> Result<(), String> {
        let mut pending_actions = self
            .pending_actions
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let pending = pending_actions
            .get_mut(&record.storage_id)
            .ok_or_else(|| "MCP rejection no longer has a pending action.".to_string())?;
        let current = pending.snapshot.status;
        let is_recovered_approved = current == PendingActionStatus::Approved
            && self
                .startup_recoverable_mcp_approvals
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .contains(&record.storage_id);
        if current != PendingActionStatus::Pending && !is_recovered_approved {
            return Err(
                "MCP rejection was already settled by another approval decision.".to_string(),
            );
        }
        self.persist_pending_status(pending, current, PendingActionStatus::Rejected)
            .map_err(|_| {
                "MCP rejection lost its durable status arbitration; continuation was stopped."
                    .to_string()
            })?;
        pending.snapshot.status = PendingActionStatus::Rejected;
        if is_recovered_approved {
            self.startup_recoverable_mcp_approvals
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .remove(&record.storage_id);
        }
        Ok(())
    }

    pub(super) fn queue_action_continuation(
        &self,
        run_id: &str,
        action_id: &str,
        decision_status: AgentApprovalDecisionStatus,
        message: Option<String>,
        notifications: CoreServerNotificationSender,
    ) -> Result<AgentActionExecutionOutput, String> {
        #[cfg(test)]
        run_approval_decision_barrier_hook(action_id, decision_status);
        if decision_status == AgentApprovalDecisionStatus::Approved {
            if let Some(output) =
                self.try_resume_approved_mcp_action(run_id, action_id, notifications.clone())?
            {
                return Ok(output);
            }
        }
        let mut deletion_lifecycle = Some(
            self.deletion_lifecycle
                .lock()
                .unwrap_or_else(|error| error.into_inner()),
        );
        let decided_at = now_ms();
        let (
            record,
            call,
            approved_process_guard,
            approved_materialization_guard,
            inline_continuation_guard,
            precommitted_builtin_rejection,
        ) = {
            let mut pending_actions = self
                .pending_actions
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            let Some(storage_id) =
                resolve_pending_action_storage_id(&pending_actions, run_id, action_id)
            else {
                return Err(format!(
                    "未找到待审批操作：runId={run_id}, actionId={action_id}"
                ));
            };
            let record = pending_actions
                .get(&storage_id)
                .expect("resolved pending action exists");
            let is_rejected_mcp = decision_status == AgentApprovalDecisionStatus::Rejected
                && matches!(
                    record.snapshot.action,
                    AgentProposedAction::McpToolCall { .. }
                        | AgentProposedAction::BuiltinMcpToolApproval { .. }
                );
            let is_recovered_approved_mcp_rejection = is_rejected_mcp
                && record.snapshot.status == PendingActionStatus::Approved
                && self
                    .startup_recoverable_mcp_approvals
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .contains(&storage_id);
            if record.snapshot.status != PendingActionStatus::Pending
                && !is_recovered_approved_mcp_rejection
            {
                return Err(format!("待审批操作已经处理：{action_id}"));
            }
            self.ensure_approval_predecessors_settled(record)?;
            let record = pending_actions
                .get_mut(&storage_id)
                .expect("resolved pending action exists");
            if deletion_lifecycle
                .as_ref()
                .expect("deletion lifecycle guard is held while preparing approval")
                .contains_input(&record.agent_input)
            {
                return Err("项目或会话正在移除，无法处理待审批操作。".to_string());
            }
            if decision_status == AgentApprovalDecisionStatus::Approved {
                if let AgentProposedAction::McpToolCall { approval } = &record.snapshot.action {
                    let now = self.mcp_approval_now_ms();
                    if approval.expires_at <= now {
                        let retired = self
                            .storage
                            .terminalize_mcp_agent_action_on_startup(
                                &record.storage_id,
                                "pending",
                                McpStartupActionTerminalOutcome::Expired,
                                now,
                            )
                            .map_err(|_| {
                                "MCP approval expiry could not be persisted safely.".to_string()
                            })?;
                        if !retired {
                            return Err("MCP approval changed while its expiry was being settled."
                                .to_string());
                        }
                        record.snapshot.status = PendingActionStatus::Failed;
                        self.invalidate_mcp_pending_payload(&record.snapshot.action);
                        return Err(
                            "MCP approval expired before dispatch; the tool was not invoked."
                                .to_string(),
                        );
                    }
                }
                if let AgentProposedAction::BuiltinMcpToolApproval { approval } =
                    &record.snapshot.action
                {
                    let now_ms = self.mcp_approval_now_ms();
                    let now_seconds = u64::try_from(now_ms.max(0)).unwrap_or_default() / 1_000;
                    if approval.expires_at <= now_seconds {
                        let retired = self
                            .storage
                            .terminalize_builtin_mcp_tool_agent_action_on_startup(
                                &record.storage_id,
                                pending_status_label(record.snapshot.status),
                                McpStartupActionTerminalOutcome::Expired,
                                now_ms,
                            )
                            .map_err(|_| {
                                "Built-in MCP Tool approval expiry could not be persisted safely."
                                    .to_string()
                            })?;
                        if !retired {
                            return Err(
                                "Built-in MCP Tool approval changed while its expiry was being settled."
                                    .to_string(),
                            );
                        }
                        record.snapshot.status = PendingActionStatus::Failed;
                        let expired_record = record.clone();
                        pending_actions.remove(&storage_id);
                        self.startup_recoverable_mcp_approvals
                            .lock()
                            .unwrap_or_else(|error| error.into_inner())
                            .remove(&storage_id);
                        drop(pending_actions);
                        drop(deletion_lifecycle.take());
                        self.invalidate_mcp_pending_payload(&expired_record.snapshot.action);
                        if let Some(conversation_id) =
                            expired_record.snapshot.conversation_id.as_deref()
                        {
                            self.release_conversation_turn_if_current(
                                conversation_id,
                                &expired_record.snapshot.run_id,
                            );
                            self.release_turn_concurrency_permit(&expired_record.snapshot.run_id);
                        }
                        return Err(
                            "Built-in MCP Tool approval expired before dispatch; the Tool was not invoked."
                                .to_string(),
                        );
                    }
                }
                if let Err(error) = self.validate_provider_continuations_before_dispatch(record) {
                    // Publish the durable terminal intent before replacing the resume payload.
                    // A crash between these writes leaves a non-dispatchable pending row that
                    // startup reconciliation can finish; it can never fall back to approval.
                    self.storage.set_pending_agent_action_target_status(
                        &record.storage_id,
                        pending_status_label(PendingActionStatus::Pending),
                        pending_status_label(PendingActionStatus::Cancelled),
                        now_ms(),
                    )?;
                    self.persist_pending_status(
                        record,
                        PendingActionStatus::Pending,
                        PendingActionStatus::Cancelled,
                    )?;
                    record.snapshot.status = PendingActionStatus::Cancelled;
                    self.invalidate_mcp_pending_payload(&record.snapshot.action);
                    self.record_action_audit(
                        record,
                        Some("blocked"),
                        "cancelled",
                        None,
                        None,
                        None,
                        Some("Provider continuation validation failed before dispatch."),
                        Some(now_ms()),
                        Some(now_ms()),
                    );
                    return Err(error);
                }
            }
            let call = tool_call_for_pending_record(record)?;
            // A sensitive built-in rejection must win the durable decision CAS before its
            // process-only payload is mutated. Otherwise an approve thread can transition the
            // same pending row while Host rejection removes the frozen invocation, producing a
            // split-brain approval. Commit the value-free rejection ToolResult/trace first; the
            // target_status=rejected receipt makes every concurrent approve transition fail.
            let precommitted_builtin_rejection =
                if decision_status == AgentApprovalDecisionStatus::Rejected {
                    if let AgentProposedAction::BuiltinMcpToolApproval { approval } =
                        &record.snapshot.action
                    {
                        let tool_result = mycopilot_core::builtin_mcp_tool_rejected_result(
                            approval,
                            message.as_deref(),
                        );
                        let mut live_agent_input = record.agent_input.clone();
                        live_agent_input.approval_decision = Some(AgentApprovalDecision {
                            action_id: record.snapshot.action_id.clone(),
                            status: decision_status,
                            message: message.clone(),
                        });
                        live_agent_input.tool_continuation = Some(AgentToolContinuation {
                            call: call.clone(),
                            result: tool_result.clone(),
                        });
                        let mut persisted_agent_input = live_agent_input.clone();
                        if let Some(decision) = persisted_agent_input.approval_decision.as_mut() {
                            // User feedback is process-only model context. It can contain a
                            // password or other unlabelled secret and is never needed to prove
                            // the durable decision/ToolResult identity.
                            decision.message = None;
                        }
                        persisted_agent_input
                            .tool_continuation
                            .as_mut()
                            .expect("built-in rejection continuation was just populated")
                            .result =
                            mycopilot_core::builtin_capability_tool_result_persistence_projection(
                                &tool_result,
                            );
                        let completed_at = now_ms();
                        self.commit_rejected_mcp_receipt(
                            record,
                            &persisted_agent_input,
                            completed_at,
                            &notifications,
                        )?;
                        Some((live_agent_input, tool_result))
                    } else {
                        None
                    }
                } else {
                    None
                };
            if decision_status == AgentApprovalDecisionStatus::Approved {
                authorize_structured_file_write(
                    &record.agent_input,
                    &record.snapshot.action,
                    FileWriteAuthorizationSource::ExplicitUser,
                )
                .map_err(|error| error.to_string())?;
            }
            let is_approved_process = decision_status == AgentApprovalDecisionStatus::Approved
                && matches!(
                    record.snapshot.action,
                    AgentProposedAction::Command { .. }
                        | AgentProposedAction::SkillScript { .. }
                        | AgentProposedAction::OfficeOperation { .. }
                        | AgentProposedAction::McpToolCall { .. }
                        | AgentProposedAction::BuiltinMcpToolApproval { .. }
                );
            let is_approved_skill_script = decision_status == AgentApprovalDecisionStatus::Approved
                && matches!(
                    record.snapshot.action,
                    AgentProposedAction::SkillScript { .. }
                );
            let is_approved_materialization = decision_status
                == AgentApprovalDecisionStatus::Approved
                && matches!(
                    record.snapshot.action,
                    AgentProposedAction::SkillMaterialization { .. }
                );
            let approved_process_guard = is_approved_process.then(|| {
                self.process_runs
                    .register(&record.storage_id, &record.snapshot.run_id)
            });
            let approved_materialization_guard = is_approved_materialization.then(|| {
                // The shared lifecycle lock is held, so direct registration is atomic with the
                // project/conversation tombstone check above and cannot race deletion.
                self.file_effects.register(
                    agent_input_project_id(&record.agent_input),
                    agent_input_conversation_id(&record.agent_input),
                    &record.snapshot.run_id,
                    &record.storage_id,
                )
            });
            let inline_continuation_guard = (!is_approved_process && !is_rejected_mcp).then(|| {
                // Synchronous decisions still queue an async model continuation. Register its
                // pre-spawn lease under the lifecycle lock so message deletion cannot pass
                // between durable action settlement and worker startup.
                self.process_runs
                    .register(&record.storage_id, &record.snapshot.run_id)
            });
            let execution_status = if is_approved_skill_script {
                PendingActionStatus::Executing
            } else if is_approved_process || is_approved_materialization {
                PendingActionStatus::Approved
            } else if is_rejected_mcp {
                // Rejection is definitely pre-dispatch. Keep either the original `pending`
                // state or a startup-recovered `approved` state until the paired audit,
                // target and ToolResult receipt commit atomically. `executing` remains reserved
                // for the conservative external-dispatch boundary.
                record.snapshot.status
            } else {
                PendingActionStatus::Executing
            };
            if execution_status != record.snapshot.status {
                if is_approved_skill_script {
                    let approved_audit = action_audit_record(
                        record,
                        Some("approved"),
                        "approved",
                        None,
                        None,
                        None,
                        None,
                        Some(decided_at),
                        None,
                    );
                    let active_child_wake = record
                        .agent_input
                        .context
                        .as_ref()
                        .and_then(|context| context.collaboration_identity.as_ref())
                        .map(|identity| {
                            ChildAgentFactory::new(Arc::clone(&self.storage))
                                .resolve_trusted_active_wake_by_identity(identity)
                                .map_err(|error| {
                                    format!("子 Agent 审批无法取得原 Wake 的执行权：{error}")
                                })
                        })
                        .transpose()?;
                    let child_wake = active_child_wake.as_ref().map(|active| {
                        (
                            active.spawn.initial_wake.wake_id.as_str(),
                            active.spawn.initial_wake.status,
                            active.claim_token.as_str(),
                        )
                    });
                    self.storage
                        .commit_pending_skill_script_approval_execution(
                            &record.storage_id,
                            &persisted_pending_agent_input_json(
                                &record.agent_input,
                                PendingActionStatus::Executing,
                            )?,
                            &approved_audit,
                            child_wake,
                            decided_at,
                        )?;
                } else {
                    self.persist_pending_status(
                        record,
                        PendingActionStatus::Pending,
                        execution_status,
                    )?;
                }
                record.snapshot.status = execution_status;
            }
            (
                record.clone(),
                call,
                approved_process_guard,
                approved_materialization_guard,
                inline_continuation_guard,
                precommitted_builtin_rejection,
            )
        };
        let mut approved_materialization_guard = approved_materialization_guard;
        let mut inline_continuation_guard = inline_continuation_guard;

        let mut call = call;
        let is_mcp_action = matches!(
            record.snapshot.action,
            AgentProposedAction::McpToolCall { .. }
        );
        let is_builtin_mcp_action = matches!(
            record.snapshot.action,
            AgentProposedAction::BuiltinMcpToolApproval { .. }
        );
        if !is_mcp_action && !is_builtin_mcp_action {
            call.approval_status = match decision_status {
                AgentApprovalDecisionStatus::Approved => AgentApprovalStatus::Approved,
                AgentApprovalDecisionStatus::Rejected => AgentApprovalStatus::Rejected,
            };
        }
        if decision_status == AgentApprovalDecisionStatus::Rejected
            && !is_mcp_action
            && !is_builtin_mcp_action
        {
            self.invalidate_mcp_pending_payload(&record.snapshot.action);
        }
        if decision_status == AgentApprovalDecisionStatus::Approved
            && matches!(record.snapshot.action, AgentProposedAction::Command { .. })
        {
            self.record_action_audit(
                &record,
                Some("approved"),
                "approved",
                None,
                None,
                None,
                None,
                Some(decided_at),
                None,
            );
            drop(deletion_lifecycle);
            return self.queue_command_execution(
                record,
                call,
                approved_process_guard.expect("approved command registered under pending lock"),
                notifications,
            );
        }
        if decision_status == AgentApprovalDecisionStatus::Approved
            && matches!(
                record.snapshot.action,
                AgentProposedAction::SkillScript { .. }
            )
        {
            drop(deletion_lifecycle);
            return self.queue_skill_script_execution(
                record,
                call,
                approved_process_guard
                    .expect("approved Skill script registered under pending lock"),
                notifications,
            );
        }
        if decision_status == AgentApprovalDecisionStatus::Approved
            && matches!(
                record.snapshot.action,
                AgentProposedAction::OfficeOperation { .. }
            )
        {
            self.record_action_audit(
                &record,
                Some("approved"),
                "approved",
                None,
                None,
                None,
                None,
                Some(decided_at),
                None,
            );
            drop(deletion_lifecycle);
            return self.queue_office_operation_execution(
                record,
                call,
                approved_process_guard
                    .expect("approved Office operation registered under pending lock"),
                notifications,
            );
        }
        if decision_status == AgentApprovalDecisionStatus::Approved
            && matches!(
                record.snapshot.action,
                AgentProposedAction::McpToolCall { .. }
            )
        {
            self.record_action_audit(
                &record,
                Some("approved"),
                "approved",
                None,
                None,
                None,
                None,
                Some(decided_at),
                None,
            );
            drop(deletion_lifecycle);
            return self.queue_mcp_tool_execution(
                record,
                call,
                approved_process_guard
                    .expect("approved MCP invocation registered under pending lock"),
                notifications,
            );
        }
        if decision_status == AgentApprovalDecisionStatus::Approved && is_builtin_mcp_action {
            let AgentProposedAction::BuiltinMcpToolApproval { approval } = &record.snapshot.action
            else {
                unreachable!("typed built-in MCP action was checked above");
            };
            let mut approved = (**approval).clone();
            approved.approval_status = AgentApprovalStatus::Approved;
            let runtime = self
                .builtin_capabilities
                .as_ref()
                .ok_or_else(|| "Builtin capability Host is unavailable.".to_string())?;
            let grant = match runtime.approve_builtin_mcp_tool(&approved) {
                Ok(grant) => grant,
                Err(_error) => {
                    let completed_at = now_ms();
                    let terminalized = self
                        .storage
                        .terminalize_builtin_mcp_tool_agent_action_on_startup(
                            &record.storage_id,
                            pending_status_label(PendingActionStatus::Approved),
                            McpStartupActionTerminalOutcome::PayloadUnavailable,
                            completed_at,
                        )
                        .map_err(|_| {
                            "Built-in MCP Tool approval could not be terminalized safely."
                                .to_string()
                        })?;
                    if !terminalized {
                        return Err(
                            "Built-in MCP Tool approval changed while process authority was being retired."
                                .to_string(),
                        );
                    }
                    self.invalidate_mcp_pending_payload(&record.snapshot.action);
                    self.pending_actions
                        .lock()
                        .unwrap_or_else(|error| error.into_inner())
                        .remove(&record.storage_id);
                    if let Some(conversation_id) = record.snapshot.conversation_id.as_deref() {
                        self.release_conversation_turn_if_current(
                            conversation_id,
                            &record.snapshot.run_id,
                        );
                        self.release_turn_concurrency_permit(&record.snapshot.run_id);
                    }
                    let tool_result = AgentToolResult {
                        exact_archive_file: None,
                        call_id: approval.identity.call_id.clone(),
                        tool: approval.identity.model_name.clone(),
                        ok: false,
                        result: Some(serde_json::json!({
                            "schemaVersion": 1,
                            "type": "builtin_mcp_tool_approval",
                            "status": "failed",
                            "errorCode": "mcp.tool_approval_payload_unavailable",
                            "dispatchCertainty": "definitely_not_dispatched",
                            "retryable": false,
                            "contentOmitted": true,
                        })),
                        error: Some(
                            "The sensitive built-in MCP Tool was not dispatched because its process authority was unavailable."
                                .to_string(),
                        ),
                    };
                    drop(deletion_lifecycle);
                    return Ok(AgentActionExecutionOutput {
                        action_id: record.snapshot.action_id,
                        action_type: record.snapshot.action_type,
                        tool_name: record.snapshot.tool_name,
                        status: "failed".to_string(),
                        patch_result: None,
                        file_write_result: None,
                        command_result: None,
                        tool_result: Some(tool_result),
                        agent_output: AgentChatOutput {
                            content: String::new(),
                            status: AgentRunStatus::Failed,
                            run_id: record.snapshot.run_id,
                            events: Vec::new(),
                            tool_definitions: Vec::new(),
                            todo: None,
                            usage: None,
                            finish_reason: None,
                            proposed_actions: Vec::new(),
                            conversation_turn_trace: None,
                        },
                    });
                }
            };
            self.record_action_audit(
                &record,
                Some("approved"),
                "approved",
                None,
                None,
                None,
                None,
                Some(decided_at),
                None,
            );
            drop(deletion_lifecycle);
            let queue = self.queue_builtin_mcp_tool_execution(
                record,
                call,
                grant.clone(),
                approved_process_guard
                    .expect("approved built-in MCP invocation registered under pending lock"),
                notifications,
            );
            if queue.is_err() {
                let _ = runtime.revoke_builtin_mcp_tool_grant(&grant.grant_id, &grant.approval_id);
            }
            return queue;
        }

        let is_approved_materialization = decision_status == AgentApprovalDecisionStatus::Approved
            && matches!(
                record.snapshot.action,
                AgentProposedAction::SkillMaterialization { .. }
            );
        if is_approved_materialization {
            // The lease was registered atomically with the deletion-marker check above. Release
            // the global lifecycle mutex before resource restoration and filesystem I/O so a
            // deletion can install its marker and use the bounded FileEffectTracker drain.
            drop(deletion_lifecycle.take());
        }

        let mut continuation_message = message.clone();
        if decision_status == AgentApprovalDecisionStatus::Rejected {
            if let AgentProposedAction::SkillInstallation { installation } = &record.snapshot.action
            {
                let conversation_id =
                    record.snapshot.conversation_id.as_deref().ok_or_else(|| {
                        "Skill installation approval has no conversation identity.".to_string()
                    })?;
                let service = self
                    .skill_installation
                    .as_ref()
                    .ok_or_else(|| "Skill installation Host is unavailable.".to_string())?;
                service
                    .reject_action(installation, conversation_id, &record.snapshot.run_id)
                    .map_err(|error| error.to_string())?;
            }
        }
        let mut builtin_activation_settlement = None;
        let execution = if decision_status == AgentApprovalDecisionStatus::Rejected && is_mcp_action
        {
            let AgentProposedAction::McpToolCall { approval } = &record.snapshot.action else {
                unreachable!("typed MCP action was checked above");
            };
            let tool_result = mcp_tool_result_from_rejected_approval(approval, message.as_deref())
                .map_err(|error| error.to_string())?;
            continuation_message = tool_result
                .result
                .as_ref()
                .and_then(|value| value.get("userFeedback"))
                .and_then(serde_json::Value::as_str)
                .map(ToString::to_string);
            ActionExecutionDecision {
                status: "rejected".to_string(),
                final_pending_status: PendingActionStatus::Rejected,
                patch_result: None,
                file_write_result: None,
                file_change: None,
                tool_result,
            }
        } else if decision_status == AgentApprovalDecisionStatus::Rejected && is_builtin_mcp_action
        {
            let AgentProposedAction::BuiltinMcpToolApproval { approval } = &record.snapshot.action
            else {
                unreachable!("typed built-in MCP action was checked above");
            };
            if let Some(runtime) = self.builtin_capabilities.as_ref() {
                if runtime.reject_builtin_mcp_tool_approval(approval).is_err() {
                    // Durable user rejection is already authoritative. Cleanup is idempotent and
                    // must never turn it into a transport failure or reopen approval authority.
                    let _ = runtime.dismiss_builtin_mcp_tool_approval(approval);
                }
            }
            let tool_result = precommitted_builtin_rejection
                .as_ref()
                .map(|(_, result)| result.clone())
                .unwrap_or_else(|| {
                    mycopilot_core::builtin_mcp_tool_rejected_result(approval, message.as_deref())
                });
            continuation_message = message.clone();
            ActionExecutionDecision {
                status: "rejected".to_string(),
                final_pending_status: PendingActionStatus::Rejected,
                patch_result: None,
                file_write_result: None,
                file_change: None,
                tool_result,
            }
        } else if let AgentProposedAction::BuiltinCapabilityActivation { approval } =
            &record.snapshot.action
        {
            let tool_result = if decision_status == AgentApprovalDecisionStatus::Rejected {
                mycopilot_core::builtin_capability_activation_rejected_result(
                    approval,
                    message.as_deref(),
                )
            } else {
                let mut approved = (**approval).clone();
                approved.approval_status = AgentApprovalStatus::Approved;
                // Waiting for a human decision has no deadline. The timestamp carried by the
                // pending card only bounded the original proposal; mint a fresh, short-lived
                // activation authorization when the user actually approves it. Grant lifetime
                // and all live policy/manifest checks remain unchanged.
                let approved_at =
                    u64::try_from(self.mcp_approval_now_ms().max(0)).unwrap_or_default() / 1_000;
                approved.created_at = approved_at;
                approved.expires_at = approved_at
                    .saturating_add(mycopilot_core::BUILTIN_CAPABILITY_ACTIVATION_TTL_SECONDS);
                match self.builtin_capabilities.as_ref() {
                    Some(runtime) => match runtime.approve_activation(&approved) {
                        Ok(grant) => {
                            builtin_activation_settlement =
                                Some(BuiltinActivationSettlementGuard::new(
                                    runtime.clone(),
                                    grant.activation_id,
                                    approved.action_id.clone(),
                                ));
                            mycopilot_core::builtin_capability_activation_result(
                                &approved,
                                mycopilot_core::CapabilityActivationState::Active,
                                None,
                            )
                        }
                        Err(error) => AgentToolResult {
                            exact_archive_file: None,
                            call_id: approved.call_id.clone(),
                            tool: "activate_capability".to_string(),
                            ok: false,
                            result: Some(serde_json::json!({
                                "status": "revoked",
                                "capability": approved.capability_id,
                            })),
                            error: Some(error.to_string()),
                        },
                    },
                    None => AgentToolResult {
                        exact_archive_file: None,
                        call_id: approved.call_id.clone(),
                        tool: "activate_capability".to_string(),
                        ok: false,
                        result: Some(serde_json::json!({
                            "status": "revoked",
                            "capability": approved.capability_id,
                        })),
                        error: Some("Builtin capability Host is unavailable.".to_string()),
                    },
                }
            };
            continuation_message = if decision_status == AgentApprovalDecisionStatus::Rejected {
                message.clone()
            } else {
                None
            };
            ActionExecutionDecision {
                status: if decision_status == AgentApprovalDecisionStatus::Rejected {
                    "rejected".to_string()
                } else if tool_result.ok {
                    // Keep the transport-level action status within the established approval
                    // vocabulary. The capability-specific ToolResult carries `status=active`;
                    // adding another wire enum here would create a second source of truth.
                    "approved".to_string()
                } else {
                    "failed".to_string()
                },
                final_pending_status: if decision_status == AgentApprovalDecisionStatus::Rejected {
                    PendingActionStatus::Rejected
                } else if tool_result.ok {
                    PendingActionStatus::Completed
                } else {
                    PendingActionStatus::Failed
                },
                patch_result: None,
                file_write_result: None,
                file_change: None,
                tool_result,
            }
        } else if decision_status == AgentApprovalDecisionStatus::Approved {
            if let AgentProposedAction::SkillInstallation { installation } = &record.snapshot.action
            {
                let mut installation = (**installation).clone();
                installation.approval_status = AgentApprovalStatus::Approved;
                let conversation_id =
                    record.snapshot.conversation_id.as_deref().ok_or_else(|| {
                        "Skill installation approval has no conversation identity.".to_string()
                    })?;
                let service = self
                    .skill_installation
                    .as_ref()
                    .ok_or_else(|| "Skill installation Host is unavailable.".to_string())?;
                let tool_result = service.commit_approved(
                    &installation,
                    conversation_id,
                    &record.snapshot.run_id,
                );
                ActionExecutionDecision {
                    status: if tool_result.ok {
                        "installed".to_string()
                    } else {
                        "failed".to_string()
                    },
                    final_pending_status: if tool_result.ok {
                        PendingActionStatus::Completed
                    } else {
                        PendingActionStatus::Failed
                    },
                    patch_result: None,
                    file_write_result: None,
                    file_change: None,
                    tool_result,
                }
            } else if let AgentProposedAction::SkillMaterialization { materialization } =
                &record.snapshot.action
            {
                let mut materialization = materialization.clone();
                materialization.approval_status = AgentApprovalStatus::Approved;
                let tool_result = match self.restore_skill_resource_session(&record.agent_input) {
                    Ok(resources) => {
                        approved_materialization_guard
                            .as_mut()
                            .expect(
                                "approved Skill materialization registered under lifecycle lock",
                            )
                            .mark_effects_started();
                        self.execute_skill_materialization(
                            &record.agent_input,
                            &materialization,
                            resources.as_deref(),
                        )
                    }
                    Err(error) => AgentToolResult {
                        exact_archive_file: None,
                        call_id: materialization.id.clone(),
                        tool: "skills_materialize_resource".to_string(),
                        ok: false,
                        result: Some(serde_json::json!({
                            "type": "skill_materialization",
                            "code": "snapshotUnavailable",
                            "recovery": "reactivateSkill",
                        })),
                        error: Some(error.to_string()),
                    },
                };
                ActionExecutionDecision {
                    status: if tool_result.ok {
                        "applied".to_string()
                    } else {
                        "failed".to_string()
                    },
                    final_pending_status: if tool_result.ok {
                        PendingActionStatus::Completed
                    } else {
                        PendingActionStatus::Failed
                    },
                    patch_result: None,
                    file_write_result: None,
                    file_change: None,
                    tool_result,
                }
            } else {
                action_execution_for_decision(
                    &self.storage,
                    &record,
                    &call,
                    decision_status,
                    message.as_deref(),
                )
            }
        } else {
            action_execution_for_decision(
                &self.storage,
                &record,
                &call,
                decision_status,
                message.as_deref(),
            )
        };
        let mut final_pending_status = execution.final_pending_status;
        let mut tool_result = execution.tool_result.clone();
        let mut execution_status = execution.status.clone();
        if decision_status == AgentApprovalDecisionStatus::Approved
            && matches!(
                record.snapshot.action,
                AgentProposedAction::SkillInstallation { .. }
            )
        {
            let commit_may_have_succeeded = tool_result
                .result
                .as_ref()
                .and_then(|value| value.get("commitMayHaveSucceeded"))
                .and_then(Value::as_bool)
                .unwrap_or(false);
            if tool_result.ok || commit_may_have_succeeded {
                if let (Some(installations), Some(workflow)) = (
                    self.skill_installation_service.as_deref(),
                    self.skill_installation_workflow.as_deref(),
                ) {
                    crate::transport::notify_skills_changed(
                        &self.storage,
                        &self.skills,
                        installations,
                        Some(workflow),
                        Some(&notifications),
                        if tool_result.ok {
                            mycopilot_protocol_rs::SkillsChangedReasonDto::Installed
                        } else {
                            mycopilot_protocol_rs::SkillsChangedReasonDto::CatalogChanged
                        },
                        None,
                    );
                }
            }
        }
        let is_rejected_mcp = decision_status == AgentApprovalDecisionStatus::Rejected
            && (is_mcp_action || is_builtin_mcp_action);
        let agent_input = if is_rejected_mcp {
            let (agent_input, needs_commit) = match precommitted_builtin_rejection.as_ref() {
                Some((agent_input, _)) => (agent_input.clone(), false),
                None => {
                    let mut agent_input = record.agent_input.clone();
                    agent_input.approval_decision = Some(AgentApprovalDecision {
                        action_id: record.snapshot.action_id.clone(),
                        status: decision_status,
                        message: continuation_message.clone(),
                    });
                    agent_input.tool_continuation = Some(AgentToolContinuation {
                        call: call.clone(),
                        result: tool_result.clone(),
                    });
                    (agent_input, true)
                }
            };
            if needs_commit {
                let mut persisted_agent_input = agent_input.clone();
                if is_builtin_mcp_action {
                    if let Some(decision) = persisted_agent_input.approval_decision.as_mut() {
                        decision.message = None;
                    }
                }
                if let Some(continuation) = persisted_agent_input.tool_continuation.as_mut() {
                    continuation.result = if is_builtin_mcp_action {
                        mycopilot_core::builtin_capability_tool_result_persistence_projection(
                            &continuation.result,
                        )
                    } else {
                        mycopilot_core::mcp_tool_result_persistence_projection(&continuation.result)
                    };
                }
                self.commit_rejected_mcp_receipt(
                    &record,
                    &persisted_agent_input,
                    now_ms(),
                    &notifications,
                )?;
            }
            self.claim_rejected_mcp_continuation(&record)?;
            // Only the durable status-CAS winner owns the pre-spawn lease. Registering before
            // arbitration would let a duplicate reject overwrite the winner's action-keyed guard
            // and unregister it when the losing guard drops.
            inline_continuation_guard = Some(
                self.process_runs
                    .register(&record.storage_id, &record.snapshot.run_id),
            );
            self.invalidate_mcp_pending_payload(&record.snapshot.action);
            if let AgentProposedAction::McpToolCall { approval } = &record.snapshot.action {
                if let Ok(invocation) = mcp_tool_invocation_event(
                    approval,
                    McpToolInvocationEventUpdate {
                        state: AgentMcpToolInvocationState::Rejected,
                        dispatch_certainty: AgentMcpDispatchCertainty::DefinitelyNotDispatched,
                        outcome: Some(AgentMcpToolInvocationOutcome::Rejected),
                        is_error: None,
                        error_code: Some("mcp.approval_rejected"),
                        duration_ms: None,
                        output_truncated: false,
                        result_size: None,
                        failure_stage: None,
                    },
                ) {
                    let _ = notifications.send(agent_event_notification(
                        AgentEvent::McpToolInvocationStateChanged {
                            run_id: record.snapshot.run_id.clone(),
                            invocation,
                        },
                    ));
                }
            }
            agent_input
        } else if is_approved_materialization {
            match self.settle_manual_file_effect(
                &record,
                &call,
                final_pending_status,
                tool_result,
                "skill_materialization",
                &notifications,
            ) {
                ManualFileEffectSettlement::Committed {
                    agent_input,
                    tool_result: settled_result,
                    pending_status,
                } => {
                    final_pending_status = pending_status;
                    tool_result = settled_result;
                    execution_status = if tool_result.ok {
                        "applied".to_string()
                    } else {
                        "failed".to_string()
                    };
                    approved_materialization_guard
                        .as_mut()
                        .expect("approved Skill materialization has an effect guard")
                        .mark_durably_settled();
                    *agent_input
                }
                ManualFileEffectSettlement::CommittedAndAdvanced => {
                    approved_materialization_guard
                        .as_mut()
                        .expect("approved Skill materialization has an effect guard")
                        .mark_durably_settled();
                    return Err(
                        "Skill materialization receipt was already advanced by another continuation; duplicate continuation was stopped."
                            .to_string(),
                    );
                }
                ManualFileEffectSettlement::Unsettled => {
                    return Err(
                        "Skill materialization finished without a confirmed durable terminal receipt; inspect state before retrying."
                            .to_string(),
                    );
                }
            }
        } else {
            self.record_action_audit(
                &record,
                Some(match decision_status {
                    AgentApprovalDecisionStatus::Approved => "approved",
                    AgentApprovalDecisionStatus::Rejected => "rejected",
                }),
                &execution.status,
                execution.patch_result.as_ref(),
                None,
                Some(&tool_result),
                tool_result.error.as_deref(),
                Some(decided_at),
                Some(now_ms()),
            );
            self.persist_pending_target_status(&record, final_pending_status)?;

            let mut agent_input = record.agent_input.clone();
            agent_input.approval_decision = Some(AgentApprovalDecision {
                action_id: record.snapshot.action_id.clone(),
                status: decision_status,
                message: continuation_message.clone(),
            });
            agent_input.tool_continuation = Some(AgentToolContinuation {
                call: call.clone(),
                result: tool_result.clone(),
            });
            let mut persisted_agent_input = agent_input.clone();
            if is_builtin_mcp_action {
                if let Some(continuation) = persisted_agent_input.tool_continuation.as_mut() {
                    continuation.result =
                        mycopilot_core::builtin_capability_tool_result_persistence_projection(
                            &continuation.result,
                        );
                }
            }
            self.commit_trace_snapshot_with_continuation(
                &record,
                &persisted_agent_input,
                &notifications,
            )?;
            agent_input
        };
        if let Some(settlement) = builtin_activation_settlement.take() {
            // From this point the pending target and paired continuation are durably committed.
            // Before this boundary the guard removes the process-memory grant on every error path.
            settlement.commit();
        }
        let run_id = record.snapshot.run_id.clone();
        let inline_continuation_guard = inline_continuation_guard
            .expect("every synchronous approval decision owns a pre-spawn continuation lease");
        let continuation_cancel_flag = inline_continuation_guard.cancel_flag();
        let continuation_cancellation = AgentCancellationToken::new();
        self.register_cancellation(&run_id, continuation_cancellation.clone());
        // Inline diff/file-write actions keep the shared lifecycle lock until their target status
        // and paired ToolResult trace are durable. Materialization and asynchronous process paths
        // use a FileEffectGuard, so deletion can observe and drain them without an unbounded mutex
        // wait.
        drop(approved_materialization_guard);
        drop(deletion_lifecycle);
        let publish_lifecycle = self
            .deletion_lifecycle
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if !publish_lifecycle.contains_input(&record.agent_input)
            && publish_inline_file_write_tool_result(
                &notifications,
                &record.snapshot.run_id,
                &record.snapshot.action,
                decision_status,
                &tool_result,
            )
        {
            // Approved Office operations return earlier and publish exactly one ToolResult from
            // their asynchronous executor. Rejected Office actions reach this synchronous path,
            // so publish the paired rejection result here just like write_file.
            if matches!(
                record.snapshot.action,
                AgentProposedAction::FileWrite { .. }
            ) {
                if let Some(file_write_result) = execution.file_write_result.as_ref() {
                    if let Ok(Some(draft)) = self
                        .storage
                        .get_agent_file_draft(&file_write_result.draft_id)
                    {
                        if let Ok(snapshot) = file_draft_snapshot(&draft) {
                            let _ = notifications.send(agent_event_notification(
                                AgentEvent::FileDraftUpdated {
                                    run_id: record.snapshot.run_id.clone(),
                                    draft: snapshot,
                                },
                            ));
                        }
                    }
                }
            }
        }
        drop(publish_lifecycle);

        let service = self.clone();
        let continuation_record = record.clone();
        tokio::spawn(async move {
            // Deletion may have cancelled the pre-spawn lease before this task was first polled.
            // Translate that signal without consulting the lifecycle mutex so the worker can
            // release its lease while deletion holds the marker and waits for a bounded drain.
            if continuation_cancel_flag.load(Ordering::SeqCst) {
                continuation_cancellation.cancel();
            }
            service
                .run_action_continuation(
                    continuation_record,
                    agent_input,
                    notifications,
                    final_pending_status,
                    Some(continuation_cancellation),
                )
                .await;
            drop(inline_continuation_guard);
        });

        let renderer_tool_result = if matches!(
            record.snapshot.action,
            AgentProposedAction::McpToolCall { .. }
        ) {
            None
        } else {
            Some(tool_result)
        };
        Ok(AgentActionExecutionOutput {
            action_id: record.snapshot.action_id,
            action_type: record.snapshot.action_type,
            tool_name: record.snapshot.tool_name,
            status: execution_status,
            patch_result: execution.patch_result,
            file_write_result: execution.file_write_result,
            command_result: None,
            // MCP has a dedicated approval/lifecycle contract. Keep the generic result response
            // for built-ins and runtime extensions only.
            tool_result: renderer_tool_result,
            agent_output: AgentChatOutput {
                content: String::new(),
                status: AgentRunStatus::Running,
                run_id,
                events: Vec::new(),
                tool_definitions: Vec::new(),
                todo: None,
                usage: None,
                finish_reason: None,
                proposed_actions: Vec::new(),
                conversation_turn_trace: None,
            },
        })
    }

    /// Explicitly resumes an MCP approval which was durably `approved` but had not crossed the
    /// dispatch boundary before restart.
    ///
    /// The same public approve API is intentionally reused: a second explicit user action performs
    /// complete current Host/catalog/payload revalidation, claims the durable dispatch boundary,
    /// and only then queues execution. Concurrent attempts lose the `approved -> executing` CAS.
    fn try_resume_approved_mcp_action(
        &self,
        run_id: &str,
        action_id: &str,
        notifications: CoreServerNotificationSender,
    ) -> Result<Option<AgentActionExecutionOutput>, String> {
        let deletion_lifecycle = self
            .deletion_lifecycle
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let mut pending_actions = self
            .pending_actions
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let Some(storage_id) =
            resolve_pending_action_storage_id(&pending_actions, run_id, action_id)
        else {
            return Ok(None);
        };
        let record = pending_actions
            .get(&storage_id)
            .expect("resolved pending action exists");
        let is_recoverable_approved_mcp = record.snapshot.status == PendingActionStatus::Approved
            && matches!(
                record.snapshot.action,
                AgentProposedAction::McpToolCall { .. }
            )
            && self
                .startup_recoverable_mcp_approvals
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .contains(&storage_id);
        if !is_recoverable_approved_mcp {
            return Ok(None);
        }
        self.ensure_approval_predecessors_settled(record)?;
        let record = pending_actions
            .get_mut(&storage_id)
            .expect("resolved pending action exists");
        if deletion_lifecycle.contains_input(&record.agent_input) {
            return Err("项目或会话正在移除，无法恢复 MCP 操作。".to_string());
        }
        if let Err(error) = self.validate_provider_continuations_before_dispatch(record) {
            let retired = self
                .storage
                .terminalize_mcp_agent_action_on_startup(
                    &record.storage_id,
                    "approved",
                    McpStartupActionTerminalOutcome::PayloadUnavailable,
                    self.mcp_approval_now_ms(),
                )
                .map_err(|_| {
                    "Recovered MCP approval could not be retired before dispatch.".to_string()
                })?;
            if !retired {
                return Err(
                    "Recovered MCP approval changed while Provider continuation validation was being settled."
                        .to_string(),
                );
            }
            record.snapshot.status = PendingActionStatus::Failed;
            self.startup_recoverable_mcp_approvals
                .lock()
                .unwrap_or_else(|lock_error| lock_error.into_inner())
                .remove(&storage_id);
            self.invalidate_mcp_pending_payload(&record.snapshot.action);
            return Err(error);
        }
        let AgentProposedAction::McpToolCall { approval } = &record.snapshot.action else {
            unreachable!("MCP action was checked above");
        };
        let now = self.mcp_approval_now_ms();
        if approval.expires_at <= now {
            let retired = self
                .storage
                .terminalize_mcp_agent_action_on_startup(
                    &record.storage_id,
                    "approved",
                    McpStartupActionTerminalOutcome::Expired,
                    now,
                )
                .map_err(|_| "MCP approval expiry could not be persisted safely.".to_string())?;
            if !retired {
                return Err("MCP approval changed while its expiry was being settled.".to_string());
            }
            record.snapshot.status = PendingActionStatus::Failed;
            self.startup_recoverable_mcp_approvals
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .remove(&storage_id);
            self.invalidate_mcp_pending_payload(&record.snapshot.action);
            return Err(
                "MCP approval expired before dispatch; the tool was not invoked.".to_string(),
            );
        }
        let invoker = self
            .mcp_tool_invoker
            .as_ref()
            .ok_or_else(|| "MCP invocation Host is unavailable.".to_string())?;
        invoker
            .revalidate_approved(approval)
            .map_err(|error| error.to_string())?;
        // Preserve the frozen Provider ToolCall exactly as it appeared in the checkpoint.
        // Approval is carried by the typed action journal and lifecycle, not by rewriting the
        // append-only call from `required` to `approved` after restart.
        let call = tool_call_for_pending_record(record)?;
        self.persist_pending_status(
            record,
            PendingActionStatus::Approved,
            PendingActionStatus::Executing,
        )?;
        record.snapshot.status = PendingActionStatus::Executing;
        self.startup_recoverable_mcp_approvals
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .remove(&storage_id);
        let guard = self
            .process_runs
            .register(&record.storage_id, &record.snapshot.run_id);
        let record = record.clone();
        drop(pending_actions);
        drop(deletion_lifecycle);
        self.queue_claimed_mcp_tool_execution(record, call, guard, notifications)
            .map(Some)
    }
}

pub(super) fn paginate_chars(
    value: &str,
    offset: Option<usize>,
    max_chars: Option<usize>,
) -> (String, usize, Option<usize>, bool) {
    let offset = offset.unwrap_or(0);
    let limit = max_chars.unwrap_or(50_000).clamp(1_000, 100_000);
    let total = value.chars().count();
    let start = offset.min(total);
    let end = start.saturating_add(limit).min(total);
    let content = value.chars().skip(start).take(end - start).collect();
    let truncated = end < total;
    (content, start, truncated.then_some(end), truncated)
}
