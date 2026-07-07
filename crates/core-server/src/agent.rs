// Rust core-server agent actions and conversation bridge.
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use mycopilot_core::command::{run_approved_command, AgentCommandExecutionResult, CommandRunState};
use mycopilot_core::patch::apply_unified_diff_in_workspace;
use mycopilot_core::storage::models::{
    AgentActionAuditRecord, AgentPromptPreferencesRecord, AgentUsageRecordInsert,
    ChatConversationRecord, ChatMessageAttachmentRecord, ChatMessageRecord, ModelConfigRecord,
    ProjectRecord,
};
use mycopilot_core::storage::service::StorageService;
use mycopilot_core::{
    next_run_id, send_chat_with_events_and_cancellation, AgentApprovalDecision,
    AgentApprovalDecisionStatus, AgentApprovalStatus, AgentCancellationToken, AgentChatInput,
    AgentChatMessage, AgentChatOutput, AgentCommandRequest, AgentDiffProposal, AgentEvent,
    AgentEventEmitter, AgentInputAttachment, AgentInputAttachmentEncoding,
    AgentInputAttachmentKind, AgentPatchResult, AgentPatchResultStatus, AgentPermissions,
    AgentPromptDetailLevel, AgentPromptPreferences, AgentPromptTone, AgentPromptWorkMode,
    AgentProposedAction, AgentRunContext, AgentRunMode, AgentRunStatus, AgentSearchConfig,
    AgentSearchMode, AgentToolCall, AgentToolContinuation, AgentToolResult, AgentUsage,
    AgentUsageClearInput, AgentUsageClearOutput, AgentUsageSummaryInput, AgentUsageSummaryOutput,
    AgentWorkspaceContext,
};
use mycopilot_protocol_rs::AGENT_EVENT_NOTIFICATION_METHOD;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::sync::mpsc::UnboundedSender;

const AGENT_EVENT_NAME: &str = "agent.event";
const THINKING_PLACEHOLDER: &str = "正在思考...";
static ID_COUNTER: AtomicU64 = AtomicU64::new(1);

pub type CoreServerNotificationSender = UnboundedSender<Value>;

#[derive(Clone)]
pub struct AgentService {
    storage: Arc<StorageService>,
    cancellations: Arc<Mutex<HashMap<String, AgentCancellationToken>>>,
    pending_actions: Arc<Mutex<HashMap<String, PendingActionRecord>>>,
    usage_contexts: Arc<Mutex<HashMap<String, AgentRunUsageState>>>,
    command_runs: CommandRunState,
}

impl AgentService {
    pub fn new(storage: Arc<StorageService>) -> Self {
        Self {
            storage,
            cancellations: Arc::new(Mutex::new(HashMap::new())),
            pending_actions: Arc::new(Mutex::new(HashMap::new())),
            usage_contexts: Arc::new(Mutex::new(HashMap::new())),
            command_runs: CommandRunState::default(),
        }
    }

    pub fn start_conversation_turn(
        &self,
        input: AgentConversationTurnInput,
        notifications: CoreServerNotificationSender,
    ) -> Result<AgentConversationTurnOutput, String> {
        let run_id = next_run_id();
        let cancellation_token = AgentCancellationToken::new();
        self.register_cancellation(&run_id, cancellation_token.clone());

        let prepared = match prepare_conversation_turn(&self.storage, input, &run_id) {
            Ok(prepared) => prepared,
            Err(error) => {
                self.unregister_cancellation(&run_id);
                return Err(error);
            }
        };

        self.register_usage_context(&run_id, prepared.usage_context.clone());

        let output = prepared.output.clone();
        let service = self.clone();
        let worker_run_id = run_id.clone();
        let worker_conversation_id = output.conversation_id.clone();
        let worker_assistant_message_id = output.assistant_message_id.clone();
        let pending_agent_input = prepared.agent_input.clone();

        tokio::spawn(async move {
            let emitter_notifications = notifications.clone();
            let emitter_service = service.clone();
            let emitter_conversation_id = worker_conversation_id.clone();
            let emitter_assistant_message_id = worker_assistant_message_id.clone();
            let emitter_agent_input = pending_agent_input.clone();
            let emitter: AgentEventEmitter = Arc::new(move |event| {
                if let AgentEvent::ApprovalRequired { run_id, action } = &event {
                    emitter_service.store_pending_action(
                        run_id,
                        &emitter_conversation_id,
                        &emitter_assistant_message_id,
                        action.clone(),
                        emitter_agent_input.clone(),
                    );
                }
                let _ = emitter_notifications.send(agent_event_notification(event));
            });

            let result = send_chat_with_events_and_cancellation(
                prepared.agent_input,
                worker_run_id.clone(),
                emitter,
                cancellation_token,
            )
            .await;

            match result {
                Ok(agent_output) => {
                    let _ = service.persist_final_assistant_output(
                        &worker_conversation_id,
                        &worker_assistant_message_id,
                        &agent_output,
                    );
                }
                Err(error) => {
                    let message = error.to_string();
                    let _ = service.persist_assistant_error(
                        &worker_conversation_id,
                        &worker_assistant_message_id,
                        &message,
                    );
                    let _ = notifications.send(agent_event_notification(AgentEvent::Error {
                        run_id: Some(worker_run_id.clone()),
                        message: message.clone(),
                        recoverable: false,
                    }));
                    let _ = notifications.send(agent_event_notification(AgentEvent::Done {
                        run_id: worker_run_id.clone(),
                        success: false,
                        status: Some(AgentRunStatus::Failed),
                        content: Some(message),
                        usage: None,
                        finish_reason: None,
                        proposed_actions: Vec::new(),
                    }));
                }
            }

            service.unregister_cancellation(&worker_run_id);
        });

        Ok(output)
    }

    pub fn cancel_run(&self, run_id: &str) -> bool {
        let cancellations = self
            .cancellations
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let cancelled_run = if let Some(token) = cancellations.get(run_id) {
            token.cancel();
            true
        } else {
            false
        };
        let cancelled_commands = self.command_runs.cancel_run(run_id);
        cancelled_run || cancelled_commands > 0
    }

    pub fn list_pending_actions(&self) -> Vec<PendingAgentActionSnapshot> {
        let pending_actions = self
            .pending_actions
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let mut snapshots = pending_actions
            .values()
            .filter(|record| record.snapshot.status == PendingActionStatus::Pending)
            .map(|record| record.snapshot.clone())
            .collect::<Vec<_>>();
        snapshots.sort_by_key(|snapshot| snapshot.created_at);
        snapshots
    }

