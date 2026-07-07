// Rust core-server agent actions and conversation bridge.
use crate::agent_support::*;
pub use crate::agent_support::{
    AgentActionExecutionOutput, AgentConversationTurnInput, AgentConversationTurnOutput,
    PendingActionStatus, PendingAgentActionSnapshot,
};
use std::collections::HashMap;
use std::sync::atomic::AtomicU64;
use std::sync::{Arc, Mutex};

use mycopilot_core::command::{run_approved_command, AgentCommandExecutionResult, CommandRunState};
use mycopilot_core::storage::models::{AgentActionAuditRecord, AgentUsageRecordInsert};
use mycopilot_core::storage::service::StorageService;
use mycopilot_core::{
    next_run_id, send_chat_with_events_and_cancellation, AgentApprovalDecision,
    AgentApprovalDecisionStatus, AgentApprovalStatus, AgentCancellationToken, AgentChatInput,
    AgentChatOutput, AgentEvent, AgentEventEmitter, AgentPatchResult, AgentProposedAction,
    AgentRunStatus, AgentToolCall, AgentToolContinuation, AgentToolResult, AgentUsage,
    AgentUsageClearInput, AgentUsageClearOutput, AgentUsageSummaryInput, AgentUsageSummaryOutput,
};
use serde_json::Value;
use tokio::sync::mpsc::UnboundedSender;

pub(crate) const AGENT_EVENT_NAME: &str = "agent.event";
pub(crate) const THINKING_PLACEHOLDER: &str = "正在思考...";
pub(crate) static ID_COUNTER: AtomicU64 = AtomicU64::new(1);

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
