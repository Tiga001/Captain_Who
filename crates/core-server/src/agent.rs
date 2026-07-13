use crate::agent_support::*;
pub use crate::agent_support::{
    AgentActionExecutionOutput, AgentContextWindowSnapshotInput, AgentContextWindowSnapshotOutput,
    AgentConversationTurnInput, AgentConversationTurnOutput, AgentFileDraftContentPage,
    AgentFileWriteDiffPage, PendingActionStatus, PendingAgentActionSnapshot,
};
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use mycopilot_core::command::{run_approved_command, AgentCommandExecutionResult, CommandRunState};
use mycopilot_core::file_write::{file_draft_snapshot, file_write_diff};
use mycopilot_core::storage::models::{
    AgentActionAuditRecord, AgentPendingActionRecord, AgentUsageRecordInsert,
};
use mycopilot_core::storage::service::StorageService;
use mycopilot_core::{
    cancelled_conversation_trace_from_checkpoint, cancelled_conversation_trace_from_snapshot,
    cancelled_conversation_trace_without_items, completed_conversation_trace_without_items,
    conversation_context_configuration_revision,
    conversation_trace_snapshot_from_checkpoint_and_continuation,
    create_conversation_context_state, failed_conversation_trace_without_items,
    inspect_context_window, next_run_id, send_chat_with_host_executor_and_trace_observer,
    AgentApprovalDecision, AgentApprovalDecisionStatus, AgentApprovalStatus,
    AgentCancellationToken, AgentChatInput, AgentChatOutput, AgentContextBaseline,
    AgentContextWindowPhase, AgentContextWindowSnapshot, AgentConversationContextState,
    AgentConversationTraceObserver, AgentError, AgentEvent, AgentEventEmitter,
    AgentHostActionExecutor, AgentPatchResult, AgentProposedAction, AgentResult,
    AgentRunCheckpoint, AgentRunContext, AgentRunStatus, AgentSearchConfig, AgentToolCall,
    AgentToolContinuation, AgentToolResult, AgentUsage, AgentUsageClearInput,
    AgentUsageClearOutput, AgentUsageSummaryInput, AgentUsageSummaryOutput,
    ConversationTraceSnapshot, ConversationTurnTrace,
};
use serde_json::Value;
use tokio::sync::mpsc::UnboundedSender;

pub(crate) const AGENT_EVENT_NAME: &str = "agent.event";
pub(crate) const THINKING_PLACEHOLDER: &str = "正在思考...";
pub(crate) static ID_COUNTER: AtomicU64 = AtomicU64::new(1);
const MAX_CONVERSATION_CONTEXT_STATE_CACHE_ENTRIES: usize = 32;

pub type CoreServerNotificationSender = UnboundedSender<Value>;

struct ConversationContextStateEntry {
    state: AgentConversationContextState,
    configuration_revision: String,
    active_run_id: Option<String>,
    active_assistant_message_id: Option<String>,
    committed_trace_items: usize,
    terminal: bool,
    last_access: u64,
}

struct ConversationContextStateUpdate {
    baseline: AgentContextBaseline,
    snapshot: Option<AgentContextWindowSnapshot>,
}

#[derive(Clone)]
pub struct AgentService {
    storage: Arc<StorageService>,
    cancellations: Arc<Mutex<HashMap<String, AgentCancellationToken>>>,
    pending_actions: Arc<Mutex<HashMap<String, PendingActionRecord>>>,
    usage_contexts: Arc<Mutex<HashMap<String, AgentRunUsageState>>>,
    trace_snapshots: Arc<Mutex<HashMap<String, ConversationTraceSnapshot>>>,
    conversation_context_states: Arc<Mutex<HashMap<String, ConversationContextStateEntry>>>,
    conversation_context_state_clock: Arc<AtomicU64>,
    command_runs: CommandRunState,
    deleting_projects: Arc<Mutex<HashSet<String>>>,
}

impl AgentService {
    pub fn new(storage: Arc<StorageService>) -> Self {
        let pending_actions = load_persisted_pending_actions(&storage);
        Self {
            storage,
            cancellations: Arc::new(Mutex::new(HashMap::new())),
            pending_actions: Arc::new(Mutex::new(pending_actions)),
            usage_contexts: Arc::new(Mutex::new(HashMap::new())),
            trace_snapshots: Arc::new(Mutex::new(HashMap::new())),
            conversation_context_states: Arc::new(Mutex::new(HashMap::new())),
            conversation_context_state_clock: Arc::new(AtomicU64::new(1)),
            command_runs: CommandRunState::default(),
            deleting_projects: Arc::new(Mutex::new(HashSet::new())),
        }
    }