    pub fn approve_action(
        &self,
        action_id: &str,
        notifications: CoreServerNotificationSender,
    ) -> Result<AgentActionExecutionOutput, String> {
        self.queue_action_continuation(
            action_id,
            PendingActionStatus::Approved,
            AgentApprovalDecisionStatus::Approved,
            None,
            notifications,
        )
    }

    pub fn reject_action(
        &self,
        action_id: &str,
        message: Option<String>,
        notifications: CoreServerNotificationSender,
    ) -> Result<AgentActionExecutionOutput, String> {
        self.queue_action_continuation(
            action_id,
            PendingActionStatus::Rejected,
            AgentApprovalDecisionStatus::Rejected,
            message,
            notifications,
        )
    }

    pub fn cancel_action(&self, action_id: &str) -> Result<bool, String> {
        let mut pending_actions = self
            .pending_actions
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let Some(record) = pending_actions.get_mut(action_id) else {
            return Ok(false);
        };
        if record.snapshot.status == PendingActionStatus::Approved
            && matches!(record.snapshot.action, AgentProposedAction::Command { .. })
        {
            let cancelled = self.command_runs.cancel(action_id);
            if cancelled {
                record.snapshot.status = PendingActionStatus::Cancelled;
                self.record_action_audit(
                    record,
                    Some("cancelled"),
                    "cancelled",
                    None,
                    None,
                    None,
                    Some("Command execution was cancelled by the user."),
                    Some(now_ms()),
                    Some(now_ms()),
                );
            }
            return Ok(cancelled);
        }
        if record.snapshot.status != PendingActionStatus::Pending {
            return Ok(false);
        }
        record.snapshot.status = PendingActionStatus::Cancelled;
        self.record_action_audit(
            record,
            Some("cancelled"),
            "cancelled",
            None,
            None,
            None,
            Some("Pending action was cancelled by the user."),
            Some(now_ms()),
            Some(now_ms()),
        );
        Ok(true)
    }

    fn queue_action_continuation(
        &self,
        action_id: &str,
        pending_status: PendingActionStatus,
        decision_status: AgentApprovalDecisionStatus,
        message: Option<String>,
        notifications: CoreServerNotificationSender,
    ) -> Result<AgentActionExecutionOutput, String> {
        let record = {
            let mut pending_actions = self
                .pending_actions
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            let Some(record) = pending_actions.get_mut(action_id) else {
                return Err(format!("未找到待审批操作：{action_id}"));
            };
            if record.snapshot.status != PendingActionStatus::Pending {
                return Err(format!("待审批操作已经处理：{action_id}"));
            }
            record.snapshot.status = pending_status;
            record.clone()
        };

        let mut call = tool_call_for_action(&record.snapshot.action);
        call.approval_status = match decision_status {
            AgentApprovalDecisionStatus::Approved => AgentApprovalStatus::Approved,
            AgentApprovalDecisionStatus::Rejected => AgentApprovalStatus::Rejected,
        };
        let decided_at = now_ms();
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
            return self.queue_command_execution(record, call, notifications);
        }

        let execution =
            action_execution_for_decision(&record, &call, decision_status, message.as_deref());
        let final_pending_status = execution.final_pending_status;
        let tool_result = execution.tool_result.clone();
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

        let mut agent_input = record.agent_input.clone();
        agent_input.approval_decision = Some(AgentApprovalDecision {
            action_id: record.snapshot.action_id.clone(),
            status: decision_status,
            message: message.clone(),
        });
        agent_input.tool_continuation = Some(AgentToolContinuation {
            call: call.clone(),
            result: tool_result.clone(),
        });

        let run_id = record.snapshot.run_id.clone();
        let service = self.clone();
        let continuation_record = record.clone();
        tokio::spawn(async move {
            service
                .run_action_continuation(
                    continuation_record,
                    agent_input,
                    notifications,
                    final_pending_status,
                )
                .await;
        });

        Ok(AgentActionExecutionOutput {
            action_id: record.snapshot.action_id,
            action_type: record.snapshot.action_type,
            tool_name: record.snapshot.tool_name,
            status: execution.status,
            patch_result: execution.patch_result,
            command_result: None,
            tool_result: Some(execution.tool_result),
            agent_output: AgentChatOutput {
                content: String::new(),
                status: AgentRunStatus::Running,
                run_id,
                events: Vec::new(),
                tool_definitions: Vec::new(),
                usage: None,
                finish_reason: None,
                proposed_actions: Vec::new(),
            },
        })
    }

    fn queue_command_execution(
        &self,
        record: PendingActionRecord,
        call: AgentToolCall,
        notifications: CoreServerNotificationSender,
    ) -> Result<AgentActionExecutionOutput, String> {
        let run_id = record.snapshot.run_id.clone();
        let service = self.clone();
        let command_record = record.clone();
        tokio::spawn(async move {
            service
                .run_command_execution(command_record, call, notifications)
                .await;
        });

        Ok(AgentActionExecutionOutput {
            action_id: record.snapshot.action_id,
            action_type: record.snapshot.action_type,
            tool_name: record.snapshot.tool_name,
            status: "approved".to_string(),
            patch_result: None,
            command_result: None,
            tool_result: None,
            agent_output: AgentChatOutput {
                content: String::new(),
                status: AgentRunStatus::Running,
                run_id,
                events: Vec::new(),
                tool_definitions: Vec::new(),
                usage: None,
                finish_reason: None,
                proposed_actions: Vec::new(),
            },
        })
    }

    async fn run_command_execution(
        &self,
        record: PendingActionRecord,
        call: AgentToolCall,
        notifications: CoreServerNotificationSender,
    ) {
        let run_id = record.snapshot.run_id.clone();
        let action_id = record.snapshot.action_id.clone();
        let AgentProposedAction::Command { command } = record.snapshot.action.clone() else {
            self.update_pending_status(&action_id, PendingActionStatus::Failed);
            return;
        };

        let cancellation_token = AgentCancellationToken::new();
        self.register_cancellation(&run_id, cancellation_token.clone());

        let guard = self.command_runs.register(&action_id, &run_id);
        let cancel_flag = guard.cancel_flag();
        let workspace_root = workspace_root_optional(&record.agent_input);
        let permissions = permissions_from_input(&record.agent_input);
        let command_for_error = command.clone();
        let command_result = tokio::task::spawn_blocking(move || {
            let _guard = guard;
            run_approved_command(
                workspace_root.as_deref(),
                &command,
                permissions,
                cancellation_token,
                Some(cancel_flag),
            )
        })
        .await
        .map_err(|error| format!("命令执行任务失败：{error}"))
        .and_then(|result| result)
        .unwrap_or_else(|error| failed_command_result(&command_for_error, error));
        self.unregister_cancellation(&run_id);

        let command_succeeded = command_result.error.is_none()
            && !command_result.timed_out
            && !command_result.cancelled
            && command_result.exit_code == Some(0);
        let final_pending_status = if command_result.cancelled {
            PendingActionStatus::Cancelled
        } else if command_succeeded {
            PendingActionStatus::Completed
        } else {
            PendingActionStatus::Failed
        };
        let tool_result = command_tool_result(&action_id, command_succeeded, &command_result);
        self.record_action_audit(
            &record,
            Some("approved"),
            pending_status_label(final_pending_status),
            None,
            Some(&command_result),
            Some(&tool_result),
            tool_result.error.as_deref(),
            None,
            Some(now_ms()),
        );

        let _ = notifications.send(agent_event_notification(AgentEvent::ToolResult {
            run_id: run_id.clone(),
            result: tool_result.clone(),
        }));

        let mut agent_input = record.agent_input.clone();
        agent_input.approval_decision = Some(AgentApprovalDecision {
            action_id: action_id.clone(),
            status: AgentApprovalDecisionStatus::Approved,
            message: None,
        });
        agent_input.tool_continuation = Some(AgentToolContinuation {
            call,
            result: tool_result,
        });

        self.run_action_continuation(record, agent_input, notifications, final_pending_status)
            .await;
    }

    async fn run_action_continuation(
        &self,
        record: PendingActionRecord,
        agent_input: AgentChatInput,
        notifications: CoreServerNotificationSender,
        final_pending_status: PendingActionStatus,
    ) {
        let run_id = record.snapshot.run_id.clone();
        let cancellation_token = AgentCancellationToken::new();
        self.register_cancellation(&run_id, cancellation_token.clone());

        let emitter_notifications = notifications.clone();
        let emitter_service = self.clone();
        let emitter_conversation_id = record.snapshot.conversation_id.clone();
        let emitter_assistant_message_id = record.snapshot.assistant_message_id.clone();
        let emitter_agent_input = record.agent_input.clone();
        let emitter: AgentEventEmitter = Arc::new(move |event| {
            if let AgentEvent::ApprovalRequired { run_id, action } = &event {
                emitter_service.store_pending_action(
                    run_id,
                    emitter_conversation_id.as_deref().unwrap_or_default(),
                    emitter_assistant_message_id.as_deref().unwrap_or_default(),
                    action.clone(),
                    emitter_agent_input.clone(),
                );
            }
            let _ = emitter_notifications.send(agent_event_notification(event));
        });

        let result = send_chat_with_events_and_cancellation(
            agent_input,
            run_id.clone(),
            emitter,
            cancellation_token,
        )
        .await;

        match result {
            Ok(agent_output) => {
                if let (Some(conversation_id), Some(assistant_message_id)) = (
                    record.snapshot.conversation_id.as_deref(),
                    record.snapshot.assistant_message_id.as_deref(),
                ) {
                    let _ = self.persist_final_assistant_output(
                        conversation_id,
                        assistant_message_id,
                        &agent_output,
                    );
                }
                self.update_pending_status(&record.snapshot.action_id, final_pending_status);
            }
            Err(error) => {
                let message = error.to_string();
                if let (Some(conversation_id), Some(assistant_message_id)) = (
                    record.snapshot.conversation_id.as_deref(),
                    record.snapshot.assistant_message_id.as_deref(),
                ) {
                    let _ = self.persist_assistant_error(
                        conversation_id,
                        assistant_message_id,
                        &message,
                    );
                }
                let _ = notifications.send(agent_event_notification(AgentEvent::Error {
                    run_id: Some(run_id.clone()),
                    message: message.clone(),
                    recoverable: false,
                }));
                let _ = notifications.send(agent_event_notification(AgentEvent::Done {
                    run_id: run_id.clone(),
                    success: false,
                    status: Some(AgentRunStatus::Failed),
                    content: Some(message),
                    usage: None,
                    finish_reason: None,
                    proposed_actions: Vec::new(),
                }));
                self.update_pending_status(&record.snapshot.action_id, PendingActionStatus::Failed);
            }
        }

        self.unregister_cancellation(&run_id);
    }

    fn store_pending_action(
        &self,
        run_id: &str,
        conversation_id: &str,
        assistant_message_id: &str,
        action: AgentProposedAction,
        agent_input: AgentChatInput,
    ) {
        let action_id = action_id_for_action(&action);
        let pending_record = PendingActionRecord {
            snapshot: PendingAgentActionSnapshot {
                action_id: action_id.clone(),
                run_id: run_id.to_string(),
                conversation_id: normalized_optional(Some(conversation_id)),
                assistant_message_id: normalized_optional(Some(assistant_message_id)),
                action_type: action_type_for_action(&action).to_string(),
                tool_name: tool_name_for_action(&action),
                tool_call_id: Some(action_id_for_action(&action)),
                action,
                created_at: now_ms(),
                status: PendingActionStatus::Pending,
            },
            agent_input,
        };

        {
            let mut pending_actions = self
                .pending_actions
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            pending_actions.insert(action_id, pending_record.clone());
        }
        self.record_action_audit(
            &pending_record,
            None,
            "pending",
            None,
            None,
            None,
            None,
            None,
            None,
        );
    }

    fn update_pending_status(&self, action_id: &str, status: PendingActionStatus) {
        let mut pending_actions = self
            .pending_actions
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if let Some(record) = pending_actions.get_mut(action_id) {
            record.snapshot.status = status;
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn record_action_audit(
        &self,
        record: &PendingActionRecord,
        decision: Option<&str>,
        status: &str,
        patch_result: Option<&AgentPatchResult>,
        command_result: Option<&AgentCommandExecutionResult>,
        tool_result: Option<&AgentToolResult>,
        error: Option<&str>,
        decided_at: Option<i64>,
        completed_at: Option<i64>,
    ) {
        let audit = AgentActionAuditRecord {
            action_id: record.snapshot.action_id.clone(),
            run_id: record.snapshot.run_id.clone(),
            conversation_id: record.snapshot.conversation_id.clone(),
            assistant_message_id: record.snapshot.assistant_message_id.clone(),
            action_type: record.snapshot.action_type.clone(),
            tool_name: record.snapshot.tool_name.clone(),
            decision: decision.map(ToString::to_string),
            status: status.to_string(),
            action_json: serialize_json(&record.snapshot.action),
            patch_result_json: patch_result.map(serialize_json),
            command_result_json: command_result.map(serialize_json),
            tool_result_json: tool_result.map(serialize_json),
            error: error.map(ToString::to_string),
            created_at: record.snapshot.created_at,
            decided_at,
            completed_at,
        };
        if let Err(error) = self.storage.upsert_agent_action_audit(audit) {
            eprintln!("failed to write agent action audit log: {error}");
        }
    }

    fn register_cancellation(&self, run_id: &str, token: AgentCancellationToken) {
        let mut cancellations = self
            .cancellations
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        cancellations.insert(run_id.to_string(), token);
    }

    fn unregister_cancellation(&self, run_id: &str) {
        let mut cancellations = self
            .cancellations
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        cancellations.remove(run_id);
    }

    fn register_usage_context(&self, run_id: &str, context: AgentRunUsageContext) {
        let mut contexts = self
            .usage_contexts
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        contexts.insert(
            run_id.to_string(),
            AgentRunUsageState {
                context,
                usage: None,
                status: AgentRunStatus::Running,
                error: None,
            },
        );
    }

    fn find_usage_run_id(
        &self,
        conversation_id: &str,
        assistant_message_id: &str,
    ) -> Option<String> {
        let contexts = self
            .usage_contexts
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        contexts
            .iter()
            .find(|(_, state)| {
                state.context.conversation_id == conversation_id
                    && state.context.assistant_message_id == assistant_message_id
            })
            .map(|(run_id, _)| run_id.clone())
    }

    fn persist_run_usage(
        &self,
        run_id: &str,
        status: AgentRunStatus,
        usage: Option<AgentUsage>,
        error: Option<String>,
    ) -> Result<(), String> {
        let now = now_ms();
        let (record, should_remove) = {
            let mut contexts = self
                .usage_contexts
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            let Some(state) = contexts.get_mut(run_id) else {
                return Ok(());
            };
            merge_usage(&mut state.usage, usage);
            state.status = status;
            if let Some(error) = error {
                state.error = Some(error);
            }

            let usage = state.usage.clone();
            let billable_request_count = usage
                .as_ref()
                .and_then(|usage| usage.billable_request_count)
                .unwrap_or(0);
            let input_tokens = usage.as_ref().and_then(|usage| usage.input_tokens);
            let output_tokens = usage.as_ref().and_then(|usage| usage.output_tokens);
            let estimated_cost = self.storage.estimate_usage_cost(
                input_tokens,
                output_tokens,
                state.context.input_price.as_deref().unwrap_or(""),
                state.context.output_price.as_deref().unwrap_or(""),
            );

            let record = AgentUsageRecordInsert {
                id: format!("usage-{}", state.context.run_id),
                conversation_id: state.context.conversation_id.clone(),
                message_id: state.context.assistant_message_id.clone(),
                run_id: state.context.run_id.clone(),
                project_id: state.context.project_id.clone(),
                model_id: state.context.model_id.clone(),
                model_name: state.context.model_name.clone(),
                provider_path: state.context.provider_path.clone(),
                started_at: Some(state.context.started_at),
                completed_at: is_terminal_run_status(status).then_some(now),
                status: Some(run_status_label(status).to_string()),
                error: state.error.clone(),
                created_at: now,
                input_tokens,
                output_tokens,
                total_tokens: usage.as_ref().and_then(|usage| usage.total_tokens),
                cached_input_tokens: usage.as_ref().and_then(|usage| usage.cached_input_tokens),
                cache_creation_input_tokens: usage
                    .as_ref()
                    .and_then(|usage| usage.cache_creation_input_tokens),
                billable_request_count,
                input_price: state.context.input_price.clone(),
                output_price: state.context.output_price.clone(),
                estimated_cost,
            };
            (record, is_terminal_run_status(status))
        };

        self.storage.upsert_agent_usage(record)?;
        if should_remove {
            let mut contexts = self
                .usage_contexts
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            contexts.remove(run_id);
        }
        Ok(())
    }

    fn persist_final_assistant_output(
        &self,
        conversation_id: &str,
        assistant_message_id: &str,
        output: &AgentChatOutput,
    ) -> Result<(), String> {
        self.persist_run_usage(
            &output.run_id,
            output.status,
            output.usage.clone(),
            output.finish_reason.clone(),
        )?;
        self.storage.update_chat_message_status_and_content(
            conversation_id,
            assistant_message_id,
            &output.content,
            status_for_run(output.status),
            now_ms(),
        )
    }

    fn persist_assistant_error(
        &self,
        conversation_id: &str,
        assistant_message_id: &str,
        message: &str,
    ) -> Result<(), String> {
        if let Some(run_id) = self.find_usage_run_id(conversation_id, assistant_message_id) {
            self.persist_run_usage(
                &run_id,
                AgentRunStatus::Failed,
                None,
                Some(message.to_string()),
            )?;
        }
        self.storage.update_chat_message_status_and_content(
            conversation_id,
            assistant_message_id,
            message,
            Some("error"),
            now_ms(),
        )
    }

    pub fn get_usage_summary(
        &self,
        input: &AgentUsageSummaryInput,
    ) -> Result<AgentUsageSummaryOutput, String> {
        self.storage.get_usage_summary(input, now_ms())
    }

    pub fn clear_usage_records(
        &self,
        input: &AgentUsageClearInput,
    ) -> Result<AgentUsageClearOutput, String> {
        self.storage.clear_usage_records(input)
    }
}

#[derive(Debug, Clone)]
struct PendingActionRecord {
    snapshot: PendingAgentActionSnapshot,
    agent_input: AgentChatInput,
}

#[derive(Debug, Clone)]
struct AgentRunUsageContext {
    conversation_id: String,
    assistant_message_id: String,
    run_id: String,
    project_id: Option<String>,
    model_id: String,
    model_name: String,
    provider_path: Option<String>,
    input_price: Option<String>,
    output_price: Option<String>,
    started_at: i64,
}

#[derive(Debug, Clone)]
struct AgentRunUsageState {
    context: AgentRunUsageContext,
    usage: Option<AgentUsage>,
    status: AgentRunStatus,
    error: Option<String>,
}

struct ActionExecutionDecision {
    status: String,
    final_pending_status: PendingActionStatus,
    patch_result: Option<AgentPatchResult>,
    tool_result: AgentToolResult,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PendingActionStatus {
    Pending,
    Approved,
    Rejected,
    Cancelled,
    Completed,
    Failed,
}

fn pending_status_label(status: PendingActionStatus) -> &'static str {
    match status {
        PendingActionStatus::Pending => "pending",
        PendingActionStatus::Approved => "approved",
        PendingActionStatus::Rejected => "rejected",
        PendingActionStatus::Cancelled => "cancelled",
        PendingActionStatus::Completed => "completed",
        PendingActionStatus::Failed => "failed",
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PendingAgentActionSnapshot {
    pub action_id: String,
    pub action_type: String,
    pub tool_name: String,
    pub tool_call_id: Option<String>,
    pub run_id: String,
    pub conversation_id: Option<String>,
    pub assistant_message_id: Option<String>,
    pub action: AgentProposedAction,
    pub created_at: i64,
    pub status: PendingActionStatus,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentActionExecutionOutput {
    pub action_id: String,
    pub action_type: String,
    pub tool_name: String,
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub patch_result: Option<AgentPatchResult>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub command_result: Option<AgentCommandExecutionResult>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_result: Option<AgentToolResult>,
    pub agent_output: AgentChatOutput,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentConversationTurnInput {
    pub conversation_id: Option<String>,
    pub project_id: Option<String>,
    pub model_id: String,
    pub content: String,
    #[serde(default)]
    pub attachments: Vec<AgentInputAttachment>,
    pub title: Option<String>,
    pub user_message_id: Option<String>,
    pub assistant_message_id: Option<String>,
    pub max_tokens: Option<u32>,
    pub temperature: Option<f32>,
    pub mode: Option<AgentRunMode>,
    pub prompt_preferences: Option<AgentPromptPreferences>,
    #[serde(default)]
    pub permissions: AgentPermissions,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentConversationTurnOutput {
    pub run_id: String,
    pub event_name: String,
    pub conversation_id: String,
    pub user_message_id: String,
    pub assistant_message_id: String,
    pub user_message: ChatMessageRecord,
    pub assistant_message: ChatMessageRecord,
}

struct PreparedConversationTurn {
    output: AgentConversationTurnOutput,
    agent_input: AgentChatInput,
    usage_context: AgentRunUsageContext,
}

fn prepare_conversation_turn(
    storage: &StorageService,
    input: AgentConversationTurnInput,
    run_id: &str,
) -> Result<PreparedConversationTurn, String> {
    let content = input.content.trim().to_string();
    if content.is_empty() {
        return Err("消息内容不能为空。".to_string());
    }

    let model_id = input.model_id.trim().to_string();
    if model_id.is_empty() {
        return Err("modelId 不能为空。".to_string());
    }

    let settings = storage
        .load_model_settings()?
        .ok_or_else(|| "请先配置模型 API。".to_string())?;
    if settings.api_url.trim().is_empty() || settings.api_token.trim().is_empty() {
        return Err("请先配置模型 API。".to_string());
    }

    let model = settings
        .models
        .iter()
        .find(|model| model.id == model_id)
        .cloned()
        .ok_or_else(|| format!("未找到模型配置：{model_id}"))?;
    if !model.enabled {
        return Err(format!("模型未启用：{model_id}"));
    }

    let prompt_preferences = match input.prompt_preferences.clone() {
        Some(preferences) => preferences,
        None => agent_prompt_preferences_from_record(storage.load_agent_prompt_preferences()?),
    };

    let timestamp = now_ms();
    let conversation_id = normalized_optional(input.conversation_id.as_deref())
        .unwrap_or_else(|| create_id("conversation"));
    let user_message_id = normalized_optional(input.user_message_id.as_deref())
        .unwrap_or_else(|| create_id("message"));
    let assistant_message_id = normalized_optional(input.assistant_message_id.as_deref())
        .unwrap_or_else(|| create_id("message"));

    let existing = storage
        .load_conversations()?
        .into_iter()
        .find(|conversation| conversation.id == conversation_id);
    let input_project_id = normalized_optional(input.project_id.as_deref());
    let resolved_project_id = input_project_id.or_else(|| {
        existing
            .as_ref()
            .and_then(|conversation| normalized_optional(conversation.project_id.as_deref()))
    });
    let project = resolve_project(storage, resolved_project_id.as_deref())?;

    let mut conversation = existing.unwrap_or_else(|| ChatConversationRecord {
        id: conversation_id.clone(),
        project_id: resolved_project_id.clone(),
        model_id: Some(model_id.clone()),
        title: input
            .title
            .as_deref()
            .map(str::trim)
            .filter(|title| !title.is_empty())
            .map(ToString::to_string)
            .unwrap_or_else(|| create_conversation_title(&content)),
        messages: Vec::new(),
        created_at: timestamp,
        updated_at: timestamp,
        pinned_at: None,
        archived_at: None,
        unread_at: None,
    });

    conversation.project_id = resolved_project_id.clone();
    conversation.model_id = Some(model_id.clone());
    conversation.updated_at = timestamp;

    let history_messages = conversation
        .messages
        .iter()
        .filter(|message| message.id != user_message_id && message.id != assistant_message_id)
        .filter(|message| message.status.as_deref() != Some("pending"))
        .filter(|message| message.status.as_deref() != Some("error"))
        .filter(|message| matches!(message.role.as_str(), "user" | "assistant"))
        .filter(|message| !message.content.trim().is_empty())
        .map(|message| AgentChatMessage {
            role: message.role.clone(),
            content: message.content.clone(),
        })
        .collect::<Vec<_>>();

    let user_message = ChatMessageRecord {
        id: user_message_id.clone(),
        role: "user".to_string(),
        content: content.clone(),
        created_at: timestamp,
        status: Some("sent".to_string()),
        attachments: message_attachments_from_input(&input.attachments, timestamp),
        agent_run_json: None,
        ui_state_json: None,
    };
    let assistant_message = ChatMessageRecord {
        id: assistant_message_id.clone(),
        role: "assistant".to_string(),
        content: THINKING_PLACEHOLDER.to_string(),
        created_at: timestamp + 1,
        status: Some("pending".to_string()),
        attachments: Vec::new(),
        agent_run_json: None,
        ui_state_json: None,
    };

    upsert_message(&mut conversation.messages, user_message.clone());
    upsert_message(&mut conversation.messages, assistant_message.clone());
    storage.save_conversation(conversation)?;

    let mut agent_messages = history_messages;
    agent_messages.push(AgentChatMessage {
        role: "user".to_string(),
        content,
    });

    let agent_input = AgentChatInput {
        api_url: settings.api_url.trim().to_string(),
        api_token: settings.api_token.trim().to_string(),
        model: model_provider_path(&model),
        api_style: None,
        max_tokens: input.max_tokens,
        temperature: input.temperature,
        mode: input.mode,
        stream: Some(true),
        context: Some(AgentRunContext {
            conversation_id: Some(conversation_id.clone()),
            project_id: resolved_project_id.clone(),
            workspace: project.as_ref().map(|project| AgentWorkspaceContext {
                project_id: Some(project.id.clone()),
                display_name: Some(project.name.clone()),
                root_path: project.path.clone(),
            }),
            attachment_library: None,
            permissions: input.permissions,
        }),
        search_config: Some(AgentSearchConfig {
            mode: search_mode_from_storage(&settings.search_mode),
            tavily_api_key: non_empty(settings.tavily_api_key),
        }),
        prompt_preferences: Some(prompt_preferences),
        approval_decision: None,
        tool_continuation: None,
        attachments: input.attachments,
        messages: agent_messages,
    };

    Ok(PreparedConversationTurn {
        usage_context: AgentRunUsageContext {
            conversation_id: conversation_id.clone(),
            assistant_message_id: assistant_message_id.clone(),
            run_id: run_id.to_string(),
            project_id: resolved_project_id.clone(),
            model_id: model.id.clone(),
            model_name: model.display_name.clone(),
            provider_path: model.provider_path.clone(),
            input_price: Some(model.input_price.clone()),
            output_price: Some(model.output_price.clone()),
            started_at: timestamp,
        },
        output: AgentConversationTurnOutput {
            run_id: run_id.to_string(),
            event_name: AGENT_EVENT_NAME.to_string(),
            conversation_id,
            user_message_id,
            assistant_message_id,
            user_message,
            assistant_message,
        },
        agent_input,
    })
}

pub fn agent_event_notification(event: AgentEvent) -> Value {
    json!({
        "jsonrpc": "2.0",
        "method": AGENT_EVENT_NOTIFICATION_METHOD,
        "params": event
    })
}

fn resolve_project(
    storage: &StorageService,
    project_id: Option<&str>,
) -> Result<Option<ProjectRecord>, String> {
    let Some(project_id) = project_id else {
        return Ok(None);
    };

    let project = storage
        .load_projects()?
        .into_iter()
        .find(|project| project.id == project_id)
        .ok_or_else(|| format!("未找到项目：{project_id}"))?;
    if project
        .path
        .as_deref()
        .map(str::trim)
        .filter(|path| !path.is_empty())
        .is_none()
    {
        return Err(format!(
            "项目「{}」没有绑定本地 workspace 路径，请重新选择项目目录。",
            project.name
        ));
    }
    Ok(Some(project))
}

fn message_attachments_from_input(
    attachments: &[AgentInputAttachment],
    created_at: i64,
) -> Vec<ChatMessageAttachmentRecord> {
    attachments
        .iter()
        .map(|attachment| {
            let preview_data = if attachment.kind == AgentInputAttachmentKind::Image
                && attachment.encoding == AgentInputAttachmentEncoding::Base64
                && attachment
                    .mime_type
                    .as_deref()
                    .is_some_and(|mime_type| mime_type.starts_with("image/"))
            {
                Some(attachment.data.clone())
            } else {
                None
            };

            ChatMessageAttachmentRecord {
                id: safe_path_component(&attachment.id, "attachment"),
                kind: input_attachment_kind_label(attachment.kind).to_string(),
                name: attachment.name.clone(),
                mime_type: attachment.mime_type.clone(),
                size_bytes: attachment.size_bytes,
                preview_mime_type: preview_data.as_ref().and(attachment.mime_type.clone()),
                preview_data,
                created_at,
            }
        })
        .collect()
}

fn agent_prompt_preferences_from_record(
    record: AgentPromptPreferencesRecord,
) -> AgentPromptPreferences {
    AgentPromptPreferences {
        work_mode: Some(match record.work_mode.as_str() {
            "general" => AgentPromptWorkMode::General,
            _ => AgentPromptWorkMode::Coding,
        }),
        tone: Some(match record.tone.as_str() {
            "friendly" => AgentPromptTone::Friendly,
            _ => AgentPromptTone::Pragmatic,
        }),
        detail_level: Some(match record.detail_level.as_str() {
            "low" => AgentPromptDetailLevel::Low,
            "high" => AgentPromptDetailLevel::High,
            _ => AgentPromptDetailLevel::Medium,
        }),
        custom_instructions: normalized_optional(Some(&record.custom_instructions)),
        updated_at: Some(record.updated_at),
    }
}

fn input_attachment_kind_label(kind: AgentInputAttachmentKind) -> &'static str {
    match kind {
        AgentInputAttachmentKind::File => "file",
        AgentInputAttachmentKind::Image => "image",
    }
}

fn search_mode_from_storage(value: &str) -> AgentSearchMode {
    match value {
        "disabled" => AgentSearchMode::Disabled,
        "tavily" => AgentSearchMode::Tavily,
        _ => AgentSearchMode::Auto,
    }
}

fn model_provider_path(model: &ModelConfigRecord) -> String {
    model
        .provider_path
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(model.id.as_str())
        .to_string()
}

fn normalized_optional(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
}

fn non_empty(value: String) -> Option<String> {
    let value = value.trim().to_string();
    if value.is_empty() || value == "tvly-my-copilot-search-key" {
        None
    } else {
        Some(value)
    }
}

fn upsert_message(messages: &mut Vec<ChatMessageRecord>, next: ChatMessageRecord) {
    if let Some(existing) = messages.iter_mut().find(|message| message.id == next.id) {
        *existing = next;
    } else {
        messages.push(next);
    }
}

fn status_for_run(status: AgentRunStatus) -> Option<&'static str> {
    match status {
        AgentRunStatus::Completed | AgentRunStatus::Cancelled => Some("sent"),
        AgentRunStatus::WaitingForApproval | AgentRunStatus::Running | AgentRunStatus::Idle => {
            Some("pending")
        }
        AgentRunStatus::Failed => Some("error"),
    }
}

fn run_status_label(status: AgentRunStatus) -> &'static str {
    match status {
        AgentRunStatus::Idle => "idle",
        AgentRunStatus::Running => "running",
        AgentRunStatus::WaitingForApproval => "waiting_for_approval",
        AgentRunStatus::Completed => "completed",
        AgentRunStatus::Failed => "failed",
        AgentRunStatus::Cancelled => "cancelled",
    }
}

fn is_terminal_run_status(status: AgentRunStatus) -> bool {
    matches!(
        status,
        AgentRunStatus::Completed | AgentRunStatus::Failed | AgentRunStatus::Cancelled
    )
}

fn merge_usage(total: &mut Option<AgentUsage>, next: Option<AgentUsage>) {
    let Some(next) = next else {
        return;
    };
    let total_usage = total.get_or_insert(AgentUsage {
        input_tokens: None,
        output_tokens: None,
        total_tokens: None,
        cached_input_tokens: None,
        cache_creation_input_tokens: None,
        billable_request_count: None,
    });

    total_usage.input_tokens = add_optional(total_usage.input_tokens, next.input_tokens);
    total_usage.output_tokens = add_optional(total_usage.output_tokens, next.output_tokens);
    total_usage.total_tokens = add_optional(total_usage.total_tokens, next.total_tokens);
    total_usage.cached_input_tokens =
        add_optional(total_usage.cached_input_tokens, next.cached_input_tokens);
    total_usage.cache_creation_input_tokens = add_optional(
        total_usage.cache_creation_input_tokens,
        next.cache_creation_input_tokens,
    );
    total_usage.billable_request_count = add_optional(
        total_usage.billable_request_count,
        next.billable_request_count.or_else(|| {
            (next.input_tokens.is_some()
                || next.output_tokens.is_some()
                || next.total_tokens.is_some())
            .then_some(1)
        }),
    );
}

fn add_optional(left: Option<u64>, right: Option<u64>) -> Option<u64> {
    match (left, right) {
        (Some(left), Some(right)) => Some(left.saturating_add(right)),
        (Some(left), None) => Some(left),
        (None, Some(right)) => Some(right),
        (None, None) => None,
    }
}

fn action_id_for_action(action: &AgentProposedAction) -> String {
    match action {
        AgentProposedAction::ToolCall { call } => call.id.clone(),
        AgentProposedAction::Diff { diff } => diff.id.clone(),
        AgentProposedAction::Command { command } => command.id.clone(),
    }
}

fn action_type_for_action(action: &AgentProposedAction) -> &'static str {
    match action {
        AgentProposedAction::ToolCall { .. } => "tool_call",
        AgentProposedAction::Diff { .. } => "diff",
        AgentProposedAction::Command { .. } => "command",
    }
}

fn tool_name_for_action(action: &AgentProposedAction) -> String {
    match action {
        AgentProposedAction::ToolCall { call } => call.tool.clone(),
        AgentProposedAction::Diff { .. } => "apply_patch".to_string(),
        AgentProposedAction::Command { .. } => "run_command".to_string(),
    }
}

fn tool_call_for_action(action: &AgentProposedAction) -> AgentToolCall {
    match action {
        AgentProposedAction::ToolCall { call } => call.clone(),
        AgentProposedAction::Diff { diff } => diff_tool_call(diff),
        AgentProposedAction::Command { command } => command_tool_call(command),
    }
}

fn diff_tool_call(diff: &AgentDiffProposal) -> AgentToolCall {
    AgentToolCall {
        id: diff.id.clone(),
        tool: "apply_patch".to_string(),
        args: json!({
            "operation": diff.operation,
            "filePath": diff.file_path.clone(),
            "patch": diff.patch.clone(),
            "baseRevision": diff.base_revision.clone(),
            "summary": diff.summary.clone()
        }),
        approval_status: diff.approval_status,
        reason: diff.summary.clone(),
    }
}

fn command_tool_call(command: &AgentCommandRequest) -> AgentToolCall {
    AgentToolCall {
        id: command.id.clone(),
        tool: "run_command".to_string(),
        args: json!({
            "command": command.command.clone(),
            "cwd": command.cwd.clone(),
            "timeoutMs": command.timeout_ms,
            "riskLevel": command.risk_level,
            "reason": command.reason.clone()
        }),
        approval_status: command.approval_status,
        reason: command.reason.clone(),
    }
}

fn tool_result_for_decision(
    call: &AgentToolCall,
    decision_status: AgentApprovalDecisionStatus,
    message: Option<&str>,
) -> AgentToolResult {
    match decision_status {
        AgentApprovalDecisionStatus::Approved => AgentToolResult {
            call_id: call.id.clone(),
            tool: call.tool.clone(),
            ok: false,
            result: Some(json!({
                "status": "not_implemented",
                "phase": "pending_actions_continuation",
                "message": "Host approval path is wired, but execution is not implemented in this phase. No file, command, git, or shell changes were made."
            })),
            error: Some("已批准，但本阶段尚未实现真实执行；没有执行文件修改或命令。".to_string()),
        },
        AgentApprovalDecisionStatus::Rejected => AgentToolResult {
            call_id: call.id.clone(),
            tool: call.tool.clone(),
            ok: true,
            result: Some(json!({
                "status": "rejected",
                "message": message
            })),
            error: None,
        },
    }
}

fn action_execution_for_decision(
    record: &PendingActionRecord,
    call: &AgentToolCall,
    decision_status: AgentApprovalDecisionStatus,
    message: Option<&str>,
) -> ActionExecutionDecision {
    if decision_status == AgentApprovalDecisionStatus::Rejected {
        return rejected_action_execution(record, call, message);
    }

    match &record.snapshot.action {
        AgentProposedAction::Diff { diff } => approved_patch_execution(record, diff),
        AgentProposedAction::ToolCall { call } if call.tool == "apply_patch" => {
            ActionExecutionDecision {
                status: "failed".to_string(),
                final_pending_status: PendingActionStatus::Failed,
                patch_result: None,
                tool_result: AgentToolResult {
                    call_id: call.id.clone(),
                    tool: call.tool.clone(),
                    ok: false,
                    result: Some(json!({
                        "status": "failed",
                        "message": "apply_patch approval action must be represented as a diff proposal before execution."
                    })),
                    error: Some("apply_patch 审批缺少 diff proposal，不能执行。".to_string()),
                },
            }
        }
        _ => ActionExecutionDecision {
            status: "failed".to_string(),
            final_pending_status: PendingActionStatus::Failed,
            patch_result: None,
            tool_result: tool_result_for_decision(call, decision_status, message),
        },
    }
}

fn rejected_action_execution(
    record: &PendingActionRecord,
    call: &AgentToolCall,
    message: Option<&str>,
) -> ActionExecutionDecision {
    if let AgentProposedAction::Diff { diff } = &record.snapshot.action {
        let patch_result = AgentPatchResult {
            status: AgentPatchResultStatus::Rejected,
            operation: diff.operation,
            file_path: diff.file_path.clone(),
            applied_file_paths: Vec::new(),
            git_diff: None,
            git_diff_error: None,
            error: None,
            message: message.map(ToString::to_string),
        };
        let tool_result = patch_tool_result(&record.snapshot.action_id, true, &patch_result);
        return ActionExecutionDecision {
            status: "rejected".to_string(),
            final_pending_status: PendingActionStatus::Rejected,
            patch_result: Some(patch_result),
            tool_result,
        };
    }

    ActionExecutionDecision {
        status: "rejected".to_string(),
        final_pending_status: PendingActionStatus::Rejected,
        patch_result: None,
        tool_result: tool_result_for_decision(call, AgentApprovalDecisionStatus::Rejected, message),
    }
}

fn approved_patch_execution(
    record: &PendingActionRecord,
    diff: &mycopilot_core::AgentDiffProposal,
) -> ActionExecutionDecision {
    let workspace_root = workspace_root_optional(&record.agent_input);
    let permissions = permissions_from_input(&record.agent_input);
    let patch_result = match apply_unified_diff_in_workspace(
        workspace_root.as_deref(),
        diff.operation,
        &diff.file_path,
        &diff.patch,
        diff.base_revision.as_deref(),
        permissions,
    ) {
        Ok(apply_result) => AgentPatchResult {
            status: AgentPatchResultStatus::Applied,
            operation: diff.operation,
            file_path: diff.file_path.clone(),
            applied_file_paths: apply_result.file_paths,
            git_diff: None,
            git_diff_error: None,
            error: None,
            message: diff.summary.clone(),
        },
        Err(error) => AgentPatchResult {
            status: patch_status_for_error(&error),
            operation: diff.operation,
            file_path: diff.file_path.clone(),
            applied_file_paths: Vec::new(),
            git_diff: None,
            git_diff_error: None,
            error: Some(error),
            message: None,
        },
    };

    let applied = patch_result.status == AgentPatchResultStatus::Applied;
    let conflict = patch_result.status == AgentPatchResultStatus::Conflict;
    let tool_result = patch_tool_result(&record.snapshot.action_id, applied, &patch_result);
    ActionExecutionDecision {
        status: if applied {
            "applied".to_string()
        } else if conflict {
            "conflict".to_string()
        } else {
            "failed".to_string()
        },
        final_pending_status: if applied {
            PendingActionStatus::Completed
        } else {
            PendingActionStatus::Failed
        },
        patch_result: Some(patch_result),
        tool_result,
    }
}

fn patch_status_for_error(error: &str) -> AgentPatchResultStatus {
    if error.contains("审批前已发生变化")
        || error.contains("baseRevision")
        || error.contains("git apply --check")
        || error.contains("patch does not apply")
        || error.contains("patch failed")
    {
        AgentPatchResultStatus::Conflict
    } else {
        AgentPatchResultStatus::Failed
    }
}

fn patch_tool_result(
    action_id: &str,
    observation_ok: bool,
    patch_result: &AgentPatchResult,
) -> AgentToolResult {
    AgentToolResult {
        call_id: action_id.to_string(),
        tool: "apply_patch".to_string(),
        ok: observation_ok,
        result: Some(json!(patch_result)),
        error: if observation_ok {
            None
        } else {
            patch_result
                .error
                .clone()
                .or_else(|| Some("应用 patch 失败。".to_string()))
        },
    }
}

fn command_tool_result(
    action_id: &str,
    observation_ok: bool,
    command_result: &AgentCommandExecutionResult,
) -> AgentToolResult {
    AgentToolResult {
        call_id: action_id.to_string(),
        tool: "run_command".to_string(),
        ok: observation_ok,
        result: Some(json!(command_result)),
        error: if observation_ok {
            None
        } else {
            command_result.error.clone().or_else(|| {
                Some(if command_result.cancelled {
                    "命令已取消。".to_string()
                } else if command_result.timed_out {
                    "命令执行超时。".to_string()
                } else {
                    "命令执行失败。".to_string()
                })
            })
        },
    }
}

fn failed_command_result(
    request: &AgentCommandRequest,
    error: String,
) -> AgentCommandExecutionResult {
    AgentCommandExecutionResult {
        command: request.command.clone(),
        cwd: request.cwd.clone().unwrap_or_else(|| ".".to_string()),
        exit_code: None,
        stdout: String::new(),
        stderr: String::new(),
        timed_out: false,
        cancelled: false,
        duration_ms: 0,
        stdout_truncated: false,
        stderr_truncated: false,
        error: Some(error),
    }
}

fn workspace_root_optional(input: &AgentChatInput) -> Option<PathBuf> {
    input
        .context
        .as_ref()
        .and_then(|context| context.workspace.as_ref())
        .and_then(|workspace| workspace.root_path.as_ref())
        .map(PathBuf::from)
}

fn permissions_from_input(input: &AgentChatInput) -> AgentPermissions {
    input
        .context
        .as_ref()
        .map(|context| context.permissions)
        .unwrap_or_default()
}

fn create_conversation_title(message: &str) -> String {
    let first_line = message
        .lines()
        .next()
        .unwrap_or("新对话")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    if first_line.chars().count() > 24 {
        format!("{}...", first_line.chars().take(24).collect::<String>())
    } else if first_line.is_empty() {
        "新对话".to_string()
    } else {
        first_line
    }
}

fn create_id(prefix: &str) -> String {
    let counter = ID_COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("{prefix}-{}-{counter}", now_ms())
}

fn safe_path_component(value: &str, fallback: &str) -> String {
    let sanitized = value
        .trim()
        .chars()
        .map(|character| match character {
            '/' | '\\' | ':' | '\0' => '_',
            character if character.is_control() => '_',
            character => character,
        })
        .collect::<String>();

    let sanitized = sanitized.trim();
    if sanitized.is_empty() || sanitized == "." || sanitized == ".." {
        fallback.to_string()
    } else {
        sanitized.to_string()
    }
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or(0)
}

fn serialize_json<T: Serialize>(value: &T) -> String {
    serde_json::to_string(value).unwrap_or_else(|error| {
        json!({
            "serializationError": error.to_string()
        })
        .to_string()
    })
}