    pub fn start_conversation_turn(
        &self,
        input: AgentConversationTurnInput,
        notifications: CoreServerNotificationSender,
    ) -> Result<AgentConversationTurnOutput, String> {
        if self.is_project_deleting(input.project_id.as_deref()) {
            return Err("项目正在移除，无法开始新的 agent 运行。".to_string());
        }
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
        if self.is_agent_input_project_deleting(&prepared.agent_input) {
            self.discard_usage_context(&run_id);
            self.unregister_cancellation(&run_id);
            return Err("项目正在移除，无法开始新的 agent 运行。".to_string());
        }

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
                if let AgentEvent::ApprovalRequired {
                    run_id,
                    action,
                    checkpoint,
                } = &event
                {
                    let agent_input =
                        agent_input_with_run_checkpoint(&emitter_agent_input, checkpoint);
                    emitter_service.store_pending_action(
                        run_id,
                        &emitter_conversation_id,
                        &emitter_assistant_message_id,
                        action.clone(),
                        agent_input,
                    );
                }
                let _ = emitter_notifications.send(agent_event_notification(event));
            });

            let host_executor = service.host_action_executor(
                pending_agent_input.clone(),
                worker_run_id.clone(),
                Some(worker_conversation_id.clone()),
                Some(worker_assistant_message_id.clone()),
            );
            let trace_observer = service.trace_observer(
                &worker_run_id,
                &worker_conversation_id,
                &worker_assistant_message_id,
                prepared.usage_context.started_at,
                pending_agent_input.clone(),
                notifications.clone(),
            );
            let result = send_chat_with_host_executor_and_trace_observer(
                prepared.agent_input,
                worker_run_id.clone(),
                emitter,
                cancellation_token,
                host_executor,
                service.storage.clone(),
                Some(trace_observer),
            )
            .await;
            let keep_trace_snapshot = matches!(
                &result,
                Ok(output) if output.status == AgentRunStatus::WaitingForApproval
            );

            let deleting_projects = service
                .deleting_projects
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            if agent_input_project_id(&pending_agent_input)
                .is_some_and(|project_id| deleting_projects.contains(project_id))
            {
                drop(deleting_projects);
                service.discard_usage_context(&worker_run_id);
                service.unregister_cancellation(&worker_run_id);
                return;
            }

            match result {
                Ok(agent_output) => {
                    let committed_durable_context = is_terminal_run_status(agent_output.status);
                    let persisted = service.persist_final_assistant_output(
                        &worker_conversation_id,
                        &worker_assistant_message_id,
                        &agent_output,
                    );
                    if persisted.is_ok() && committed_durable_context {
                        service.emit_terminal_context_window_snapshot(
                            &notifications,
                            &pending_agent_input,
                            &worker_run_id,
                            &worker_conversation_id,
                            &worker_assistant_message_id,
                            if agent_output.status == AgentRunStatus::Cancelled {
                                ""
                            } else {
                                &agent_output.content
                            },
                        );
                    } else if let Err(error) = persisted {
                        let _ = notifications.send(agent_event_notification(AgentEvent::Error {
                            run_id: Some(worker_run_id.clone()),
                            message: format!("无法原子持久化 assistant 终态与会话轨迹：{error}"),
                            recoverable: true,
                            code: Some("conversation_trace_persistence_failed".to_string()),
                            details: None,
                        }));
                    }
                }
                Err(error) => {
                    let usage = error.usage().cloned();
                    let code = error.code().map(ToString::to_string);
                    let details = error.details().cloned();
                    let message = error.to_string();
                    let conversation_turn_trace =
                        error.conversation_turn_trace().cloned().unwrap_or_else(|| {
                            failed_conversation_trace_without_items(
                                &worker_run_id,
                                &worker_conversation_id,
                                &worker_assistant_message_id,
                                &message,
                            )
                        });
                    let persisted = service.persist_assistant_error(
                        &worker_conversation_id,
                        &worker_assistant_message_id,
                        &message,
                        usage.clone(),
                        &conversation_turn_trace,
                    );
                    if persisted.is_ok() {
                        service.emit_terminal_context_window_snapshot(
                            &notifications,
                            &pending_agent_input,
                            &worker_run_id,
                            &worker_conversation_id,
                            &worker_assistant_message_id,
                            &message,
                        );
                    } else if let Err(error) = persisted {
                        let _ = notifications.send(agent_event_notification(AgentEvent::Error {
                            run_id: Some(worker_run_id.clone()),
                            message: format!(
                                "无法原子持久化 assistant 失败终态与会话轨迹：{error}"
                            ),
                            recoverable: true,
                            code: Some("conversation_trace_persistence_failed".to_string()),
                            details: None,
                        }));
                    }
                    let _ = notifications.send(agent_event_notification(AgentEvent::Error {
                        run_id: Some(worker_run_id.clone()),
                        message: message.clone(),
                        recoverable: false,
                        code,
                        details,
                    }));
                    let _ = notifications.send(agent_event_notification(AgentEvent::Done {
                        run_id: worker_run_id.clone(),
                        success: false,
                        status: Some(AgentRunStatus::Failed),
                        content: Some(message),
                        usage,
                        finish_reason: None,
                        proposed_actions: Vec::new(),
                    }));
                }
            }

            drop(deleting_projects);
            if !keep_trace_snapshot {
                service.discard_trace_snapshot(&worker_run_id);
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

    pub fn delete_project(&self, project_id: &str) -> Result<(), String> {
        {
            let mut deleting_projects = self
                .deleting_projects
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            deleting_projects.insert(project_id.to_string());
        }

        let run_ids = {
            let usage_contexts = self
                .usage_contexts
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            usage_contexts
                .iter()
                .filter(|(_, state)| state.context.project_id.as_deref() == Some(project_id))
                .map(|(run_id, _)| run_id.clone())
                .collect::<Vec<_>>()
        };
        for run_id in run_ids {
            self.cancel_run(&run_id);
        }

        let result = self.storage.delete_project(project_id);
        if result.is_ok() {
            self.invalidate_all_conversation_context_states();
            {
                let mut pending_actions = self
                    .pending_actions
                    .lock()
                    .unwrap_or_else(|error| error.into_inner());
                pending_actions.retain(|_, record| {
                    !agent_input_belongs_to_project(&record.agent_input, project_id)
                });
            }
            let mut usage_contexts = self
                .usage_contexts
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            usage_contexts
                .retain(|_, state| state.context.project_id.as_deref() != Some(project_id));
        } else {
            let mut deleting_projects = self
                .deleting_projects
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            deleting_projects.remove(project_id);
        }
        result
    }

    pub fn delete_conversation(&self, conversation_id: &str) -> Result<(), String> {
        self.storage.delete_conversation(conversation_id)?;
        self.invalidate_conversation_context_state(conversation_id);
        Ok(())
    }

    pub fn delete_chat_messages(
        &self,
        conversation_id: &str,
        message_ids: &[String],
    ) -> Result<(), String> {
        self.storage
            .delete_chat_messages(conversation_id, message_ids)?;
        self.invalidate_conversation_context_state(conversation_id);
        Ok(())
    }

    pub async fn shutdown_active_runs(&self, timeout: Duration) -> (usize, bool) {
        let active_runs = {
            let cancellations = self
                .cancellations
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            cancellations
                .iter()
                .map(|(run_id, token)| (run_id.clone(), token.clone()))
                .collect::<Vec<_>>()
        };

        for (run_id, token) in &active_runs {
            token.cancel();
            self.command_runs.cancel_run(run_id);
        }

        if active_runs.is_empty() {
            return (0, false);
        }

        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            let active_run_count = self
                .cancellations
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .len();
            if active_run_count == 0 {
                return (active_runs.len(), false);
            }
            if tokio::time::Instant::now() >= deadline {
                let remaining_run_ids = self
                    .cancellations
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .keys()
                    .cloned()
                    .collect::<Vec<_>>();
                self.persist_forced_cancelled_runs(&remaining_run_ids);
                return (active_runs.len(), true);
            }

            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    fn persist_forced_cancelled_runs(&self, run_ids: &[String]) {
        const REASON: &str = "Core shutdown timed out while cancelling the active run.";
        let contexts = {
            let usage_contexts = self
                .usage_contexts
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            run_ids
                .iter()
                .filter_map(|run_id| {
                    usage_contexts
                        .get(run_id)
                        .map(|state| state.context.clone())
                })
                .collect::<Vec<_>>()
        };
        let snapshots = self
            .trace_snapshots
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clone();
        let completed_at = now_ms();
        for context in contexts {
            let trace = cancelled_conversation_trace_from_snapshot(
                snapshots.get(&context.run_id).cloned().unwrap_or_default(),
                &context.run_id,
                &context.conversation_id,
                &context.assistant_message_id,
                REASON,
            );
            let persisted = self.storage.finalize_chat_message_with_conversation_trace(
                &context.conversation_id,
                &context.assistant_message_id,
                "",
                status_for_run(AgentRunStatus::Cancelled),
                run_status_label(AgentRunStatus::Cancelled),
                &trace,
                context.started_at,
                completed_at,
            );
            if persisted.is_ok() {
                self.invalidate_conversation_context_state(&context.conversation_id);
                let _ = self.persist_run_usage(
                    &context.run_id,
                    AgentRunStatus::Cancelled,
                    None,
                    Some(REASON.to_string()),
                );
            } else if let Err(error) = persisted {
                eprintln!("failed to persist forced cancelled conversation trace: {error}");
            }
        }
    }

    fn host_action_executor(
        &self,
        agent_input: AgentChatInput,
        run_id: String,
        conversation_id: Option<String>,
        assistant_message_id: Option<String>,
    ) -> AgentHostActionExecutor {
        let service = self.clone();
        Arc::new(move |action, cancellation_token| {
            service.execute_auto_approved_action(
                agent_input.clone(),
                run_id.clone(),
                conversation_id.clone(),
                assistant_message_id.clone(),
                action,
                cancellation_token,
            )
        })
    }

    fn execute_auto_approved_action(
        &self,
        agent_input: AgentChatInput,
        run_id: String,
        conversation_id: Option<String>,
        assistant_message_id: Option<String>,
        action: AgentProposedAction,
        cancellation_token: AgentCancellationToken,
    ) -> AgentResult<AgentToolResult> {
        cancellation_token.check()?;
        if self.is_agent_input_project_deleting(&agent_input) {
            return Err(AgentError::cancelled());
        }
        let created_at = now_ms();
        match action {
            AgentProposedAction::Diff { diff } => {
                let deleting_projects = self
                    .deleting_projects
                    .lock()
                    .unwrap_or_else(|error| error.into_inner());
                if agent_input_project_id(&agent_input)
                    .is_some_and(|project_id| deleting_projects.contains(project_id))
                {
                    return Err(AgentError::cancelled());
                }
                cancellation_token.check()?;
                let action_id = diff.id.clone();
                let execution = approved_patch_execution_for_input(&agent_input, &action_id, &diff);
                self.record_auto_action_audit(
                    &run_id,
                    conversation_id,
                    assistant_message_id,
                    &agent_input,
                    AgentProposedAction::Diff { diff },
                    &execution.status,
                    execution.patch_result.as_ref(),
                    None,
                    Some(&execution.tool_result),
                    execution.tool_result.error.as_deref(),
                    created_at,
                    now_ms(),
                );
                drop(deleting_projects);
                Ok(execution.tool_result)
            }
            AgentProposedAction::Command { command } => {
                let workspace_root = workspace_root_optional(&agent_input);
                let permissions = permissions_from_input(&agent_input);
                let command_for_error = command.clone();
                let command_result = run_approved_command(
                    workspace_root.as_deref(),
                    &command,
                    permissions,
                    cancellation_token.clone(),
                    None,
                )
                .unwrap_or_else(|error| failed_command_result(&command_for_error, error));
                let deleting_projects = self
                    .deleting_projects
                    .lock()
                    .unwrap_or_else(|error| error.into_inner());
                if agent_input_project_id(&agent_input)
                    .is_some_and(|project_id| deleting_projects.contains(project_id))
                {
                    return Err(AgentError::cancelled());
                }
                let command_succeeded = command_result.error.is_none()
                    && !command_result.timed_out
                    && !command_result.cancelled
                    && command_result.exit_code == Some(0);
                let tool_result =
                    command_tool_result(&command.id, command_succeeded, &command_result);
                self.record_auto_action_audit(
                    &run_id,
                    conversation_id,
                    assistant_message_id,
                    &agent_input,
                    AgentProposedAction::Command { command },
                    if command_succeeded {
                        "completed"
                    } else {
                        "failed"
                    },
                    None,
                    Some(&command_result),
                    Some(&tool_result),
                    tool_result.error.as_deref(),
                    created_at,
                    now_ms(),
                );
                drop(deleting_projects);
                Ok(tool_result)
            }
            AgentProposedAction::FileWrite { file_write } => {
                let deleting_projects = self
                    .deleting_projects
                    .lock()
                    .unwrap_or_else(|error| error.into_inner());
                if agent_input_project_id(&agent_input)
                    .is_some_and(|project_id| deleting_projects.contains(project_id))
                {
                    return Err(AgentError::cancelled());
                }
                cancellation_token.check()?;
                let action = AgentProposedAction::FileWrite {
                    file_write: file_write.clone(),
                };
                let execution =
                    approved_file_write_execution(&self.storage, &agent_input, &file_write);
                self.record_auto_action_audit(
                    &run_id,
                    conversation_id,
                    assistant_message_id,
                    &agent_input,
                    action,
                    &execution.status,
                    None,
                    None,
                    Some(&execution.tool_result),
                    execution.tool_result.error.as_deref(),
                    created_at,
                    now_ms(),
                );
                drop(deleting_projects);
                Ok(execution.tool_result)
            }
            AgentProposedAction::ToolCall { call } => Ok(AgentToolResult {
                call_id: call.id,
                tool: call.tool,
                ok: false,
                result: None,
                error: Some(
                    "自动批准执行器只支持结构化 apply_patch diff 和 run_command。".to_string(),
                ),
            }),
        }
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

    pub fn read_file_draft(
        &self,
        draft_id: &str,
        offset: Option<usize>,
        max_chars: Option<usize>,
    ) -> Result<AgentFileDraftContentPage, String> {
        let draft = self
            .storage
            .get_agent_file_draft(draft_id)?
            .ok_or_else(|| format!("未找到文件草稿：{draft_id}"))?;
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
        offset: Option<usize>,
        max_chars: Option<usize>,
    ) -> Result<AgentFileWriteDiffPage, String> {
        let draft = self
            .storage
            .get_agent_file_draft(draft_id)?
            .ok_or_else(|| format!("未找到文件草稿：{draft_id}"))?;
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
        let deleting_projects = self
            .deleting_projects
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let mut pending_actions = self
            .pending_actions
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let Some(record) = pending_actions.get_mut(action_id) else {
            return Ok(false);
        };
        if agent_input_project_id(&record.agent_input)
            .is_some_and(|project_id| deleting_projects.contains(project_id))
        {
            return Ok(false);
        }
        if record.snapshot.status == PendingActionStatus::Approved
            && matches!(record.snapshot.action, AgentProposedAction::Command { .. })
        {
            let cancelled = self.command_runs.cancel(action_id);
            if cancelled {
                record.snapshot.status = PendingActionStatus::Cancelled;
                self.persist_pending_status(action_id, PendingActionStatus::Cancelled);
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
        self.persist_pending_status(action_id, PendingActionStatus::Cancelled);
        let record = record.clone();
        drop(pending_actions);
        drop(deleting_projects);
        if let Err(error) = self.finalize_cancelled_pending_action(&record) {
            let mut pending_actions = self
                .pending_actions
                .lock()
                .unwrap_or_else(|lock_error| lock_error.into_inner());
            if let Some(pending) = pending_actions.get_mut(action_id) {
                pending.snapshot.status = PendingActionStatus::Pending;
            }
            drop(pending_actions);
            self.persist_pending_status(action_id, PendingActionStatus::Pending);
            return Err(error);
        }
        Ok(true)
    }

    fn finalize_cancelled_pending_action(
        &self,
        record: &PendingActionRecord,
    ) -> Result<(), String> {
        const REASON: &str = "Pending action was cancelled by the user.";
        let mut call = tool_call_for_action(&record.snapshot.action);
        call.approval_status = AgentApprovalStatus::Rejected;
        let execution = action_execution_for_decision(
            &self.storage,
            record,
            &call,
            AgentApprovalDecisionStatus::Rejected,
            Some(REASON),
        );
        let completed_at = now_ms();
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

        let (Some(conversation_id), Some(assistant_message_id), Some(checkpoint)) = (
            record.snapshot.conversation_id.as_deref(),
            record.snapshot.assistant_message_id.as_deref(),
            record.agent_input.resume_checkpoint.as_ref(),
        ) else {
            return Ok(());
        };
        let trace = cancelled_conversation_trace_from_checkpoint(
            checkpoint,
            conversation_id,
            assistant_message_id,
            &call,
            &execution.tool_result,
            REASON,
        );
        self.storage.finalize_chat_message_with_conversation_trace(
            conversation_id,
            assistant_message_id,
            "",
            status_for_run(AgentRunStatus::Cancelled),
            run_status_label(AgentRunStatus::Cancelled),
            &trace,
            completed_at,
            completed_at,
        )?;
        self.persist_run_usage(
            &record.snapshot.run_id,
            AgentRunStatus::Cancelled,
            None,
            Some(REASON.to_string()),
        )?;
        self.invalidate_conversation_context_state(conversation_id);
        self.discard_trace_snapshot(&record.snapshot.run_id);
        Ok(())
    }

    fn queue_action_continuation(
        &self,
        action_id: &str,
        pending_status: PendingActionStatus,
        decision_status: AgentApprovalDecisionStatus,
        message: Option<String>,
        notifications: CoreServerNotificationSender,
    ) -> Result<AgentActionExecutionOutput, String> {
        let deleting_projects = self
            .deleting_projects
            .lock()
            .unwrap_or_else(|error| error.into_inner());
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
            if agent_input_project_id(&record.agent_input)
                .is_some_and(|project_id| deleting_projects.contains(project_id))
            {
                return Err("项目正在移除，无法处理待审批操作。".to_string());
            }
            record.snapshot.status = pending_status;
            self.persist_pending_status(action_id, pending_status);
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
            drop(deleting_projects);
            return self.queue_command_execution(record, call, notifications);
        }

        let execution = action_execution_for_decision(
            &self.storage,
            &record,
            &call,
            decision_status,
            message.as_deref(),
        );
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
        drop(deleting_projects);

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
        self.commit_trace_snapshot_with_continuation(&record, &agent_input, &notifications)?;
        if matches!(
            record.snapshot.action,
            AgentProposedAction::FileWrite { .. }
        ) {
            let _ = notifications.send(agent_event_notification(AgentEvent::ToolResult {
                run_id: record.snapshot.run_id.clone(),
                result: tool_result.clone(),
            }));
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
                    None,
                )
                .await;
        });

        Ok(AgentActionExecutionOutput {
            action_id: record.snapshot.action_id,
            action_type: record.snapshot.action_type,
            tool_name: record.snapshot.tool_name,
            status: execution.status,
            patch_result: execution.patch_result,
            file_write_result: execution.file_write_result,
            command_result: None,
            tool_result: Some(execution.tool_result),
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
            file_write_result: None,
            command_result: None,
            tool_result: None,
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

    async fn run_command_execution(
        &self,
        record: PendingActionRecord,
        call: AgentToolCall,
        notifications: CoreServerNotificationSender,
    ) {
        let run_id = record.snapshot.run_id.clone();
        let action_id = record.snapshot.action_id.clone();
        self.seed_trace_snapshot_from_checkpoint(
            &run_id,
            record.agent_input.resume_checkpoint.as_ref(),
        );
        if self.is_agent_input_project_deleting(&record.agent_input) {
            self.discard_usage_context(&run_id);
            return;
        }
        let AgentProposedAction::Command { command } = record.snapshot.action.clone() else {
            self.update_pending_status(&action_id, PendingActionStatus::Failed);
            return;
        };

        let cancellation_token = AgentCancellationToken::new();
        self.register_cancellation(&run_id, cancellation_token.clone());
        if self.is_agent_input_project_deleting(&record.agent_input) {
            cancellation_token.cancel();
            self.unregister_cancellation(&run_id);
            self.discard_usage_context(&run_id);
            return;
        }

        let guard = self.command_runs.register(&action_id, &run_id);
        let cancel_flag = guard.cancel_flag();
        let run_cancellation_token = cancellation_token.clone();
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
        let run_was_cancelled = run_cancellation_token.is_cancelled();

        let (agent_input, final_pending_status) = {
            let deleting_projects = self
                .deleting_projects
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            if agent_input_project_id(&record.agent_input)
                .is_some_and(|project_id| deleting_projects.contains(project_id))
            {
                self.discard_usage_context(&run_id);
                self.unregister_cancellation(&run_id);
                return;
            }

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
            (agent_input, final_pending_status)
        };
        if let Err(error) =
            self.commit_trace_snapshot_with_continuation(&record, &agent_input, &notifications)
        {
            self.update_pending_status(&action_id, PendingActionStatus::Failed);
            self.unregister_cancellation(&run_id);
            let _ = notifications.send(agent_event_notification(AgentEvent::Error {
                run_id: Some(run_id),
                message: format!("命令结果无法写入会话轨迹：{error}"),
                recoverable: true,
                code: Some("conversation_trace_persistence_failed".to_string()),
                details: None,
            }));
            return;
        }
        if let Some(continuation) = agent_input.tool_continuation.as_ref() {
            let _ = notifications.send(agent_event_notification(AgentEvent::ToolResult {
                run_id: run_id.clone(),
                result: continuation.result.clone(),
            }));
        }

        if run_was_cancelled {
            const REASON: &str =
                "Agent run was cancelled while the approved command was executing.";
            let persisted = if let (
                Some(conversation_id),
                Some(assistant_message_id),
                Some(checkpoint),
                Some(continuation),
            ) = (
                record.snapshot.conversation_id.as_deref(),
                record.snapshot.assistant_message_id.as_deref(),
                record.agent_input.resume_checkpoint.as_ref(),
                agent_input.tool_continuation.as_ref(),
            ) {
                let trace = cancelled_conversation_trace_from_checkpoint(
                    checkpoint,
                    conversation_id,
                    assistant_message_id,
                    &continuation.call,
                    &continuation.result,
                    REASON,
                );
                let output = AgentChatOutput {
                    content: String::new(),
                    status: AgentRunStatus::Cancelled,
                    run_id: run_id.clone(),
                    events: Vec::new(),
                    tool_definitions: Vec::new(),
                    todo: None,
                    usage: None,
                    finish_reason: Some(REASON.to_string()),
                    proposed_actions: Vec::new(),
                    conversation_turn_trace: Some(trace),
                };
                self.persist_final_assistant_output(conversation_id, assistant_message_id, &output)
            } else {
                Err("cancelled command is missing its conversation trace checkpoint".to_string())
            };
            if let Err(error) = persisted {
                self.update_pending_status(&action_id, PendingActionStatus::Failed);
                self.unregister_cancellation(&run_id);
                let _ = notifications.send(agent_event_notification(AgentEvent::Error {
                    run_id: Some(run_id),
                    message: format!(
                        "无法原子持久化已取消命令的 assistant 终态与会话轨迹：{error}"
                    ),
                    recoverable: true,
                    code: Some("conversation_trace_persistence_failed".to_string()),
                    details: None,
                }));
                return;
            }
            self.update_pending_status(&action_id, PendingActionStatus::Cancelled);
            self.discard_trace_snapshot(&run_id);
            self.unregister_cancellation(&run_id);
            let _ = notifications.send(agent_event_notification(AgentEvent::Done {
                run_id,
                success: false,
                status: Some(AgentRunStatus::Cancelled),
                content: None,
                usage: None,
                finish_reason: Some(REASON.to_string()),
                proposed_actions: Vec::new(),
            }));
            return;
        }

        self.run_action_continuation(
            record,
            agent_input,
            notifications,
            final_pending_status,
            Some(run_cancellation_token),
        )
        .await;
    }

    async fn run_action_continuation(
        &self,
        record: PendingActionRecord,
        agent_input: AgentChatInput,
        notifications: CoreServerNotificationSender,
        final_pending_status: PendingActionStatus,
        existing_cancellation_token: Option<AgentCancellationToken>,
    ) {
        let run_id = record.snapshot.run_id.clone();
        if self.is_agent_input_project_deleting(&record.agent_input) {
            self.discard_usage_context(&run_id);
            self.unregister_cancellation(&run_id);
            return;
        }
        let cancellation_token = existing_cancellation_token.unwrap_or_default();
        self.register_cancellation(&run_id, cancellation_token.clone());
        if self.is_agent_input_project_deleting(&record.agent_input) {
            cancellation_token.cancel();
            self.unregister_cancellation(&run_id);
            self.discard_usage_context(&run_id);
            return;
        }

        let emitter_notifications = notifications.clone();
        let emitter_service = self.clone();
        let emitter_conversation_id = record.snapshot.conversation_id.clone();
        let emitter_assistant_message_id = record.snapshot.assistant_message_id.clone();
        let emitter_agent_input = record.agent_input.clone();
        let emitter: AgentEventEmitter = Arc::new(move |event| {
            if let AgentEvent::ApprovalRequired {
                run_id,
                action,
                checkpoint,
            } = &event
            {
                let agent_input = agent_input_with_run_checkpoint(&emitter_agent_input, checkpoint);
                emitter_service.store_pending_action(
                    run_id,
                    emitter_conversation_id.as_deref().unwrap_or_default(),
                    emitter_assistant_message_id.as_deref().unwrap_or_default(),
                    action.clone(),
                    agent_input,
                );
            }
            let _ = emitter_notifications.send(agent_event_notification(event));
        });

        let host_executor = self.host_action_executor(
            agent_input.clone(),
            run_id.clone(),
            record.snapshot.conversation_id.clone(),
            record.snapshot.assistant_message_id.clone(),
        );
        let trace_conversation_id = record
            .snapshot
            .conversation_id
            .as_deref()
            .unwrap_or_default();
        let trace_assistant_message_id = record
            .snapshot
            .assistant_message_id
            .as_deref()
            .unwrap_or_default();
        let trace_observer = self.trace_observer(
            &run_id,
            trace_conversation_id,
            trace_assistant_message_id,
            record.snapshot.created_at,
            record.agent_input.clone(),
            notifications.clone(),
        );
        let result = send_chat_with_host_executor_and_trace_observer(
            agent_input,
            run_id.clone(),
            emitter,
            cancellation_token,
            host_executor,
            self.storage.clone(),
            Some(trace_observer),
        )
        .await;
        let keep_trace_snapshot = matches!(
            &result,
            Ok(output) if output.status == AgentRunStatus::WaitingForApproval
        );

        let deleting_projects = self
            .deleting_projects
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if agent_input_project_id(&record.agent_input)
            .is_some_and(|project_id| deleting_projects.contains(project_id))
        {
            drop(deleting_projects);
            self.discard_usage_context(&run_id);
            self.unregister_cancellation(&run_id);
            return;
        }

        match result {
            Ok(agent_output) => {
                let committed_durable_context = is_terminal_run_status(agent_output.status);
                if let (Some(conversation_id), Some(assistant_message_id)) = (
                    record.snapshot.conversation_id.as_deref(),
                    record.snapshot.assistant_message_id.as_deref(),
                ) {
                    let persisted = self.persist_final_assistant_output(
                        conversation_id,
                        assistant_message_id,
                        &agent_output,
                    );
                    if persisted.is_ok() && committed_durable_context {
                        self.emit_terminal_context_window_snapshot(
                            &notifications,
                            &record.agent_input,
                            &run_id,
                            conversation_id,
                            assistant_message_id,
                            if agent_output.status == AgentRunStatus::Cancelled {
                                ""
                            } else {
                                &agent_output.content
                            },
                        );
                    } else if let Err(error) = persisted {
                        let _ = notifications.send(agent_event_notification(AgentEvent::Error {
                            run_id: Some(run_id.clone()),
                            message: format!("无法原子持久化 assistant 终态与会话轨迹：{error}"),
                            recoverable: true,
                            code: Some("conversation_trace_persistence_failed".to_string()),
                            details: None,
                        }));
                    }
                }
                self.update_pending_status(&record.snapshot.action_id, final_pending_status);
            }
            Err(error) => {
                let usage = error.usage().cloned();
                let code = error.code().map(ToString::to_string);
                let details = error.details().cloned();
                let message = error.to_string();
                let conversation_turn_trace = match (
                    error.conversation_turn_trace().cloned(),
                    record.snapshot.conversation_id.as_deref(),
                    record.snapshot.assistant_message_id.as_deref(),
                ) {
                    (Some(trace), _, _) => Some(trace),
                    (None, Some(conversation_id), Some(assistant_message_id)) => {
                        Some(failed_conversation_trace_without_items(
                            &run_id,
                            conversation_id,
                            assistant_message_id,
                            &message,
                        ))
                    }
                    _ => None,
                };
                if let (Some(conversation_id), Some(assistant_message_id)) = (
                    record.snapshot.conversation_id.as_deref(),
                    record.snapshot.assistant_message_id.as_deref(),
                ) {
                    let persisted = self.persist_assistant_error(
                        conversation_id,
                        assistant_message_id,
                        &message,
                        usage.clone(),
                        conversation_turn_trace
                            .as_ref()
                            .expect("trace exists when conversation and assistant ids exist"),
                    );
                    if persisted.is_ok() {
                        self.emit_terminal_context_window_snapshot(
                            &notifications,
                            &record.agent_input,
                            &run_id,
                            conversation_id,
                            assistant_message_id,
                            &message,
                        );
                    } else if let Err(error) = persisted {
                        let _ = notifications.send(agent_event_notification(AgentEvent::Error {
                            run_id: Some(run_id.clone()),
                            message: format!(
                                "无法原子持久化 assistant 失败终态与会话轨迹：{error}"
                            ),
                            recoverable: true,
                            code: Some("conversation_trace_persistence_failed".to_string()),
                            details: None,
                        }));
                    }
                }
                let _ = notifications.send(agent_event_notification(AgentEvent::Error {
                    run_id: Some(run_id.clone()),
                    message: message.clone(),
                    recoverable: false,
                    code,
                    details,
                }));
                let _ = notifications.send(agent_event_notification(AgentEvent::Done {
                    run_id: run_id.clone(),
                    success: false,
                    status: Some(AgentRunStatus::Failed),
                    content: Some(message),
                    usage,
                    finish_reason: None,
                    proposed_actions: Vec::new(),
                }));
                self.update_pending_status(&record.snapshot.action_id, PendingActionStatus::Failed);
            }
        }

        drop(deleting_projects);
        if !keep_trace_snapshot {
            self.discard_trace_snapshot(&run_id);
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
        let deleting_projects = self
            .deleting_projects
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if agent_input_project_id(&agent_input)
            .is_some_and(|project_id| deleting_projects.contains(project_id))
        {
            return;
        }
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
        if let Err(error) = self.persist_pending_action(&pending_record) {
            eprintln!("failed to persist pending agent action: {error}");
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
        drop(deleting_projects);
    }

    fn update_pending_status(&self, action_id: &str, status: PendingActionStatus) {
        let mut pending_actions = self
            .pending_actions
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if let Some(record) = pending_actions.get_mut(action_id) {
            record.snapshot.status = status;
        }
        drop(pending_actions);
        self.persist_pending_status(action_id, status);
    }

    fn persist_pending_action(&self, record: &PendingActionRecord) -> Result<(), String> {
        self.storage
            .upsert_pending_agent_action(pending_storage_record(record, now_ms()))
    }

    fn persist_pending_status(&self, action_id: &str, status: PendingActionStatus) {
        if let Err(error) = self.storage.update_pending_agent_action_status(
            action_id,
            pending_status_label(status),
            now_ms(),
        ) {
            eprintln!("failed to update pending agent action status: {error}");
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
            effective_permissions_json: Some(serialize_json(&permissions_from_input(
                &record.agent_input,
            ))),
            path_scope: path_scope_for_action(&record.agent_input, &record.snapshot.action),
            command_cwd_scope: command_cwd_scope_for_action(
                &record.agent_input,
                &record.snapshot.action,
            ),
            blocked_reason: error.map(ToString::to_string),
            decision_source: Some(
                if decision.is_none() && status == "pending" {
                    "manual_pending"
                } else {
                    "manual"
                }
                .to_string(),
            ),
        };
        if let Err(error) = self.storage.upsert_agent_action_audit(audit) {
            eprintln!("failed to write agent action audit log: {error}");
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn record_auto_action_audit(
        &self,
        run_id: &str,
        conversation_id: Option<String>,
        assistant_message_id: Option<String>,
        agent_input: &AgentChatInput,
        action: AgentProposedAction,
        status: &str,
        patch_result: Option<&AgentPatchResult>,
        command_result: Option<&AgentCommandExecutionResult>,
        tool_result: Option<&AgentToolResult>,
        error: Option<&str>,
        created_at: i64,
        completed_at: i64,
    ) {
        let audit = AgentActionAuditRecord {
            action_id: action_id_for_action(&action),
            run_id: run_id.to_string(),
            conversation_id,
            assistant_message_id,
            action_type: action_type_for_action(&action).to_string(),
            tool_name: tool_name_for_action(&action),
            decision: Some("approved".to_string()),
            status: status.to_string(),
            action_json: serialize_json(&action),
            patch_result_json: patch_result.map(serialize_json),
            command_result_json: command_result.map(serialize_json),
            tool_result_json: tool_result.map(serialize_json),
            error: error.map(ToString::to_string),
            created_at,
            decided_at: Some(created_at),
            completed_at: Some(completed_at),
            effective_permissions_json: Some(serialize_json(&permissions_from_input(agent_input))),
            path_scope: path_scope_for_action(agent_input, &action),
            command_cwd_scope: command_cwd_scope_for_action(agent_input, &action),
            blocked_reason: error.map(ToString::to_string),
            decision_source: Some("auto".to_string()),
        };
        if let Err(error) = self.storage.upsert_agent_action_audit(audit) {
            eprintln!("failed to write auto agent action audit log: {error}");
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

    fn is_agent_input_project_deleting(&self, agent_input: &AgentChatInput) -> bool {
        self.is_project_deleting(agent_input_project_id(agent_input))
    }

    fn is_project_deleting(&self, project_id: Option<&str>) -> bool {
        let Some(project_id) = project_id else {
            return false;
        };
        self.deleting_projects
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .contains(project_id)
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

    #[allow(clippy::too_many_arguments)]
    fn trace_observer(
        &self,
        run_id: &str,
        conversation_id: &str,
        assistant_message_id: &str,
        created_at: i64,
        agent_input: AgentChatInput,
        notifications: CoreServerNotificationSender,
    ) -> AgentConversationTraceObserver {
        let service = self.clone();
        let snapshots = self.trace_snapshots.clone();
        let run_id = run_id.to_string();
        let conversation_id = conversation_id.to_string();
        let assistant_message_id = assistant_message_id.to_string();
        let configuration_revision = conversation_context_configuration_revision(&agent_input)
            .map_err(|error| error.to_string());
        Arc::new(move |snapshot| {
            snapshots
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .insert(run_id.clone(), snapshot.clone());
            let configuration_revision = configuration_revision
                .as_deref()
                .map_err(|error| AgentError::new(format!("无法准备会话上下文状态：{error}")))?;
            service
                .persist_in_progress_trace_snapshot(
                    &run_id,
                    &conversation_id,
                    &assistant_message_id,
                    created_at,
                    &agent_input,
                    &notifications,
                    &snapshot,
                    configuration_revision,
                )
                .map_err(|error| AgentError::new(format!("无法增量持久化运行中会话轨迹：{error}")))
        })
    }

    #[allow(clippy::too_many_arguments)]
    fn persist_in_progress_trace_snapshot(
        &self,
        run_id: &str,
        conversation_id: &str,
        assistant_message_id: &str,
        created_at: i64,
        agent_input: &AgentChatInput,
        notifications: &CoreServerNotificationSender,
        snapshot: &ConversationTraceSnapshot,
        configuration_revision: &str,
    ) -> Result<Option<AgentContextBaseline>, String> {
        let trace = snapshot.in_progress_trace(run_id, conversation_id, assistant_message_id);
        let changed = self.storage.append_in_progress_conversation_turn_trace(
            &trace,
            created_at,
            now_ms(),
        )?;
        let update = self.update_running_conversation_context_state(
            agent_input,
            run_id,
            conversation_id,
            assistant_message_id,
            &trace,
            configuration_revision,
        )?;
        if changed {
            self.emit_context_window_snapshot(
                notifications,
                run_id,
                conversation_id,
                update.snapshot,
            );
        }
        Ok(Some(update.baseline))
    }

    fn update_running_conversation_context_state(
        &self,
        agent_input: &AgentChatInput,
        run_id: &str,
        conversation_id: &str,
        assistant_message_id: &str,
        trace: &ConversationTurnTrace,
        configuration_revision: &str,
    ) -> Result<ConversationContextStateUpdate, String> {
        let access = self.next_conversation_context_state_access();
        let needs_rebuild;
        {
            let mut states = self
                .conversation_context_states
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            if let Some(entry) = states.get_mut(conversation_id) {
                if entry.configuration_revision != configuration_revision {
                    needs_rebuild = true;
                } else {
                    let update = if !entry.terminal
                        && entry.active_run_id.as_deref() == Some(run_id)
                        && entry.active_assistant_message_id.as_deref()
                            == Some(assistant_message_id)
                    {
                        entry
                            .state
                            .append_trace_items(trace, entry.committed_trace_items)
                    } else if entry.terminal
                        && entry.active_assistant_message_id.as_deref()
                            != Some(assistant_message_id)
                    {
                        let current_user = agent_input
                            .messages
                            .iter()
                            .rev()
                            .find(|message| message.role == "user")
                            .map(|message| message.content.as_str())
                            .unwrap_or_default();
                        entry.state.append_user_message(current_user);
                        entry.active_run_id = Some(run_id.to_string());
                        entry.active_assistant_message_id = Some(assistant_message_id.to_string());
                        entry.committed_trace_items = 0;
                        entry.terminal = false;
                        entry.state.append_trace_items(trace, 0)
                    } else {
                        Err(AgentError::new("会话上下文状态与当前运行身份不一致。"))
                    };
                    match update {
                        Ok(committed_trace_items) => {
                            entry.active_run_id = Some(run_id.to_string());
                            entry.committed_trace_items = committed_trace_items;
                            entry.last_access = access;
                            let baseline = entry
                                .state
                                .shared_baseline()
                                .map_err(|error| error.to_string())?;
                            let snapshot =
                                agent_input.context_window_indicator_enabled.then(|| {
                                    entry.state.snapshot(AgentContextWindowPhase::DurableCommit)
                                });
                            return Ok(ConversationContextStateUpdate { baseline, snapshot });
                        }
                        Err(_) => needs_rebuild = true,
                    }
                }
            } else {
                needs_rebuild = true;
            }
            if needs_rebuild {
                states.remove(conversation_id);
            }
        }

        self.rebuild_conversation_context_state(
            agent_input,
            conversation_id,
            AgentContextWindowPhase::DurableCommit,
            Some(run_id),
        )
    }

    fn seed_trace_snapshot_from_checkpoint(
        &self,
        run_id: &str,
        checkpoint: Option<&AgentRunCheckpoint>,
    ) {
        let Some(checkpoint) = checkpoint else {
            return;
        };
        self.trace_snapshots
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .entry(run_id.to_string())
            .or_insert_with(|| ConversationTraceSnapshot {
                items: checkpoint.conversation_trace_items.clone(),
                next_sequence: checkpoint.next_conversation_trace_sequence,
                truncated: checkpoint.conversation_trace_truncated,
            });
    }

    fn commit_trace_snapshot_with_continuation(
        &self,
        record: &PendingActionRecord,
        agent_input: &AgentChatInput,
        notifications: &CoreServerNotificationSender,
    ) -> Result<(), String> {
        let checkpoint = record
            .agent_input
            .resume_checkpoint
            .as_ref()
            .ok_or_else(|| "审批续跑缺少会话轨迹检查点。".to_string())?;
        let continuation = agent_input
            .tool_continuation
            .as_ref()
            .ok_or_else(|| "审批续跑缺少工具结果。".to_string())?;
        let snapshot = conversation_trace_snapshot_from_checkpoint_and_continuation(
            checkpoint,
            &continuation.call,
            &continuation.result,
        );
        let run_id = &record.snapshot.run_id;
        let conversation_id = record
            .snapshot
            .conversation_id
            .as_deref()
            .ok_or_else(|| "审批续跑缺少 conversation id。".to_string())?;
        let assistant_message_id = record
            .snapshot
            .assistant_message_id
            .as_deref()
            .ok_or_else(|| "审批续跑缺少 assistant message id。".to_string())?;
        self.trace_snapshots
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .insert(run_id.to_string(), snapshot.clone());
        let configuration_revision =
            conversation_context_configuration_revision(&record.agent_input)
                .map_err(|error| error.to_string())?;
        self.persist_in_progress_trace_snapshot(
            run_id,
            conversation_id,
            assistant_message_id,
            record.snapshot.created_at,
            &record.agent_input,
            notifications,
            &snapshot,
            &configuration_revision,
        )
        .map(|_| ())
    }

    fn discard_trace_snapshot(&self, run_id: &str) {
        self.trace_snapshots
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .remove(run_id);
    }

    fn discard_usage_context(&self, run_id: &str) {
        let mut contexts = self
            .usage_contexts
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        contexts.remove(run_id);
        drop(contexts);
        self.discard_trace_snapshot(run_id);
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
                output_thinking_tokens: usage
                    .as_ref()
                    .and_then(|usage| usage.output_thinking_tokens),
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
        let completed_at = now_ms();
        if matches!(
            output.status,
            AgentRunStatus::Completed | AgentRunStatus::Failed | AgentRunStatus::Cancelled
        ) {
            let trace =
                output
                    .conversation_turn_trace
                    .clone()
                    .unwrap_or_else(|| match output.status {
                        AgentRunStatus::Completed => completed_conversation_trace_without_items(
                            &output.run_id,
                            conversation_id,
                            assistant_message_id,
                        ),
                        AgentRunStatus::Cancelled => cancelled_conversation_trace_without_items(
                            &output.run_id,
                            conversation_id,
                            assistant_message_id,
                            output
                                .finish_reason
                                .as_deref()
                                .unwrap_or("Agent run was cancelled."),
                        ),
                        AgentRunStatus::Failed => failed_conversation_trace_without_items(
                            &output.run_id,
                            conversation_id,
                            assistant_message_id,
                            output
                                .finish_reason
                                .as_deref()
                                .unwrap_or("Agent run failed."),
                        ),
                        _ => unreachable!(),
                    });
            self.storage.finalize_chat_message_with_conversation_trace(
                conversation_id,
                assistant_message_id,
                if output.status == AgentRunStatus::Cancelled {
                    ""
                } else {
                    &output.content
                },
                status_for_run(output.status),
                run_status_label(output.status),
                &trace,
                completed_at,
                completed_at,
            )?;
            return self.persist_run_usage(
                &output.run_id,
                output.status,
                output.usage.clone(),
                output.finish_reason.clone(),
            );
        }
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
            completed_at,
        )
    }

    fn persist_assistant_error(
        &self,
        conversation_id: &str,
        assistant_message_id: &str,
        message: &str,
        usage: Option<AgentUsage>,
        conversation_turn_trace: &ConversationTurnTrace,
    ) -> Result<(), String> {
        let run_id = self.find_usage_run_id(conversation_id, assistant_message_id);
        let completed_at = now_ms();
        self.storage.finalize_chat_message_with_conversation_trace(
            conversation_id,
            assistant_message_id,
            message,
            Some("error"),
            run_status_label(AgentRunStatus::Failed),
            conversation_turn_trace,
            completed_at,
            completed_at,
        )?;
        if let Some(run_id) = run_id {
            self.persist_run_usage(
                &run_id,
                AgentRunStatus::Failed,
                usage,
                Some(message.to_string()),
            )?;
        }
        Ok(())
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

    pub fn get_context_window_snapshot(
        &self,
        input: AgentContextWindowSnapshotInput,
    ) -> Result<AgentContextWindowSnapshotOutput, String> {
        let model_id = input.model_id.trim();
        if model_id.is_empty() {
            return Err("modelId 不能为空。".to_string());
        }
        let settings = self
            .storage
            .load_model_settings()?
            .ok_or_else(|| "请先配置模型。".to_string())?;
        let model = settings
            .models
            .iter()
            .find(|model| model.id == model_id)
            .cloned()
            .ok_or_else(|| format!("未找到模型配置：{model_id}"))?;
        if !model.enabled {
            return Err(format!("模型未启用：{model_id}"));
        }

        let conversation_id = normalized_optional(input.conversation_id.as_deref());
        let conversation = match conversation_id.as_deref() {
            Some(conversation_id) => self.storage.load_conversation(conversation_id)?,
            None => None,
        };
        let project_id = normalized_optional(input.project_id.as_deref()).or_else(|| {
            conversation
                .as_ref()
                .and_then(|conversation| normalized_optional(conversation.project_id.as_deref()))
        });
        let project = resolve_project(&self.storage, project_id.as_deref())?;
        let attachment_library = conversation_id
            .as_deref()
            .map(|conversation_id| {
                self.storage
                    .build_attachment_library_context(conversation_id, project_id.as_deref())
            })
            .transpose()?;
        let messages = match conversation.as_ref() {
            Some(conversation) => {
                let traces = self
                    .storage
                    .list_conversation_turn_traces(&conversation.id)?;
                conversation_history_messages(conversation, &traces, &[])
            }
            None => Vec::new(),
        };
        let prompt_preferences = match input.prompt_preferences {
            Some(preferences) => preferences,
            None => {
                agent_prompt_preferences_from_record(self.storage.load_agent_prompt_preferences()?)
            }
        };
        let agent_input = AgentChatInput {
            api_url: settings.api_url.trim().to_string(),
            api_token: String::new(),
            model: model_provider_path(&model),
            api_style: None,
            context_window_tokens: model.context_window_tokens,
            context_window_indicator_enabled: true,
            max_tokens: input.max_tokens,
            temperature: None,
            stream: Some(false),
            context: Some(AgentRunContext {
                conversation_id: conversation_id.clone(),
                project_id: project_id.clone(),
                workspace: project
                    .as_ref()
                    .map(|project| mycopilot_core::AgentWorkspaceContext {
                        project_id: Some(project.id.clone()),
                        display_name: Some(project.name.clone()),
                        root_path: project.path.clone(),
                    }),
                attachment_library,
                permissions: input.permissions,
            }),
            search_config: Some(AgentSearchConfig {
                mode: search_mode_from_storage(&settings.search_mode),
                tavily_api_key: non_empty(settings.tavily_api_key),
            }),
            prompt_preferences: Some(prompt_preferences),
            approval_decision: None,
            tool_continuation: None,
            attachments: Vec::new(),
            resume_checkpoint: None,
            assistant_message_id: None,
            messages,
        };

        let snapshot = match conversation_id.as_deref() {
            Some(conversation_id) => self.context_window_snapshot_with_cache(
                &agent_input,
                conversation_id,
                AgentContextWindowPhase::Idle,
            )?,
            None => inspect_context_window(agent_input).map_err(|error| error.to_string())?,
        };
        Ok(AgentContextWindowSnapshotOutput { snapshot })
    }

    fn persisted_conversation_context_state(
        &self,
        agent_input: &AgentChatInput,
        conversation_id: &str,
    ) -> Result<(AgentChatInput, Vec<ConversationTurnTrace>), String> {
        let conversation = self
            .storage
            .load_conversation(conversation_id)?
            .ok_or_else(|| format!("未找到对话：{conversation_id}"))?;
        let mut preview_input = agent_input.clone();
        let traces = self
            .storage
            .list_conversation_turn_traces(conversation_id)?;
        preview_input.messages = conversation_history_messages(&conversation, &traces, &[]);
        preview_input.attachments.clear();
        preview_input.approval_decision = None;
        preview_input.tool_continuation = None;
        preview_input.resume_checkpoint = None;
        if let Some(context) = preview_input.context.as_mut() {
            context.conversation_id = Some(conversation_id.to_string());
            context.attachment_library = Some(self.storage.build_attachment_library_context(
                conversation_id,
                context.project_id.as_deref(),
            )?);
        }
        Ok((preview_input, traces))
    }

    fn context_window_snapshot_with_cache(
        &self,
        agent_input: &AgentChatInput,
        conversation_id: &str,
        phase: AgentContextWindowPhase,
    ) -> Result<Option<AgentContextWindowSnapshot>, String> {
        if !agent_input.context_window_indicator_enabled {
            return Ok(None);
        }
        let configuration_revision = conversation_context_configuration_revision(agent_input)
            .map_err(|error| error.to_string())?;
        let access = self.next_conversation_context_state_access();
        {
            let mut states = self
                .conversation_context_states
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            if let Some(entry) = states.get_mut(conversation_id) {
                if entry.configuration_revision == configuration_revision {
                    entry.last_access = access;
                    return Ok(Some(entry.state.snapshot(phase)));
                }
                states.remove(conversation_id);
            }
        }
        self.rebuild_conversation_context_state(agent_input, conversation_id, phase, None)
            .map(|update| update.snapshot)
    }

    fn rebuild_conversation_context_state(
        &self,
        agent_input: &AgentChatInput,
        conversation_id: &str,
        phase: AgentContextWindowPhase,
        active_run_id: Option<&str>,
    ) -> Result<ConversationContextStateUpdate, String> {
        let (preview_input, traces) =
            self.persisted_conversation_context_state(agent_input, conversation_id)?;
        let mut state =
            create_conversation_context_state(preview_input).map_err(|error| error.to_string())?;
        let baseline = state.shared_baseline().map_err(|error| error.to_string())?;
        let snapshot = agent_input
            .context_window_indicator_enabled
            .then(|| state.snapshot(phase));
        let latest_trace = traces.last();
        let entry = ConversationContextStateEntry {
            configuration_revision: state.configuration_revision().to_string(),
            state,
            active_run_id: latest_trace
                .filter(|trace| !trace.terminal_status.is_terminal())
                .and(active_run_id)
                .map(ToString::to_string),
            active_assistant_message_id: latest_trace
                .map(|trace| trace.assistant_message_id.clone()),
            committed_trace_items: latest_trace.map_or(0, |trace| trace.items.len()),
            terminal: latest_trace.is_none_or(|trace| trace.terminal_status.is_terminal()),
            last_access: self.next_conversation_context_state_access(),
        };
        self.insert_conversation_context_state(conversation_id, entry);
        Ok(ConversationContextStateUpdate { baseline, snapshot })
    }

    fn finalize_conversation_context_state(
        &self,
        agent_input: &AgentChatInput,
        run_id: &str,
        conversation_id: &str,
        assistant_message_id: &str,
        assistant_content: &str,
    ) -> Result<Option<AgentContextWindowSnapshot>, String> {
        let trace = self
            .storage
            .get_conversation_turn_trace(assistant_message_id)?
            .ok_or_else(|| format!("assistant 终态缺少会话轨迹：{assistant_message_id}"))?;
        let configuration_revision = conversation_context_configuration_revision(agent_input)
            .map_err(|error| error.to_string())?;
        let access = self.next_conversation_context_state_access();
        let mut needs_rebuild = false;
        {
            let mut states = self
                .conversation_context_states
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            if let Some(entry) = states.get_mut(conversation_id) {
                if entry.configuration_revision == configuration_revision
                    && entry.active_run_id.as_deref() == Some(run_id)
                    && entry.active_assistant_message_id.as_deref() == Some(assistant_message_id)
                {
                    if !entry.terminal {
                        match entry.state.finalize_conversation_turn(
                            &trace,
                            entry.committed_trace_items,
                            assistant_content,
                        ) {
                            Ok(committed_trace_items) => {
                                entry.committed_trace_items = committed_trace_items;
                                entry.terminal = true;
                                entry.active_run_id = None;
                            }
                            Err(_) => needs_rebuild = true,
                        }
                    }
                    if !needs_rebuild {
                        entry.last_access = access;
                        entry
                            .state
                            .shared_baseline()
                            .map_err(|error| error.to_string())?;
                        return Ok(agent_input.context_window_indicator_enabled.then(|| {
                            entry.state.snapshot(AgentContextWindowPhase::DurableCommit)
                        }));
                    }
                } else {
                    needs_rebuild = true;
                }
            } else {
                needs_rebuild = true;
            }
            if needs_rebuild {
                states.remove(conversation_id);
            }
        }
        self.rebuild_conversation_context_state(
            agent_input,
            conversation_id,
            AgentContextWindowPhase::DurableCommit,
            Some(run_id),
        )
        .map(|update| update.snapshot)
    }

    fn emit_terminal_context_window_snapshot(
        &self,
        notifications: &CoreServerNotificationSender,
        agent_input: &AgentChatInput,
        run_id: &str,
        conversation_id: &str,
        assistant_message_id: &str,
        assistant_content: &str,
    ) {
        let snapshot = match self.finalize_conversation_context_state(
            agent_input,
            run_id,
            conversation_id,
            assistant_message_id,
            assistant_content,
        ) {
            Ok(snapshot) => snapshot,
            Err(error) => {
                eprintln!(
                    "failed to refresh context window snapshot for conversation {conversation_id}: {error}"
                );
                return;
            }
        };
        self.emit_context_window_snapshot(notifications, run_id, conversation_id, snapshot);
    }

    fn emit_context_window_snapshot(
        &self,
        notifications: &CoreServerNotificationSender,
        run_id: &str,
        conversation_id: &str,
        snapshot: Option<AgentContextWindowSnapshot>,
    ) {
        let Some(snapshot) = snapshot else {
            return;
        };
        let _ = notifications.send(agent_event_notification(AgentEvent::ContextWindowUpdated {
            run_id: run_id.to_string(),
            conversation_id: Some(conversation_id.to_string()),
            snapshot,
        }));
    }

    fn next_conversation_context_state_access(&self) -> u64 {
        self.conversation_context_state_clock
            .fetch_add(1, Ordering::Relaxed)
    }

    fn insert_conversation_context_state(
        &self,
        conversation_id: &str,
        entry: ConversationContextStateEntry,
    ) {
        let mut states = self
            .conversation_context_states
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if !states.contains_key(conversation_id)
            && states.len() >= MAX_CONVERSATION_CONTEXT_STATE_CACHE_ENTRIES
        {
            if let Some(oldest) = states
                .iter()
                .min_by_key(|(_, entry)| entry.last_access)
                .map(|(conversation_id, _)| conversation_id.clone())
            {
                states.remove(&oldest);
            }
        }
        states.insert(conversation_id.to_string(), entry);
    }

    /// Compression, message deletion, rollback and any future durable-history rewrite must call
    /// this before the next durable-context read.
    pub fn invalidate_conversation_context_state(&self, conversation_id: &str) {
        self.conversation_context_states
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .remove(conversation_id);
    }

    pub fn invalidate_all_conversation_context_states(&self) {
        self.conversation_context_states
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clear();
    }
}

fn load_persisted_pending_actions(
    storage: &Arc<StorageService>,
) -> HashMap<String, PendingActionRecord> {
    let records = match storage.list_pending_agent_actions() {
        Ok(records) => records,
        Err(error) => {
            eprintln!("failed to load persisted pending actions: {error}");
            return HashMap::new();
        }
    };

    records
        .into_iter()
        .filter_map(|record| {
            let action = serde_json::from_str::<AgentProposedAction>(&record.action_json)
                .map_err(|error| {
                    eprintln!(
                        "failed to parse persisted pending action {}: {error}",
                        record.action_id
                    );
                    error
                })
                .ok()?;
            let agent_input = serde_json::from_str::<AgentChatInput>(&record.agent_input_json)
                .map_err(|error| {
                    eprintln!(
                        "failed to parse persisted pending action input {}: {error}",
                        record.action_id
                    );
                    error
                })
                .ok()?;
            let agent_input = restore_agent_input_secrets(storage, agent_input);
            let status = pending_status_from_label(&record.status)?;
            let snapshot = PendingAgentActionSnapshot {
                action_id: record.action_id.clone(),
                action_type: record.action_type,
                tool_name: record.tool_name,
                tool_call_id: record.tool_call_id,
                run_id: record.run_id,
                conversation_id: record.conversation_id,
                assistant_message_id: record.assistant_message_id,
                action,
                created_at: record.created_at,
                status,
            };
            Some((
                record.action_id,
                PendingActionRecord {
                    snapshot,
                    agent_input,
                },
            ))
        })
        .collect()
}

fn paginate_chars(
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

fn pending_storage_record(
    record: &PendingActionRecord,
    updated_at: i64,
) -> AgentPendingActionRecord {
    let mut persisted_agent_input = record.agent_input.clone();
    persisted_agent_input.api_token.clear();
    AgentPendingActionRecord {
        action_id: record.snapshot.action_id.clone(),
        run_id: record.snapshot.run_id.clone(),
        conversation_id: record.snapshot.conversation_id.clone(),
        assistant_message_id: record.snapshot.assistant_message_id.clone(),
        action_type: record.snapshot.action_type.clone(),
        tool_name: record.snapshot.tool_name.clone(),
        tool_call_id: record.snapshot.tool_call_id.clone(),
        status: pending_status_label(record.snapshot.status).to_string(),
        action_json: serialize_json(&record.snapshot.action),
        agent_input_json: serialize_json(&persisted_agent_input),
        created_at: record.snapshot.created_at,
        updated_at,
    }
}

fn restore_agent_input_secrets(
    storage: &Arc<StorageService>,
    mut agent_input: AgentChatInput,
) -> AgentChatInput {
    if !agent_input.api_token.trim().is_empty() {
        return agent_input;
    }

    let Ok(Some(settings)) = storage.load_model_settings() else {
        return agent_input;
    };
    agent_input.api_url = settings.api_url.trim().to_string();
    agent_input.api_token = settings.api_token.trim().to_string();
    agent_input
}

fn agent_input_with_run_checkpoint(
    agent_input: &AgentChatInput,
    checkpoint: &AgentRunCheckpoint,
) -> AgentChatInput {
    let mut resume_input = agent_input.clone();
    resume_input.messages.clear();
    resume_input.attachments.clear();
    resume_input.approval_decision = None;
    resume_input.tool_continuation = None;
    resume_input.resume_checkpoint = Some(checkpoint.clone());
    resume_input
}

fn pending_status_from_label(value: &str) -> Option<PendingActionStatus> {
    match value {
        "pending" => Some(PendingActionStatus::Pending),
        "approved" => Some(PendingActionStatus::Approved),
        "rejected" => Some(PendingActionStatus::Rejected),
        "cancelled" => Some(PendingActionStatus::Cancelled),
        "completed" => Some(PendingActionStatus::Completed),
        "failed" => Some(PendingActionStatus::Failed),
        _ => None,
    }
}

fn path_scope_for_action(input: &AgentChatInput, action: &AgentProposedAction) -> Option<String> {
    let path = match action {
        AgentProposedAction::Diff { diff } => Some(diff.file_path.as_str()),
        AgentProposedAction::FileWrite { file_write } => Some(file_write.file_path.as_str()),
        AgentProposedAction::ToolCall { call } if call.tool == "apply_patch" => call
            .args
            .get("filePath")
            .and_then(serde_json::Value::as_str),
        _ => None,
    }?;
    Some(scope_for_path(input, path))
}

fn command_cwd_scope_for_action(
    input: &AgentChatInput,
    action: &AgentProposedAction,
) -> Option<String> {
    let cwd = match action {
        AgentProposedAction::Command { command } => command.cwd.as_deref().unwrap_or("."),
        AgentProposedAction::ToolCall { call } if call.tool == "run_command" => call
            .args
            .get("cwd")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("."),
        _ => return None,
    };
    Some(scope_for_path(input, cwd))
}

fn scope_for_path(input: &AgentChatInput, path: &str) -> String {
    let Some(root) = workspace_root_optional(input) else {
        return "no_workspace".to_string();
    };
    let trimmed = path.trim();
    if trimmed.is_empty() || trimmed == "." {
        return "workspace".to_string();
    }
    let path = Path::new(trimmed);
    if path.is_absolute() {
        if path.starts_with(root) {
            "workspace".to_string()
        } else {
            "outside_workspace".to_string()
        }
    } else if trimmed.starts_with('@') {
        "system_alias".to_string()
    } else {
        "workspace".to_string()
    }
}

fn agent_input_project_id(input: &AgentChatInput) -> Option<&str> {
    input
        .context
        .as_ref()
        .and_then(|context| context.project_id.as_deref())
}

#[cfg(test)]
mod tests {
    use super::*;
    use mycopilot_core::storage::models::{
        ChatConversationRecord, ChatMessageRecord, ModelConfigRecord, ModelSettingsRecord,
    };
    use mycopilot_core::{
        AgentCommandRequest, AgentCommandRiskLevel, AgentPermissions, AgentUsageSummaryRange,
        AgentWorkspaceContext, ConversationTraceToolResultStatus, ConversationTurnTraceItem,
        ConversationTurnTraceTerminalStatus, CONVERSATION_TURN_TRACE_SCHEMA_VERSION,
    };
    use serde_json::json;
    use tempfile::tempdir;

    fn completed_trace(conversation_id: &str, assistant_message_id: &str) -> ConversationTurnTrace {
        ConversationTurnTrace {
            schema_version: CONVERSATION_TURN_TRACE_SCHEMA_VERSION,
            run_id: "run-history".to_string(),
            conversation_id: conversation_id.to_string(),
            assistant_message_id: assistant_message_id.to_string(),
            terminal_status: ConversationTurnTraceTerminalStatus::Completed,
            terminal_error: None,
            truncated: false,
            items: vec![
                ConversationTurnTraceItem::AssistantNarration {
                    sequence: 0,
                    content: "I am creating the requested file.".to_string(),
                    truncated: false,
                },
                ConversationTurnTraceItem::ToolCall {
                    sequence: 1,
                    call_id: "write-history".to_string(),
                    tool: "write_file".to_string(),
                    operation: json!({
                        "filePath": "src/history.rs",
                        "mode": "create"
                    }),
                    approval_status: AgentApprovalStatus::Approved,
                    truncated: false,
                },
                ConversationTurnTraceItem::ToolResult {
                    sequence: 2,
                    call_id: "write-history".to_string(),
                    tool: "write_file".to_string(),
                    status: ConversationTraceToolResultStatus::Succeeded,
                    success: true,
                    observation: json!({
                        "filePath": "src/history.rs",
                        "status": "applied",
                        "additions": 3,
                        "deletions": 0
                    }),
                    approval_status: AgentApprovalStatus::Approved,
                    error: None,
                    truncated: false,
                },
            ],
        }
    }

    fn test_model_settings() -> ModelSettingsRecord {
        ModelSettingsRecord {
            api_url: "https://example.test/v1/chat/completions".to_string(),
            api_token: "token".to_string(),
            search_mode: "disabled".to_string(),
            tavily_api_key: String::new(),
            models: vec![ModelConfigRecord {
                id: "model-1".to_string(),
                display_name: "Model 1".to_string(),
                short_name: None,
                provider_path: None,
                supports_image: false,
                context_window_tokens: Some(128_000),
                input_price: "0.01".to_string(),
                output_price: "0.02".to_string(),
                enabled: true,
            }],
        }
    }

    #[test]
    fn next_turn_loads_backend_trace_and_never_parses_agent_run_json() {
        let fixture = tempdir().unwrap();
        let storage = StorageService::open(&fixture.path().join("storage.sqlite")).unwrap();
        storage.save_model_settings(test_model_settings()).unwrap();
        storage
            .save_conversation(ChatConversationRecord {
                id: "conversation-history".to_string(),
                project_id: None,
                model_id: Some("model-1".to_string()),
                title: "Trace history".to_string(),
                messages: vec![
                    ChatMessageRecord {
                        id: "user-history".to_string(),
                        role: "user".to_string(),
                        content: "Create the file".to_string(),
                        created_at: 1,
                        status: Some("sent".to_string()),
                        attachments: Vec::new(),
                        agent_run_json: None,
                        ui_state_json: None,
                    },
                    ChatMessageRecord {
                        id: "assistant-history".to_string(),
                        role: "assistant".to_string(),
                        content: "Created src/history.rs.".to_string(),
                        created_at: 2,
                        status: Some("sent".to_string()),
                        attachments: Vec::new(),
                        agent_run_json: Some(
                            json!({
                                "timeline": [{ "content": "FRONTEND_TIMELINE_MUST_NOT_ENTER_CONTEXT" }],
                                "toolCalls": [{ "args": { "filePath": "frontend/fake.rs" } }]
                            })
                            .to_string(),
                        ),
                        ui_state_json: None,
                    },
                ],
                created_at: 1,
                updated_at: 2,
                pinned_at: None,
                archived_at: None,
                unread_at: None,
            })
            .unwrap();
        let trace = completed_trace("conversation-history", "assistant-history");
        storage
            .replace_conversation_turn_trace(&trace, 1, 2)
            .unwrap();

        let prepared = prepare_conversation_turn(
            &storage,
            AgentConversationTurnInput {
                conversation_id: Some("conversation-history".to_string()),
                project_id: None,
                model_id: "model-1".to_string(),
                context_window_indicator_enabled: true,
                content: "What changed?".to_string(),
                attachments: Vec::new(),
                title: None,
                user_message_id: Some("user-next".to_string()),
                assistant_message_id: Some("assistant-next".to_string()),
                max_tokens: None,
                temperature: None,
                prompt_preferences: None,
                permissions: AgentPermissions::default(),
            },
            "run-next",
        )
        .unwrap();

        assert_eq!(prepared.agent_input.messages.len(), 3);
        assert_eq!(prepared.agent_input.messages[0].content, "Create the file");
        assert_eq!(
            prepared.agent_input.messages[1]
                .conversation_turn_trace
                .as_ref(),
            Some(&trace)
        );
        assert_eq!(
            prepared.agent_input.messages[1].content,
            "Created src/history.rs."
        );
        assert_eq!(prepared.agent_input.messages[2].content, "What changed?");
        let serialized = serde_json::to_string(&prepared.agent_input.messages).unwrap();
        assert!(serialized.contains("src/history.rs"));
        assert!(!serialized.contains("FRONTEND_TIMELINE_MUST_NOT_ENTER_CONTEXT"));
        assert!(!serialized.contains("frontend/fake.rs"));
    }

    #[test]
    fn legacy_messages_without_trace_keep_final_text_and_legacy_errors_stay_excluded() {
        let conversation = ChatConversationRecord {
            id: "conversation-legacy".to_string(),
            project_id: None,
            model_id: None,
            title: "Legacy".to_string(),
            messages: vec![
                ChatMessageRecord {
                    id: "assistant-legacy".to_string(),
                    role: "assistant".to_string(),
                    content: "Legacy final answer".to_string(),
                    created_at: 1,
                    status: Some("sent".to_string()),
                    attachments: Vec::new(),
                    agent_run_json: Some("not valid json".to_string()),
                    ui_state_json: None,
                },
                ChatMessageRecord {
                    id: "assistant-error".to_string(),
                    role: "assistant".to_string(),
                    content: "Legacy error".to_string(),
                    created_at: 2,
                    status: Some("error".to_string()),
                    attachments: Vec::new(),
                    agent_run_json: None,
                    ui_state_json: None,
                },
            ],
            created_at: 1,
            updated_at: 2,
            pinned_at: None,
            archived_at: None,
            unread_at: None,
        };

        let history = conversation_history_messages(&conversation, &[], &[]);

        assert_eq!(history.len(), 1);
        assert_eq!(history[0].content, "Legacy final answer");
        assert!(history[0].conversation_turn_trace.is_none());

        let mut failed_trace = completed_trace("conversation-legacy", "assistant-error");
        failed_trace.terminal_status = ConversationTurnTraceTerminalStatus::Failed;
        failed_trace.terminal_error = Some("permission denied".to_string());
        let history_with_trace =
            conversation_history_messages(&conversation, &[failed_trace.clone()], &[]);
        assert_eq!(history_with_trace.len(), 2);
        assert_eq!(
            history_with_trace[1].conversation_turn_trace.as_ref(),
            Some(&failed_trace)
        );
    }

    #[test]
    fn next_turn_keeps_committed_prefix_from_an_interrupted_pending_run() {
        let mut trace = completed_trace("conversation-interrupted", "assistant-interrupted");
        trace.terminal_status = ConversationTurnTraceTerminalStatus::InProgress;
        let conversation = ChatConversationRecord {
            id: "conversation-interrupted".to_string(),
            project_id: None,
            model_id: None,
            title: "Interrupted".to_string(),
            messages: vec![ChatMessageRecord {
                id: "assistant-interrupted".to_string(),
                role: "assistant".to_string(),
                content: "duplicated pending narration".to_string(),
                created_at: 1,
                status: Some("pending".to_string()),
                attachments: Vec::new(),
                agent_run_json: None,
                ui_state_json: None,
            }],
            created_at: 1,
            updated_at: 1,
            pinned_at: None,
            archived_at: None,
            unread_at: None,
        };

        let history = conversation_history_messages(&conversation, &[trace.clone()], &[]);

        assert_eq!(history.len(), 1);
        assert!(history[0].content.is_empty());
        assert_eq!(history[0].conversation_turn_trace.as_ref(), Some(&trace));
    }

    #[test]
    fn context_window_snapshot_reports_net_durable_budget() {
        let fixture = tempdir().unwrap();
        let storage =
            Arc::new(StorageService::open(&fixture.path().join("storage.sqlite")).unwrap());
        storage
            .save_model_settings(ModelSettingsRecord {
                api_url: "https://example.test/v1/chat/completions".to_string(),
                api_token: "token".to_string(),
                search_mode: "disabled".to_string(),
                tavily_api_key: String::new(),
                models: vec![ModelConfigRecord {
                    id: "model-1".to_string(),
                    display_name: "Model 1".to_string(),
                    short_name: None,
                    provider_path: None,
                    supports_image: false,
                    context_window_tokens: Some(128_000),
                    input_price: "0.01".to_string(),
                    output_price: "0.02".to_string(),
                    enabled: true,
                }],
            })
            .unwrap();
        let service = AgentService::new(storage);
        let enabled = service
            .get_context_window_snapshot(AgentContextWindowSnapshotInput {
                conversation_id: None,
                project_id: None,
                model_id: "model-1".to_string(),
                max_tokens: Some(30_000),
                prompt_preferences: None,
                permissions: AgentPermissions::default(),
            })
            .unwrap()
            .snapshot
            .unwrap();

        assert_eq!(enabled.model, "model-1");
        assert_eq!(enabled.context_window_tokens, Some(128_000));
        assert!(enabled
            .durable_capacity_tokens
            .is_some_and(|value| value > 0));
        assert_eq!(enabled.durable_input_tokens, 0);
    }

    #[test]
    fn running_trace_commits_drive_monotonic_context_window_events() {
        let fixture = tempdir().unwrap();
        let storage =
            Arc::new(StorageService::open(&fixture.path().join("storage.sqlite")).unwrap());
        storage.save_model_settings(test_model_settings()).unwrap();
        storage
            .save_conversation(ChatConversationRecord {
                id: "conversation-live".to_string(),
                project_id: None,
                model_id: Some("model-1".to_string()),
                title: "Live trace".to_string(),
                messages: vec![
                    ChatMessageRecord {
                        id: "user-live".to_string(),
                        role: "user".to_string(),
                        content: "Inspect the project".to_string(),
                        created_at: 1,
                        status: Some("sent".to_string()),
                        attachments: Vec::new(),
                        agent_run_json: None,
                        ui_state_json: None,
                    },
                    ChatMessageRecord {
                        id: "assistant-live".to_string(),
                        role: "assistant".to_string(),
                        content: THINKING_PLACEHOLDER.to_string(),
                        created_at: 2,
                        status: Some("pending".to_string()),
                        attachments: Vec::new(),
                        agent_run_json: None,
                        ui_state_json: None,
                    },
                ],
                created_at: 1,
                updated_at: 2,
                pinned_at: None,
                archived_at: None,
                unread_at: None,
            })
            .unwrap();
        let service = AgentService::new(storage.clone());
        let agent_input = serde_json::from_value::<AgentChatInput>(json!({
            "apiUrl": "https://example.test/v1/chat/completions",
            "apiToken": "secret",
            "model": "model-1",
            "contextWindowTokens": 128000,
            "contextWindowIndicatorEnabled": true,
            "maxTokens": 30000,
            "context": {
                "conversationId": "conversation-live"
            },
            "messages": []
        }))
        .unwrap();
        let (notifications, mut receiver) = tokio::sync::mpsc::unbounded_channel();
        let observer = service.trace_observer(
            "run-live",
            "conversation-live",
            "assistant-live",
            2,
            agent_input,
            notifications,
        );

        observer(ConversationTraceSnapshot::default()).unwrap();
        let initial = receiver.try_recv().unwrap();
        let initial_tokens = initial["params"]["snapshot"]["durableInputTokens"]
            .as_u64()
            .unwrap();

        let narration = ConversationTurnTraceItem::AssistantNarration {
            sequence: 0,
            content: "I will inspect the relevant files.".to_string(),
            truncated: false,
        };
        observer(ConversationTraceSnapshot {
            items: vec![narration.clone()],
            next_sequence: 1,
            truncated: false,
        })
        .unwrap();
        let narrated = receiver.try_recv().unwrap();
        let narrated_tokens = narrated["params"]["snapshot"]["durableInputTokens"]
            .as_u64()
            .unwrap();
        assert!(narrated_tokens > initial_tokens);
        {
            let states = service
                .conversation_context_states
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            let entry = states.get("conversation-live").unwrap();
            assert_eq!(entry.committed_trace_items, 1);
            assert!(!entry.terminal);
        }

        let call = ConversationTurnTraceItem::ToolCall {
            sequence: 1,
            call_id: "call-live".to_string(),
            tool: "read_file".to_string(),
            operation: json!({ "path": "src/lib.rs" }),
            approval_status: AgentApprovalStatus::NotRequired,
            truncated: false,
        };
        observer(ConversationTraceSnapshot {
            items: vec![narration.clone(), call.clone()],
            next_sequence: 2,
            truncated: false,
        })
        .unwrap();
        assert!(receiver.try_recv().is_err());
        assert_eq!(
            service
                .conversation_context_states
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .get("conversation-live")
                .unwrap()
                .committed_trace_items,
            1
        );

        let result = ConversationTurnTraceItem::ToolResult {
            sequence: 2,
            call_id: "call-live".to_string(),
            tool: "read_file".to_string(),
            status: ConversationTraceToolResultStatus::Succeeded,
            success: true,
            observation: json!({ "path": "src/lib.rs", "endLine": 20 }),
            approval_status: AgentApprovalStatus::NotRequired,
            error: None,
            truncated: false,
        };
        observer(ConversationTraceSnapshot {
            items: vec![narration, call, result],
            next_sequence: 3,
            truncated: false,
        })
        .unwrap();
        let closed = receiver.try_recv().unwrap();
        let closed_tokens = closed["params"]["snapshot"]["durableInputTokens"]
            .as_u64()
            .unwrap();
        assert!(closed_tokens > narrated_tokens);

        let trace = storage
            .get_conversation_turn_trace("assistant-live")
            .unwrap()
            .unwrap();
        assert_eq!(
            trace.terminal_status,
            ConversationTurnTraceTerminalStatus::InProgress
        );
        assert_eq!(trace.items.len(), 3);
        assert_eq!(
            service
                .conversation_context_states
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .get("conversation-live")
                .unwrap()
                .committed_trace_items,
            3
        );
    }

    #[test]
    fn disabled_indicator_still_builds_runtime_context_baseline() {
        let fixture = tempdir().unwrap();
        let storage =
            Arc::new(StorageService::open(&fixture.path().join("storage.sqlite")).unwrap());
        storage.save_model_settings(test_model_settings()).unwrap();
        storage
            .save_conversation(ChatConversationRecord {
                id: "conversation-hidden-indicator".to_string(),
                project_id: None,
                model_id: Some("model-1".to_string()),
                title: "Hidden indicator".to_string(),
                messages: vec![
                    ChatMessageRecord {
                        id: "user-hidden-indicator".to_string(),
                        role: "user".to_string(),
                        content: "Inspect the durable context".to_string(),
                        created_at: 1,
                        status: Some("sent".to_string()),
                        attachments: Vec::new(),
                        agent_run_json: None,
                        ui_state_json: None,
                    },
                    ChatMessageRecord {
                        id: "assistant-hidden-indicator".to_string(),
                        role: "assistant".to_string(),
                        content: THINKING_PLACEHOLDER.to_string(),
                        created_at: 2,
                        status: Some("pending".to_string()),
                        attachments: Vec::new(),
                        agent_run_json: None,
                        ui_state_json: None,
                    },
                ],
                created_at: 1,
                updated_at: 2,
                pinned_at: None,
                archived_at: None,
                unread_at: None,
            })
            .unwrap();
        let service = AgentService::new(storage);
        let agent_input = serde_json::from_value::<AgentChatInput>(json!({
            "apiUrl": "https://example.test/v1/chat/completions",
            "apiToken": "secret",
            "model": "model-1",
            "contextWindowTokens": 128000,
            "contextWindowIndicatorEnabled": false,
            "maxTokens": 30000,
            "context": { "conversationId": "conversation-hidden-indicator" },
            "messages": []
        }))
        .unwrap();
        let (notifications, mut receiver) = tokio::sync::mpsc::unbounded_channel();
        let observer = service.trace_observer(
            "run-hidden-indicator",
            "conversation-hidden-indicator",
            "assistant-hidden-indicator",
            2,
            agent_input.clone(),
            notifications,
        );

        let baseline = observer(ConversationTraceSnapshot::default()).unwrap();

        assert!(baseline.is_some());
        assert!(receiver.try_recv().is_err());
        assert!(service
            .conversation_context_states
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .contains_key("conversation-hidden-indicator"));
        assert!(service
            .context_window_snapshot_with_cache(
                &agent_input,
                "conversation-hidden-indicator",
                AgentContextWindowPhase::Idle,
            )
            .unwrap()
            .is_none());
        assert!(service
            .conversation_context_states
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .contains_key("conversation-hidden-indicator"));
    }

    #[test]
    fn deleting_messages_invalidates_the_conversation_context_state() {
        let fixture = tempdir().unwrap();
        let storage =
            Arc::new(StorageService::open(&fixture.path().join("storage.sqlite")).unwrap());
        storage.save_model_settings(test_model_settings()).unwrap();
        storage
            .save_conversation(ChatConversationRecord {
                id: "conversation-delete-context".to_string(),
                project_id: None,
                model_id: Some("model-1".to_string()),
                title: "Delete context".to_string(),
                messages: vec![ChatMessageRecord {
                    id: "user-delete-context".to_string(),
                    role: "user".to_string(),
                    content: "Old durable content".to_string(),
                    created_at: 1,
                    status: Some("sent".to_string()),
                    attachments: Vec::new(),
                    agent_run_json: None,
                    ui_state_json: None,
                }],
                created_at: 1,
                updated_at: 1,
                pinned_at: None,
                archived_at: None,
                unread_at: None,
            })
            .unwrap();
        let service = AgentService::new(storage);
        let input = serde_json::from_value::<AgentChatInput>(json!({
            "apiUrl": "https://example.test/v1/chat/completions",
            "apiToken": "secret",
            "model": "model-1",
            "contextWindowTokens": 128000,
            "contextWindowIndicatorEnabled": true,
            "maxTokens": 30000,
            "context": { "conversationId": "conversation-delete-context" },
            "messages": []
        }))
        .unwrap();

        assert!(service
            .context_window_snapshot_with_cache(
                &input,
                "conversation-delete-context",
                AgentContextWindowPhase::Idle,
            )
            .unwrap()
            .is_some());
        assert!(service
            .conversation_context_states
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .contains_key("conversation-delete-context"));

        service
            .delete_chat_messages(
                "conversation-delete-context",
                &["user-delete-context".to_string()],
            )
            .unwrap();

        assert!(!service
            .conversation_context_states
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .contains_key("conversation-delete-context"));
    }

    #[test]
    fn persists_usage_for_failed_runs() {
        let fixture = tempdir().unwrap();
        let storage =
            Arc::new(StorageService::open(&fixture.path().join("storage.sqlite")).unwrap());
        storage
            .save_conversation(ChatConversationRecord {
                id: "conversation-1".to_string(),
                project_id: None,
                model_id: Some("model-1".to_string()),
                title: "Usage test".to_string(),
                messages: Vec::new(),
                created_at: 1,
                updated_at: 1,
                pinned_at: None,
                archived_at: None,
                unread_at: None,
            })
            .unwrap();
        let service = AgentService::new(storage);
        service.register_usage_context(
            "run-1",
            AgentRunUsageContext {
                conversation_id: "conversation-1".to_string(),
                assistant_message_id: "assistant-1".to_string(),
                run_id: "run-1".to_string(),
                project_id: None,
                model_id: "model-1".to_string(),
                model_name: "Model 1".to_string(),
                provider_path: None,
                input_price: None,
                output_price: None,
                started_at: 1,
            },
        );

        service
            .persist_run_usage(
                "run-1",
                AgentRunStatus::Failed,
                Some(AgentUsage {
                    input_tokens: Some(20),
                    output_tokens: Some(8),
                    output_thinking_tokens: None,
                    total_tokens: Some(28),
                    cached_input_tokens: None,
                    cache_creation_input_tokens: None,
                    billable_request_count: Some(3),
                }),
                Some("invalid tool arguments".to_string()),
            )
            .unwrap();

        let summary = service
            .get_usage_summary(&AgentUsageSummaryInput {
                range: AgentUsageSummaryRange::All,
                from: None,
                to: None,
            })
            .unwrap();
        assert_eq!(summary.request_count, 3);
        assert_eq!(summary.input_tokens, Some(20));
        assert_eq!(summary.output_tokens, Some(8));
        assert_eq!(summary.total_tokens, Some(28));
    }

    #[test]
    fn deleting_project_cancels_runs_and_discards_usage_contexts() {
        let fixture = tempdir().unwrap();
        let storage =
            Arc::new(StorageService::open(&fixture.path().join("storage.sqlite")).unwrap());
        let service = AgentService::new(storage);
        let cancellation = AgentCancellationToken::new();
        service.register_cancellation("run-1", cancellation.clone());
        service.register_usage_context(
            "run-1",
            AgentRunUsageContext {
                conversation_id: "conversation-1".to_string(),
                assistant_message_id: "assistant-1".to_string(),
                run_id: "run-1".to_string(),
                project_id: Some("project-1".to_string()),
                model_id: "model-1".to_string(),
                model_name: "Model 1".to_string(),
                provider_path: None,
                input_price: None,
                output_price: None,
                started_at: 1,
            },
        );

        service.delete_project("project-1").unwrap();

        assert!(cancellation.is_cancelled());
        assert!(service.is_project_deleting(Some("project-1")));
        assert!(service
            .usage_contexts
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .is_empty());
    }

    #[test]
    fn pending_approval_persists_full_run_checkpoint() {
        let fixture = tempdir().unwrap();
        let storage =
            Arc::new(StorageService::open(&fixture.path().join("storage.sqlite")).unwrap());
        let service = AgentService::new(storage.clone());
        let base_input = serde_json::from_value::<AgentChatInput>(json!({
            "apiUrl": "https://example.test/v1/chat/completions",
            "apiToken": "secret",
            "model": "test-model",
            "contextWindowTokens": 128000,
            "messages": []
        }))
        .unwrap();
        let run_checkpoint = AgentRunCheckpoint {
            version: 1,
            run_id: "run-checkpoint".to_string(),
            context_items: vec![
                mycopilot_core::AgentContextCheckpointItem {
                    role: "system".to_string(),
                    content: "rules".to_string(),
                    images: Vec::new(),
                    tool_call_id: None,
                    tool_calls: Vec::new(),
                    is_error: false,
                    sources: vec!["backend_system_prompt".to_string()],
                    scope: "run".to_string(),
                    retention: "retained".to_string(),
                    group: None,
                },
                mycopilot_core::AgentContextCheckpointItem {
                    role: "assistant".to_string(),
                    content: String::new(),
                    images: Vec::new(),
                    tool_call_id: None,
                    tool_calls: vec![mycopilot_core::AgentContextCheckpointToolCall {
                        id: "call-checkpoint".to_string(),
                        name: "apply_patch".to_string(),
                        args: json!({ "operation": "create", "filePath": "report.txt" }),
                    }],
                    is_error: false,
                    sources: vec!["model_response".to_string()],
                    scope: "run".to_string(),
                    retention: "retained".to_string(),
                    group: Some(mycopilot_core::AgentContextCheckpointGroup {
                        id: "exchange-checkpoint".to_string(),
                        kind: "tool_exchange".to_string(),
                    }),
                },
            ],
            next_model_request_index: 1,
            queued_tool_calls: Vec::new(),
            suppressed_narration: false,
            extension_snapshots: Vec::new(),
            pending_tool_call_id: "call-checkpoint".to_string(),
            conversation_trace_items: Vec::new(),
            next_conversation_trace_sequence: 0,
            conversation_trace_truncated: false,
        };
        let checkpoint = agent_input_with_run_checkpoint(&base_input, &run_checkpoint);
        let action = AgentProposedAction::ToolCall {
            call: AgentToolCall {
                id: "call-checkpoint".to_string(),
                tool: "apply_patch".to_string(),
                args: json!({ "operation": "create", "filePath": "report.txt" }),
                approval_status: AgentApprovalStatus::Required,
                reason: None,
            },
        };

        service.store_pending_action(
            "run-checkpoint",
            "conversation-checkpoint",
            "assistant-checkpoint",
            action,
            checkpoint,
        );
        assert!(base_input.resume_checkpoint.is_none());

        let reloaded = AgentService::new(storage);
        let pending = reloaded
            .pending_actions
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let record = pending.get("call-checkpoint").unwrap();
        assert_eq!(
            record.agent_input.resume_checkpoint.as_ref(),
            Some(&run_checkpoint)
        );
        assert!(record.agent_input.messages.is_empty());
        assert!(record.agent_input.attachments.is_empty());
        assert!(record.agent_input.api_token.is_empty());
        assert_eq!(record.agent_input.context_window_tokens, Some(128_000));
    }

    #[test]
    fn cancelling_pending_approval_commits_one_paired_cancelled_trace() {
        let fixture = tempdir().unwrap();
        let storage =
            Arc::new(StorageService::open(&fixture.path().join("storage.sqlite")).unwrap());
        storage
            .save_conversation(ChatConversationRecord {
                id: "conversation-cancel".to_string(),
                project_id: None,
                model_id: Some("model-1".to_string()),
                title: "Cancel trace".to_string(),
                messages: vec![ChatMessageRecord {
                    id: "assistant-cancel".to_string(),
                    role: "assistant".to_string(),
                    content: THINKING_PLACEHOLDER.to_string(),
                    created_at: 1,
                    status: Some("pending".to_string()),
                    attachments: Vec::new(),
                    agent_run_json: None,
                    ui_state_json: None,
                }],
                created_at: 1,
                updated_at: 1,
                pinned_at: None,
                archived_at: None,
                unread_at: None,
            })
            .unwrap();
        let service = AgentService::new(storage.clone());
        service.register_usage_context(
            "run-cancel",
            AgentRunUsageContext {
                conversation_id: "conversation-cancel".to_string(),
                assistant_message_id: "assistant-cancel".to_string(),
                run_id: "run-cancel".to_string(),
                project_id: None,
                model_id: "model-1".to_string(),
                model_name: "Model 1".to_string(),
                provider_path: None,
                input_price: None,
                output_price: None,
                started_at: 1,
            },
        );
        let call = AgentToolCall {
            id: "call-cancel".to_string(),
            tool: "approval_tool".to_string(),
            args: json!({ "path": "safe.txt" }),
            approval_status: AgentApprovalStatus::Required,
            reason: None,
        };
        let mut agent_input = serde_json::from_value::<AgentChatInput>(json!({
            "apiUrl": "https://example.test/v1/chat/completions",
            "apiToken": "secret",
            "model": "test-model",
            "messages": []
        }))
        .unwrap();
        agent_input.resume_checkpoint = Some(AgentRunCheckpoint {
            version: 1,
            run_id: "run-cancel".to_string(),
            context_items: Vec::new(),
            next_model_request_index: 1,
            queued_tool_calls: Vec::new(),
            suppressed_narration: false,
            extension_snapshots: Vec::new(),
            pending_tool_call_id: call.id.clone(),
            conversation_trace_items: vec![ConversationTurnTraceItem::ToolCall {
                sequence: 0,
                call_id: call.id.clone(),
                tool: call.tool.clone(),
                operation: json!({ "path": "safe.txt" }),
                approval_status: AgentApprovalStatus::Required,
                truncated: false,
            }],
            next_conversation_trace_sequence: 1,
            conversation_trace_truncated: false,
        });
        service.store_pending_action(
            "run-cancel",
            "conversation-cancel",
            "assistant-cancel",
            AgentProposedAction::ToolCall { call: call.clone() },
            agent_input,
        );

        assert!(service.cancel_action(&call.id).unwrap());

        let trace = storage
            .get_conversation_turn_trace("assistant-cancel")
            .unwrap()
            .unwrap();
        trace.validate().unwrap();
        assert_eq!(
            trace.terminal_status,
            ConversationTurnTraceTerminalStatus::Cancelled
        );
        assert_eq!(trace.items.len(), 2);
        assert!(matches!(
            &trace.items[1],
            ConversationTurnTraceItem::ToolResult {
                call_id,
                approval_status: AgentApprovalStatus::Rejected,
                ..
            } if call_id == "call-cancel"
        ));
        let conversation = storage
            .load_conversations()
            .unwrap()
            .into_iter()
            .find(|conversation| conversation.id == "conversation-cancel")
            .unwrap();
        assert_eq!(conversation.messages[0].content, "");
        assert_eq!(conversation.messages[0].status.as_deref(), Some("sent"));
    }

    #[test]
    fn forced_cancellation_uses_backend_runtime_snapshot_instead_of_empty_trace() {
        let fixture = tempdir().unwrap();
        let storage =
            Arc::new(StorageService::open(&fixture.path().join("storage.sqlite")).unwrap());
        storage
            .save_conversation(ChatConversationRecord {
                id: "conversation-forced".to_string(),
                project_id: None,
                model_id: Some("model-1".to_string()),
                title: "Forced cancellation".to_string(),
                messages: vec![ChatMessageRecord {
                    id: "assistant-forced".to_string(),
                    role: "assistant".to_string(),
                    content: THINKING_PLACEHOLDER.to_string(),
                    created_at: 1,
                    status: Some("pending".to_string()),
                    attachments: Vec::new(),
                    agent_run_json: None,
                    ui_state_json: None,
                }],
                created_at: 1,
                updated_at: 1,
                pinned_at: None,
                archived_at: None,
                unread_at: None,
            })
            .unwrap();
        let service = AgentService::new(storage.clone());
        service.register_usage_context(
            "run-forced",
            AgentRunUsageContext {
                conversation_id: "conversation-forced".to_string(),
                assistant_message_id: "assistant-forced".to_string(),
                run_id: "run-forced".to_string(),
                project_id: None,
                model_id: "model-1".to_string(),
                model_name: "Model 1".to_string(),
                provider_path: None,
                input_price: None,
                output_price: None,
                started_at: 1,
            },
        );
        let checkpoint = AgentRunCheckpoint {
            version: 1,
            run_id: "run-forced".to_string(),
            context_items: Vec::new(),
            next_model_request_index: 1,
            queued_tool_calls: Vec::new(),
            suppressed_narration: false,
            extension_snapshots: Vec::new(),
            pending_tool_call_id: "pending-command".to_string(),
            conversation_trace_items: vec![
                ConversationTurnTraceItem::ToolCall {
                    sequence: 0,
                    call_id: "write-forced".to_string(),
                    tool: "write_file".to_string(),
                    operation: json!({ "filePath": "created.txt", "mode": "create" }),
                    approval_status: AgentApprovalStatus::Approved,
                    truncated: false,
                },
                ConversationTurnTraceItem::ToolResult {
                    sequence: 1,
                    call_id: "write-forced".to_string(),
                    tool: "write_file".to_string(),
                    status: ConversationTraceToolResultStatus::Succeeded,
                    success: true,
                    observation: json!({
                        "filePath": "created.txt",
                        "status": "applied",
                        "additions": 3,
                        "deletions": 0
                    }),
                    approval_status: AgentApprovalStatus::Approved,
                    error: None,
                    truncated: false,
                },
            ],
            next_conversation_trace_sequence: 2,
            conversation_trace_truncated: false,
        };
        service.seed_trace_snapshot_from_checkpoint("run-forced", Some(&checkpoint));

        service.persist_forced_cancelled_runs(&["run-forced".to_string()]);

        let trace = storage
            .get_conversation_turn_trace("assistant-forced")
            .unwrap()
            .unwrap();
        trace.validate().unwrap();
        assert_eq!(
            trace.terminal_status,
            ConversationTurnTraceTerminalStatus::Cancelled
        );
        assert_eq!(trace.items.len(), 2);
        assert!(serde_json::to_string(&trace)
            .unwrap()
            .contains("created.txt"));
    }

    #[test]
    fn terminal_message_and_trace_roll_back_together_when_trace_is_invalid() {
        let fixture = tempdir().unwrap();
        let storage =
            Arc::new(StorageService::open(&fixture.path().join("storage.sqlite")).unwrap());
        storage
            .save_conversation(ChatConversationRecord {
                id: "conversation-atomic".to_string(),
                project_id: None,
                model_id: None,
                title: "Atomic trace".to_string(),
                messages: vec![ChatMessageRecord {
                    id: "assistant-atomic".to_string(),
                    role: "assistant".to_string(),
                    content: THINKING_PLACEHOLDER.to_string(),
                    created_at: 1,
                    status: Some("pending".to_string()),
                    attachments: Vec::new(),
                    agent_run_json: None,
                    ui_state_json: None,
                }],
                created_at: 1,
                updated_at: 1,
                pinned_at: None,
                archived_at: None,
                unread_at: None,
            })
            .unwrap();
        let invalid_trace = ConversationTurnTrace {
            schema_version: CONVERSATION_TURN_TRACE_SCHEMA_VERSION,
            run_id: "run-atomic".to_string(),
            conversation_id: "conversation-atomic".to_string(),
            assistant_message_id: "assistant-atomic".to_string(),
            terminal_status: ConversationTurnTraceTerminalStatus::Completed,
            terminal_error: None,
            truncated: false,
            items: vec![ConversationTurnTraceItem::ToolCall {
                sequence: 0,
                call_id: "unresolved".to_string(),
                tool: "read_file".to_string(),
                operation: json!({ "path": "file.txt" }),
                approval_status: AgentApprovalStatus::NotRequired,
                truncated: false,
            }],
        };

        assert!(storage
            .finalize_chat_message_with_conversation_trace(
                "conversation-atomic",
                "assistant-atomic",
                "final answer",
                Some("sent"),
                "completed",
                &invalid_trace,
                1,
                2,
            )
            .is_err());

        let conversation = storage.load_conversations().unwrap().remove(0);
        assert_eq!(conversation.messages[0].content, THINKING_PLACEHOLDER);
        assert_eq!(conversation.messages[0].status.as_deref(), Some("pending"));
        assert!(storage
            .get_conversation_turn_trace("assistant-atomic")
            .unwrap()
            .is_none());
    }

    #[tokio::test]
    async fn cancelling_run_during_approved_command_finishes_cancelled_without_resuming_model() {
        let fixture = tempdir().unwrap();
        let storage =
            Arc::new(StorageService::open(&fixture.path().join("storage.sqlite")).unwrap());
        storage
            .save_conversation(ChatConversationRecord {
                id: "conversation-command-cancel".to_string(),
                project_id: None,
                model_id: Some("model-1".to_string()),
                title: "Command cancellation".to_string(),
                messages: vec![ChatMessageRecord {
                    id: "assistant-command-cancel".to_string(),
                    role: "assistant".to_string(),
                    content: THINKING_PLACEHOLDER.to_string(),
                    created_at: 1,
                    status: Some("pending".to_string()),
                    attachments: Vec::new(),
                    agent_run_json: None,
                    ui_state_json: None,
                }],
                created_at: 1,
                updated_at: 1,
                pinned_at: None,
                archived_at: None,
                unread_at: None,
            })
            .unwrap();
        let service = AgentService::new(storage.clone());
        service.register_usage_context(
            "run-command-cancel",
            AgentRunUsageContext {
                conversation_id: "conversation-command-cancel".to_string(),
                assistant_message_id: "assistant-command-cancel".to_string(),
                run_id: "run-command-cancel".to_string(),
                project_id: None,
                model_id: "model-1".to_string(),
                model_name: "Model 1".to_string(),
                provider_path: None,
                input_price: None,
                output_price: None,
                started_at: 1,
            },
        );
        let command = AgentCommandRequest {
            id: "command-cancel".to_string(),
            command: "sleep 5".to_string(),
            cwd: None,
            timeout_ms: Some(10_000),
            approval_status: AgentApprovalStatus::Approved,
            risk_level: Some(AgentCommandRiskLevel::ReadOnly),
            reason: Some("exercise cancellation".to_string()),
        };
        let call = command_tool_call(&command);
        let checkpoint = AgentRunCheckpoint {
            version: 1,
            run_id: "run-command-cancel".to_string(),
            context_items: Vec::new(),
            next_model_request_index: 1,
            queued_tool_calls: Vec::new(),
            suppressed_narration: false,
            extension_snapshots: Vec::new(),
            pending_tool_call_id: call.id.clone(),
            conversation_trace_items: vec![ConversationTurnTraceItem::ToolCall {
                sequence: 0,
                call_id: call.id.clone(),
                tool: call.tool.clone(),
                operation: call.args.clone(),
                approval_status: AgentApprovalStatus::Required,
                truncated: false,
            }],
            next_conversation_trace_sequence: 1,
            conversation_trace_truncated: false,
        };
        let mut agent_input = serde_json::from_value::<AgentChatInput>(json!({
            "apiUrl": "https://should-not-be-called.test/v1/chat/completions",
            "apiToken": "secret",
            "model": "test-model",
            "messages": []
        }))
        .unwrap();
        agent_input.context = Some(AgentRunContext {
            conversation_id: Some("conversation-command-cancel".to_string()),
            project_id: None,
            workspace: Some(AgentWorkspaceContext {
                project_id: None,
                display_name: Some("test".to_string()),
                root_path: Some(fixture.path().to_string_lossy().into_owned()),
            }),
            attachment_library: None,
            permissions: AgentPermissions::default(),
        });
        agent_input.resume_checkpoint = Some(checkpoint);
        let record = PendingActionRecord {
            snapshot: PendingAgentActionSnapshot {
                action_id: command.id.clone(),
                action_type: "command".to_string(),
                tool_name: "run_command".to_string(),
                tool_call_id: Some(command.id.clone()),
                run_id: "run-command-cancel".to_string(),
                conversation_id: Some("conversation-command-cancel".to_string()),
                assistant_message_id: Some("assistant-command-cancel".to_string()),
                action: AgentProposedAction::Command {
                    command: command.clone(),
                },
                created_at: 1,
                status: PendingActionStatus::Approved,
            },
            agent_input,
        };
        let (notifications, _receiver) = tokio::sync::mpsc::unbounded_channel();
        let execution_service = service.clone();
        let task = tokio::spawn(async move {
            execution_service
                .run_command_execution(record, call, notifications)
                .await;
        });

        let mut cancelled = false;
        for _ in 0..100 {
            if service.cancel_run("run-command-cancel") {
                cancelled = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        assert!(cancelled);
        task.await.unwrap();

        let trace = storage
            .get_conversation_turn_trace("assistant-command-cancel")
            .unwrap()
            .unwrap();
        trace.validate().unwrap();
        assert_eq!(
            trace.terminal_status,
            ConversationTurnTraceTerminalStatus::Cancelled
        );
        assert!(matches!(
            trace.items.last(),
            Some(ConversationTurnTraceItem::ToolResult {
                status: ConversationTraceToolResultStatus::Cancelled,
                ..
            })
        ));
    }
}
