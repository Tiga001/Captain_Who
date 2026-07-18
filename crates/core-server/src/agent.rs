use crate::agent_support::*;
pub use crate::agent_support::{
    AgentActionExecutionOutput, AgentContextCompactionAuditInput,
    AgentContextCompactionAuditOutput, AgentContextWindowSnapshotInput,
    AgentContextWindowSnapshotOutput, AgentConversationTurnInput, AgentConversationTurnOutput,
    AgentFileDraftContentPage, AgentFileWriteDiffPage, AgentServiceError, PendingActionStatus,
    PendingAgentActionSnapshot,
};
use crate::skills_adapter::{
    activate_workspace, missing_workspace_failure, PreparedSkillActivation,
};
use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::path::Path;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use mycopilot_core::command::{run_approved_command, AgentCommandExecutionResult, CommandRunState};
use mycopilot_core::file_write::{file_draft_snapshot, file_write_diff};
use mycopilot_core::skills::SkillsService;
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
    inspect_context_window, next_run_id, send_chat_with_host_services, AgentApprovalDecision,
    AgentApprovalDecisionStatus, AgentApprovalStatus, AgentCancellationToken, AgentChatInput,
    AgentChatOutput, AgentContextBaseline, AgentContextCompactionCommitOutcome,
    AgentContextCompactionCommitRequest, AgentContextCompactionGenerationOutput,
    AgentContextCompactionGenerationRequest, AgentContextCompactionModelGenerator,
    AgentContextCompactionPrepareOutcome, AgentContextCompactionServices, AgentContextWindowPhase,
    AgentContextWindowSnapshot, AgentConversationContextState, AgentConversationTraceObserver,
    AgentError, AgentEvent, AgentEventEmitter, AgentHostActionExecutor, AgentModelRequestObserver,
    AgentPatchResult, AgentProposedAction, AgentResult, AgentRunCheckpoint, AgentRunContext,
    AgentRunStatus, AgentRuntimeHostServices, AgentSearchConfig, AgentToolCall,
    AgentToolContinuation, AgentToolResult, AgentUsage, AgentUsageClearInput,
    AgentUsageClearOutput, AgentUsageSummaryInput, AgentUsageSummaryOutput, ContextJournalCursor,
    ConversationTraceSnapshot, ConversationTurnTrace, ConversationTurnTraceTerminalStatus,
};
use serde_json::Value;
use tokio::sync::mpsc::UnboundedSender;

pub(crate) const AGENT_EVENT_NAME: &str = "agent.event";
pub(crate) const THINKING_PLACEHOLDER: &str = "正在思考...";
pub(crate) static ID_COUNTER: AtomicU64 = AtomicU64::new(1);
const MAX_CONVERSATION_CONTEXT_STATE_CACHE_ENTRIES: usize = 32;

type ContextCompactionGenerationFuture = Pin<
    Box<dyn Future<Output = AgentResult<AgentContextCompactionGenerationOutput>> + Send + 'static>,
>;
type ContextCompactionSummaryGenerator = Arc<
    dyn Fn(
            AgentContextCompactionGenerationRequest,
            AgentCancellationToken,
        ) -> ContextCompactionGenerationFuture
        + Send
        + Sync,
>;

pub type CoreServerNotificationSender = UnboundedSender<Value>;

#[derive(Default)]
struct AgentTerminalEventGate {
    deferred: Mutex<Vec<AgentEvent>>,
}

impl AgentTerminalEventGate {
    fn route(&self, event: AgentEvent) -> Option<AgentEvent> {
        if should_defer_until_terminal_commit(&event) {
            self.deferred
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .push(event);
            None
        } else {
            Some(event)
        }
    }

    fn take_after_persistence(&self, output: &AgentChatOutput) -> Vec<AgentEvent> {
        let mut events = std::mem::take(
            &mut *self
                .deferred
                .lock()
                .unwrap_or_else(|error| error.into_inner()),
        );
        if !events
            .iter()
            .any(|event| matches!(event, AgentEvent::Done { .. }))
        {
            events.push(terminal_done_event(output));
        }
        events
    }

    fn discard(&self) {
        self.deferred
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clear();
    }
}

fn should_defer_until_terminal_commit(event: &AgentEvent) -> bool {
    match event {
        AgentEvent::State { state, .. } => is_terminal_run_status(state.status),
        AgentEvent::Done { status, .. } => {
            !matches!(status, Some(AgentRunStatus::WaitingForApproval))
        }
        _ => false,
    }
}

fn terminal_done_event(output: &AgentChatOutput) -> AgentEvent {
    AgentEvent::Done {
        run_id: output.run_id.clone(),
        success: output.status == AgentRunStatus::Completed,
        status: Some(output.status),
        content: (!output.content.is_empty()).then(|| output.content.clone()),
        usage: output.usage.clone(),
        finish_reason: output.finish_reason.clone(),
        proposed_actions: output.proposed_actions.clone(),
    }
}

fn emit_terminal_events_after_persistence(
    notifications: &CoreServerNotificationSender,
    gate: &AgentTerminalEventGate,
    output: &AgentChatOutput,
) {
    for event in gate.take_after_persistence(output) {
        let _ = notifications.send(agent_event_notification(event));
    }
}

fn emit_pending_transition_error(
    notifications: &CoreServerNotificationSender,
    run_id: &str,
    status: PendingActionStatus,
    error: &str,
) {
    let _ = notifications.send(agent_event_notification(AgentEvent::Error {
        run_id: Some(run_id.to_string()),
        message: format!(
            "待审批操作无法可靠迁移为 `{}`；已抑制终态事件：{error}",
            pending_status_label(status)
        ),
        recoverable: true,
        code: Some("pending_action_transition_failed".to_string()),
        details: None,
    }));
}

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
    skills: Arc<SkillsService>,
    cancellations: Arc<Mutex<HashMap<String, AgentCancellationToken>>>,
    pending_actions: Arc<Mutex<HashMap<String, PendingActionRecord>>>,
    usage_contexts: Arc<Mutex<HashMap<String, AgentRunUsageState>>>,
    trace_snapshots: Arc<Mutex<HashMap<String, ConversationTraceSnapshot>>>,
    conversation_context_states: Arc<Mutex<HashMap<String, ConversationContextStateEntry>>>,
    conversation_context_state_clock: Arc<AtomicU64>,
    context_compaction_summary_generator: Option<ContextCompactionSummaryGenerator>,
    command_runs: CommandRunState,
    deleting_projects: Arc<Mutex<HashSet<String>>>,
}

impl AgentService {
    pub fn new(storage: Arc<StorageService>) -> Self {
        let pending_actions = load_persisted_pending_actions(&storage);
        Self {
            storage,
            skills: Arc::new(SkillsService::new()),
            cancellations: Arc::new(Mutex::new(HashMap::new())),
            pending_actions: Arc::new(Mutex::new(pending_actions)),
            usage_contexts: Arc::new(Mutex::new(HashMap::new())),
            trace_snapshots: Arc::new(Mutex::new(HashMap::new())),
            conversation_context_states: Arc::new(Mutex::new(HashMap::new())),
            conversation_context_state_clock: Arc::new(AtomicU64::new(1)),
            context_compaction_summary_generator: None,
            command_runs: CommandRunState::default(),
            deleting_projects: Arc::new(Mutex::new(HashSet::new())),
        }
    }

    pub fn with_skills_service(mut self, skills: Arc<SkillsService>) -> Self {
        self.skills = skills;
        self
    }

    #[cfg(test)]
    fn with_context_compaction_summary_generator(
        mut self,
        generator: ContextCompactionSummaryGenerator,
    ) -> Self {
        self.context_compaction_summary_generator = Some(generator);
        self
    }

    pub fn start_conversation_turn(
        &self,
        input: AgentConversationTurnInput,
        notifications: CoreServerNotificationSender,
    ) -> Result<AgentConversationTurnOutput, AgentServiceError> {
        if self.is_project_deleting(input.project_id.as_deref()) {
            return Err("项目正在移除，无法开始新的 agent 运行。".to_string().into());
        }
        let run_id = next_run_id();
        let cancellation_token = AgentCancellationToken::new();
        self.register_cancellation(&run_id, cancellation_token.clone());

        let prepared = match prepare_conversation_turn(&self.storage, &self.skills, input, &run_id)
        {
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
            return Err("项目正在移除，无法开始新的 agent 运行。".to_string().into());
        }

        let output = prepared.output.clone();
        let service = self.clone();
        let worker_run_id = run_id.clone();
        let worker_conversation_id = output.conversation_id.clone();
        let worker_assistant_message_id = output.assistant_message_id.clone();
        let worker_assistant_created_at = output.assistant_message.created_at;
        let pending_agent_input = prepared.agent_input.clone();

        tokio::spawn(async move {
            let emitter_notifications = notifications.clone();
            let emitter_service = service.clone();
            let emitter_conversation_id = worker_conversation_id.clone();
            let emitter_assistant_message_id = worker_assistant_message_id.clone();
            let emitter_agent_input = pending_agent_input.clone();
            let terminal_event_gate = Arc::new(AgentTerminalEventGate::default());
            let emitter_terminal_event_gate = terminal_event_gate.clone();
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
                if let Some(event) = emitter_terminal_event_gate.route(event) {
                    let _ = emitter_notifications.send(agent_event_notification(event));
                }
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
                worker_assistant_created_at,
                pending_agent_input.clone(),
                notifications.clone(),
            );
            let context_compaction_services = service.context_compaction_services(
                &worker_run_id,
                &worker_conversation_id,
                &worker_assistant_message_id,
                pending_agent_input.clone(),
                notifications.clone(),
            );
            let model_request_observer = service.model_request_observer(
                &worker_run_id,
                &worker_conversation_id,
                &worker_assistant_message_id,
            );
            let host_services = AgentRuntimeHostServices::new()
                .with_host_actions(host_executor, service.storage.clone())
                .with_trace_observer(trace_observer)
                .with_model_request_observer(model_request_observer)
                .with_context_compaction(context_compaction_services);
            let result = send_chat_with_host_services(
                prepared.agent_input,
                worker_run_id.clone(),
                emitter,
                cancellation_token,
                host_services,
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
                    } else if let Err(error) = &persisted {
                        service.discard_usage_context(&worker_run_id);
                        let _ = notifications.send(agent_event_notification(AgentEvent::Error {
                            run_id: Some(worker_run_id.clone()),
                            message: format!("无法原子持久化 assistant 终态与会话轨迹：{error}"),
                            recoverable: true,
                            code: Some("conversation_trace_persistence_failed".to_string()),
                            details: None,
                        }));
                    }
                    if committed_durable_context {
                        emit_terminal_events_after_persistence(
                            &notifications,
                            &terminal_event_gate,
                            &agent_output,
                        );
                    }
                }
                Err(error) => {
                    terminal_event_gate.discard();
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
                self.persist_pending_status(record, PendingActionStatus::Cancelled)?;
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
        self.persist_pending_status(record, PendingActionStatus::Cancelled)?;
        record.snapshot.status = PendingActionStatus::Cancelled;
        let record = record.clone();
        drop(pending_actions);
        drop(deleting_projects);
        if let Err(error) = self.finalize_cancelled_pending_action(&record) {
            if let Err(rollback_error) =
                self.transition_pending_status(&record, PendingActionStatus::Pending)
            {
                return Err(format!(
                    "{error} 此外，待审批操作无法回滚为 pending；已保持 cancelled 状态以避免内存与数据库分歧：{rollback_error}"
                ));
            }
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
            self.persist_pending_status(record, pending_status)?;
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
            if let Err(error) = self.transition_pending_status(&record, PendingActionStatus::Failed)
            {
                emit_pending_transition_error(
                    &notifications,
                    &run_id,
                    PendingActionStatus::Failed,
                    &error,
                );
            }
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
            if let Err(transition_error) =
                self.transition_pending_status(&record, PendingActionStatus::Failed)
            {
                emit_pending_transition_error(
                    &notifications,
                    &run_id,
                    PendingActionStatus::Failed,
                    &transition_error,
                );
            }
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
                if let Err(transition_error) =
                    self.transition_pending_status(&record, PendingActionStatus::Failed)
                {
                    emit_pending_transition_error(
                        &notifications,
                        &run_id,
                        PendingActionStatus::Failed,
                        &transition_error,
                    );
                }
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
            if let Err(error) =
                self.transition_pending_status(&record, PendingActionStatus::Cancelled)
            {
                emit_pending_transition_error(
                    &notifications,
                    &run_id,
                    PendingActionStatus::Cancelled,
                    &error,
                );
                self.unregister_cancellation(&run_id);
                return;
            }
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
        let terminal_event_gate = Arc::new(AgentTerminalEventGate::default());
        let emitter_terminal_event_gate = terminal_event_gate.clone();
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
            if let Some(event) = emitter_terminal_event_gate.route(event) {
                let _ = emitter_notifications.send(agent_event_notification(event));
            }
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
        let context_compaction_services = self.context_compaction_services(
            &run_id,
            trace_conversation_id,
            trace_assistant_message_id,
            record.agent_input.clone(),
            notifications.clone(),
        );
        let model_request_observer =
            self.model_request_observer(&run_id, trace_conversation_id, trace_assistant_message_id);
        let host_services = AgentRuntimeHostServices::new()
            .with_host_actions(host_executor, self.storage.clone())
            .with_trace_observer(trace_observer)
            .with_model_request_observer(model_request_observer)
            .with_context_compaction(context_compaction_services);
        let result = send_chat_with_host_services(
            agent_input,
            run_id.clone(),
            emitter,
            cancellation_token,
            host_services,
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
                let pending_transition = self
                    .transition_pending_status(&record, final_pending_status)
                    .inspect_err(|error| {
                        terminal_event_gate.discard();
                        emit_pending_transition_error(
                            &notifications,
                            &run_id,
                            final_pending_status,
                            error,
                        );
                    });
                if let (Some(conversation_id), Some(assistant_message_id)) = (
                    record.snapshot.conversation_id.as_deref(),
                    record.snapshot.assistant_message_id.as_deref(),
                ) {
                    let persisted = self.persist_final_assistant_output(
                        conversation_id,
                        assistant_message_id,
                        &agent_output,
                    );
                    if persisted.is_ok() && pending_transition.is_ok() && committed_durable_context
                    {
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
                    } else if let Err(error) = &persisted {
                        self.discard_usage_context(&run_id);
                        let _ = notifications.send(agent_event_notification(AgentEvent::Error {
                            run_id: Some(run_id.clone()),
                            message: format!("无法原子持久化 assistant 终态与会话轨迹：{error}"),
                            recoverable: true,
                            code: Some("conversation_trace_persistence_failed".to_string()),
                            details: None,
                        }));
                    }
                    if pending_transition.is_ok() && committed_durable_context {
                        emit_terminal_events_after_persistence(
                            &notifications,
                            &terminal_event_gate,
                            &agent_output,
                        );
                    }
                }
            }
            Err(error) => {
                terminal_event_gate.discard();
                let pending_transition = self
                    .transition_pending_status(&record, PendingActionStatus::Failed)
                    .inspect_err(|transition_error| {
                        emit_pending_transition_error(
                            &notifications,
                            &run_id,
                            PendingActionStatus::Failed,
                            transition_error,
                        );
                    });
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
                    if persisted.is_ok() && pending_transition.is_ok() {
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
                if pending_transition.is_ok() {
                    let _ = notifications.send(agent_event_notification(AgentEvent::Done {
                        run_id: run_id.clone(),
                        success: false,
                        status: Some(AgentRunStatus::Failed),
                        content: Some(message),
                        usage,
                        finish_reason: None,
                        proposed_actions: Vec::new(),
                    }));
                }
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

    fn transition_pending_status(
        &self,
        record: &PendingActionRecord,
        status: PendingActionStatus,
    ) -> Result<(), String> {
        let action_id = &record.snapshot.action_id;
        let mut pending_actions = self
            .pending_actions
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let pending = pending_actions
            .get_mut(action_id)
            .ok_or_else(|| format!("待审批操作的内存状态不存在：{action_id}"))?;
        self.persist_pending_status(pending, status)?;
        pending.snapshot.status = status;
        Ok(())
    }

    fn persist_pending_action(&self, record: &PendingActionRecord) -> Result<(), String> {
        self.storage
            .upsert_pending_agent_action(pending_storage_record(record, now_ms()))
    }

    fn persist_pending_status(
        &self,
        record: &PendingActionRecord,
        status: PendingActionStatus,
    ) -> Result<(), String> {
        self.storage.transition_pending_agent_action(
            &record.snapshot.action_id,
            pending_status_label(status),
            &persisted_pending_agent_input_json(&record.agent_input, status),
            now_ms(),
        )
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

    fn model_request_observer(
        &self,
        expected_run_id: &str,
        expected_conversation_id: &str,
        expected_assistant_message_id: &str,
    ) -> AgentModelRequestObserver {
        let storage = self.storage.clone();
        let expected_run_id = expected_run_id.to_string();
        let expected_conversation_id = expected_conversation_id.to_string();
        let expected_assistant_message_id = expected_assistant_message_id.to_string();
        Arc::new(move |observation| {
            let identity_matches = observation.run_id == expected_run_id
                && observation.conversation_id.as_deref()
                    == Some(expected_conversation_id.as_str())
                && observation.assistant_message_id.as_deref()
                    == Some(expected_assistant_message_id.as_str());
            if !identity_matches {
                eprintln!(
                    "refused model request observation with mismatched run or conversation identity: {}",
                    observation.id
                );
                return;
            }
            if let Err(error) = storage.save_model_request_observation(&observation) {
                // A provider response may already be visible to the user. Diagnostics must never
                // make the runtime replay that response and duplicate tool side effects.
                eprintln!(
                    "failed to persist model request observation {}: {error}",
                    observation.id
                );
            }
        })
    }

    #[allow(clippy::too_many_arguments)]
    fn context_compaction_services(
        &self,
        run_id: &str,
        conversation_id: &str,
        assistant_message_id: &str,
        agent_input: AgentChatInput,
        notifications: CoreServerNotificationSender,
    ) -> AgentContextCompactionServices {
        let generator: ContextCompactionSummaryGenerator = self
            .context_compaction_summary_generator
            .clone()
            .unwrap_or_else(|| {
                let generator = AgentContextCompactionModelGenerator::from_chat_input(&agent_input);
                Arc::new(move |request, cancellation| {
                    let generator = generator.clone();
                    Box::pin(async move { generator.generate(request, cancellation).await })
                })
            });
        let run_id = run_id.to_string();
        let conversation_id = conversation_id.to_string();
        let assistant_message_id = assistant_message_id.to_string();

        let prepare_service = self.clone();
        let prepare_run_id = run_id.clone();
        let prepare_conversation_id = conversation_id.clone();
        let prepare_assistant_message_id = assistant_message_id.clone();
        let prepare_agent_input = agent_input.clone();
        let prepare_notifications = notifications.clone();

        let commit_service = self.clone();
        let commit_run_id = run_id.clone();
        let commit_conversation_id = conversation_id.clone();
        let commit_assistant_message_id = assistant_message_id.clone();
        let commit_agent_input = agent_input;
        let commit_notifications = notifications.clone();

        let receipt_storage = self.storage.clone();
        let receipt_run_id = run_id;
        let receipt_conversation_id = conversation_id;
        let receipt_assistant_message_id = assistant_message_id;

        AgentContextCompactionServices::new(
            move |request, cancellation| {
                let service = prepare_service.clone();
                let run_id = prepare_run_id.clone();
                let conversation_id = prepare_conversation_id.clone();
                let assistant_message_id = prepare_assistant_message_id.clone();
                let agent_input = prepare_agent_input.clone();
                let notifications = prepare_notifications.clone();
                async move {
                    cancellation.check()?;
                    validate_compaction_request_identity(
                        &request.run_id,
                        &request.conversation_id,
                        &request.assistant_message_id,
                        &run_id,
                        &conversation_id,
                        &assistant_message_id,
                    )?;
                    let active_trace = service
                        .storage
                        .get_conversation_turn_trace(&assistant_message_id)
                        .map_err(AgentError::new)?;
                    validate_compaction_model_visible_boundary(
                        &request.covered_through,
                        active_trace.as_ref(),
                        &run_id,
                        &conversation_id,
                        &assistant_message_id,
                        request.visible_trace_item_count,
                    )?;
                    let prefix = service
                        .storage
                        .prepare_context_compaction_prefix_if_current(
                            &conversation_id,
                            &request.covered_through,
                            request.expected_previous_summary_id.as_deref(),
                        )
                        .map_err(AgentError::new)?;
                    match prefix {
                        Some(prefix) => Ok(AgentContextCompactionPrepareOutcome::Ready(Arc::new(
                            prefix,
                        ))),
                        None => service
                            .rebuild_running_context_after_compaction(
                                &agent_input,
                                &run_id,
                                &conversation_id,
                                &assistant_message_id,
                                request.visible_trace_item_count,
                                &notifications,
                            )
                            .map(|baseline| {
                                AgentContextCompactionPrepareOutcome::Refresh(Box::new(baseline))
                            })
                            .map_err(AgentError::new),
                    }
                }
            },
            move |request, cancellation| generator(request, cancellation),
            move |request: AgentContextCompactionCommitRequest, cancellation| {
                let service = commit_service.clone();
                let run_id = commit_run_id.clone();
                let conversation_id = commit_conversation_id.clone();
                let assistant_message_id = commit_assistant_message_id.clone();
                let agent_input = commit_agent_input.clone();
                let notifications = commit_notifications.clone();
                async move {
                    cancellation.check()?;
                    validate_compaction_request_identity(
                        &request.run_id,
                        &request.conversation_id,
                        &request.assistant_message_id,
                        &run_id,
                        &conversation_id,
                        &assistant_message_id,
                    )?;
                    validate_compaction_request_identity(
                        &request.receipt.run_id,
                        &request.receipt.conversation_id,
                        &request.receipt.assistant_message_id,
                        &run_id,
                        &conversation_id,
                        &assistant_message_id,
                    )?;
                    let active_trace = service
                        .storage
                        .get_conversation_turn_trace(&assistant_message_id)
                        .map_err(AgentError::new)?;
                    validate_compaction_model_visible_boundary(
                        &request.prefix.covered_through,
                        active_trace.as_ref(),
                        &run_id,
                        &conversation_id,
                        &assistant_message_id,
                        request.visible_trace_item_count,
                    )?;
                    let committed = service
                        .storage
                        .commit_context_compaction_prefix_with_receipt_if_current(
                            request.prefix.as_ref(),
                            request.draft,
                            &request.receipt,
                            &request.observation,
                        )
                        .map_err(AgentError::new)?;
                    service.invalidate_conversation_context_state(&conversation_id);
                    let baseline = service.rebuild_running_context_after_compaction(
                        &agent_input,
                        &run_id,
                        &conversation_id,
                        &assistant_message_id,
                        request.visible_trace_item_count,
                        &notifications,
                    );
                    match (committed, baseline) {
                        (Some(summary), Ok(baseline)) => {
                            Ok(AgentContextCompactionCommitOutcome::Applied {
                                summary_id: summary.id,
                                baseline: Box::new(baseline),
                            })
                        }
                        (Some(summary), Err(error)) => Err(AgentError::structured(
                            "context_compaction_applied_rebuild_failed",
                            "上下文摘要已原子提交，但无法重建当前运行的上下文。",
                            serde_json::json!({
                                "summaryId": summary.id,
                                "cause": error,
                            }),
                        )),
                        (None, Ok(baseline)) => Ok(AgentContextCompactionCommitOutcome::Refresh(
                            Box::new(baseline),
                        )),
                        (None, Err(error)) => Err(AgentError::new(error)),
                    }
                }
            },
            move |receipt, observation| {
                let storage = receipt_storage.clone();
                let run_id = receipt_run_id.clone();
                let conversation_id = receipt_conversation_id.clone();
                let assistant_message_id = receipt_assistant_message_id.clone();
                async move {
                    validate_compaction_request_identity(
                        &receipt.run_id,
                        &receipt.conversation_id,
                        &receipt.assistant_message_id,
                        &run_id,
                        &conversation_id,
                        &assistant_message_id,
                    )?;
                    storage
                        .record_context_compaction_receipt(&receipt, observation.as_ref())
                        .map_err(AgentError::new)
                }
            },
        )
    }

    fn rebuild_running_context_after_compaction(
        &self,
        agent_input: &AgentChatInput,
        run_id: &str,
        conversation_id: &str,
        assistant_message_id: &str,
        visible_trace_item_count: usize,
        notifications: &CoreServerNotificationSender,
    ) -> Result<AgentContextBaseline, String> {
        self.invalidate_conversation_context_state(conversation_id);
        let conversation = self
            .storage
            .load_conversation(conversation_id)?
            .ok_or_else(|| format!("未找到对话：{conversation_id}"))?;
        let traces = self
            .storage
            .list_conversation_turn_traces(conversation_id)?;
        let summary = self
            .storage
            .get_active_context_compaction_summary(conversation_id)?;
        let active_trace = traces
            .iter()
            .find(|trace| trace.assistant_message_id == assistant_message_id);
        if let Some(trace) = active_trace {
            if trace.run_id != run_id
                || trace.conversation_id != conversation_id
                || trace.terminal_status != ConversationTurnTraceTerminalStatus::InProgress
            {
                return Err("压缩后重建上下文时，运行中 trace 身份或状态不一致。".to_string());
            }
            if visible_trace_item_count > trace.items.len() {
                return Err("压缩后重建上下文时，模型可见 trace 游标超出日志末尾。".to_string());
            }
        } else if visible_trace_item_count != 0 {
            return Err("压缩后重建上下文时，模型可见 trace 游标没有对应日志。".to_string());
        }

        // Build the runtime baseline at the model-visible cursor. The server cache is extended to
        // the physical log tail below so the context indicator remains an immediate durable view.
        let mut visible_traces = traces.clone();
        if let Some(trace) = visible_traces
            .iter_mut()
            .find(|trace| trace.assistant_message_id == assistant_message_id)
        {
            trace.items.truncate(visible_trace_item_count);
            trace.terminal_status = ConversationTurnTraceTerminalStatus::InProgress;
            trace.terminal_error = None;
            trace
                .validate()
                .map_err(|error| format!("压缩后重建上下文时，模型可见 trace 前缀无效：{error}"))?;
        }

        let mut preview_input = agent_input.clone();
        preview_input.messages = conversation_history_messages_with_compaction(
            &conversation,
            &visible_traces,
            summary.as_ref(),
            &[],
        );
        preview_input.context_compaction_summary = summary;
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

        let mut state =
            create_conversation_context_state(preview_input).map_err(|error| error.to_string())?;
        // Freeze the exact persistent prefix that the runtime may adopt without exposing a tool
        // result to compaction before the main model has observed it once.
        let runtime_baseline = state.shared_baseline().map_err(|error| error.to_string())?;
        let committed_trace_items = match active_trace {
            Some(trace) => state
                .append_trace_items(trace, visible_trace_item_count)
                .map_err(|error| error.to_string())?,
            None => 0,
        };
        // Measure and freeze the appended trace chunk once for the conversation cache and circle.
        state.shared_baseline().map_err(|error| error.to_string())?;
        let snapshot = if agent_input.context_window_indicator_enabled {
            Some(
                state
                    .snapshot_with_skill_activation(
                        AgentContextWindowPhase::DurableCommit,
                        agent_input.skill_activation.as_ref(),
                    )
                    .map_err(|error| error.to_string())?,
            )
        } else {
            None
        };
        let entry = ConversationContextStateEntry {
            configuration_revision: state.configuration_revision().to_string(),
            state,
            active_run_id: Some(run_id.to_string()),
            active_assistant_message_id: Some(assistant_message_id.to_string()),
            committed_trace_items,
            terminal: false,
            last_access: self.next_conversation_context_state_access(),
        };
        self.insert_conversation_context_state(conversation_id, entry);
        self.emit_context_window_snapshot(notifications, run_id, conversation_id, snapshot);
        Ok(runtime_baseline)
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
                            .map(|message| {
                                (
                                    message.message_id.as_deref(),
                                    message.content.as_str(),
                                    message.created_at,
                                )
                            })
                            .unwrap_or((None, "", None));
                        (|| {
                            entry.state.append_user_message(
                                current_user.0,
                                current_user.1,
                                current_user.2,
                            )?;
                            entry.active_run_id = Some(run_id.to_string());
                            entry.active_assistant_message_id =
                                Some(assistant_message_id.to_string());
                            entry.committed_trace_items = 0;
                            entry.terminal = false;
                            entry.state.append_trace_items(trace, 0)
                        })()
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
                            let snapshot = if agent_input.context_window_indicator_enabled {
                                Some(
                                    entry
                                        .state
                                        .snapshot_with_skill_activation(
                                            AgentContextWindowPhase::DurableCommit,
                                            agent_input.skill_activation.as_ref(),
                                        )
                                        .map_err(|error| error.to_string())?,
                                )
                            } else {
                                None
                            };
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
            agent_input.skill_activation.as_ref(),
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
    ) -> Result<AgentContextWindowSnapshotOutput, AgentServiceError> {
        let model_id = input.model_id.trim();
        if model_id.is_empty() {
            return Err("modelId 不能为空。".to_string().into());
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
            return Err(format!("模型未启用：{model_id}").into());
        }
        let connection = settings.effective_connection_for(&model)?;

        let conversation_id = normalized_optional(input.conversation_id.as_deref());
        let conversation = match conversation_id.as_deref() {
            Some(conversation_id) => self.storage.load_conversation(conversation_id)?,
            None => None,
        };
        let project_id = resolve_conversation_project_id(
            conversation.as_ref(),
            normalized_optional(input.project_id.as_deref()),
        )?;
        let project = resolve_project(&self.storage, project_id.as_deref())?;
        let prepared_skills = if input.skills.is_empty() {
            PreparedSkillActivation::default()
        } else {
            let project = project.as_ref().ok_or_else(missing_workspace_failure)?;
            let workspace_root = project
                .path
                .as_deref()
                .map(std::path::PathBuf::from)
                .ok_or_else(missing_workspace_failure)?;
            activate_workspace(&self.skills, &project.id, &workspace_root, &input.skills)?
        };
        let attachment_library = conversation_id
            .as_deref()
            .map(|conversation_id| {
                self.storage
                    .build_attachment_library_context(conversation_id, project_id.as_deref())
            })
            .transpose()?;
        let context_compaction_summary = match conversation_id.as_deref() {
            Some(conversation_id) => self
                .storage
                .get_active_context_compaction_summary(conversation_id)?,
            None => None,
        };
        let messages = match conversation.as_ref() {
            Some(conversation) => {
                let traces = self
                    .storage
                    .list_conversation_turn_traces(&conversation.id)?;
                conversation_history_messages_with_compaction(
                    conversation,
                    &traces,
                    context_compaction_summary.as_ref(),
                    &[],
                )
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
            api_url: connection.api_url,
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
            context_compaction_summary,
            skill_activation: prepared_skills.runtime,
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

    pub fn get_context_compaction_audit(
        &self,
        input: AgentContextCompactionAuditInput,
    ) -> Result<AgentContextCompactionAuditOutput, String> {
        let conversation_id = input.conversation_id.trim();
        if conversation_id.is_empty() {
            return Err("conversationId 不能为空。".to_string());
        }
        let operation_id = input
            .operation_id
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty());
        if input.operation_id.is_some() && operation_id.is_none() {
            return Err("operationId 不能为空字符串。".to_string());
        }
        let limit = input.limit.unwrap_or(20);
        if !(1..=100).contains(&limit) {
            return Err("limit 必须在 1 到 100 之间。".to_string());
        }
        self.storage
            .get_context_compaction_audit(conversation_id, operation_id, limit)
            .map(|report| AgentContextCompactionAuditOutput { report })
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
        let context_compaction_summary = self
            .storage
            .get_active_context_compaction_summary(conversation_id)?;
        preview_input.messages = conversation_history_messages_with_compaction(
            &conversation,
            &traces,
            context_compaction_summary.as_ref(),
            &[],
        );
        preview_input.context_compaction_summary = context_compaction_summary;
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
                    return entry
                        .state
                        .snapshot_with_skill_activation(
                            phase,
                            agent_input.skill_activation.as_ref(),
                        )
                        .map(Some)
                        .map_err(|error| error.to_string());
                }
                states.remove(conversation_id);
            }
        }
        self.rebuild_conversation_context_state(
            agent_input,
            conversation_id,
            phase,
            None,
            agent_input.skill_activation.as_ref(),
        )
        .map(|update| update.snapshot)
    }

    fn rebuild_conversation_context_state(
        &self,
        agent_input: &AgentChatInput,
        conversation_id: &str,
        phase: AgentContextWindowPhase,
        active_run_id: Option<&str>,
        snapshot_skill_activation: Option<&mycopilot_core::AgentSkillActivation>,
    ) -> Result<ConversationContextStateUpdate, String> {
        let (preview_input, traces) =
            self.persisted_conversation_context_state(agent_input, conversation_id)?;
        let mut state =
            create_conversation_context_state(preview_input).map_err(|error| error.to_string())?;
        let baseline = state.shared_baseline().map_err(|error| error.to_string())?;
        let snapshot = if agent_input.context_window_indicator_enabled {
            Some(
                state
                    .snapshot_with_skill_activation(phase, snapshot_skill_activation)
                    .map_err(|error| error.to_string())?,
            )
        } else {
            None
        };
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
        let assistant_created_at = self
            .storage
            .get_assistant_message_created_at(conversation_id, assistant_message_id)?
            .ok_or_else(|| format!("assistant 终态缺少消息创建时间：{assistant_message_id}"))?;
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
                            Some(assistant_created_at),
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
            None,
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

fn validate_compaction_request_identity(
    request_run_id: &str,
    request_conversation_id: &str,
    request_assistant_message_id: &str,
    expected_run_id: &str,
    expected_conversation_id: &str,
    expected_assistant_message_id: &str,
) -> AgentResult<()> {
    if request_run_id == expected_run_id
        && request_conversation_id == expected_conversation_id
        && request_assistant_message_id == expected_assistant_message_id
    {
        return Ok(());
    }
    Err(AgentError::structured(
        "context_compaction_identity_mismatch",
        "上下文压缩请求与当前运行身份不一致。",
        serde_json::json!({
            "requestRunId": request_run_id,
            "requestConversationId": request_conversation_id,
            "requestAssistantMessageId": request_assistant_message_id,
        }),
    ))
}

fn validate_compaction_model_visible_boundary(
    covered_through: &ContextJournalCursor,
    active_trace: Option<&ConversationTurnTrace>,
    expected_run_id: &str,
    expected_conversation_id: &str,
    expected_assistant_message_id: &str,
    visible_trace_item_count: usize,
) -> AgentResult<()> {
    let Some(trace) = active_trace else {
        if visible_trace_item_count == 0
            && !matches!(
                covered_through,
                ContextJournalCursor::TraceItem {
                    assistant_message_id,
                    ..
                } if assistant_message_id == expected_assistant_message_id
            )
        {
            return Ok(());
        }
        return Err(AgentError::structured(
            "context_compaction_visibility_mismatch",
            "上下文压缩请求缺少当前运行的会话轨迹。",
            serde_json::json!({
                "assistantMessageId": expected_assistant_message_id,
                "visibleTraceItemCount": visible_trace_item_count,
            }),
        ));
    };
    if trace.run_id != expected_run_id
        || trace.conversation_id != expected_conversation_id
        || trace.assistant_message_id != expected_assistant_message_id
        || trace.terminal_status != ConversationTurnTraceTerminalStatus::InProgress
    {
        return Err(AgentError::structured(
            "context_compaction_visibility_mismatch",
            "上下文压缩请求对应的运行中会话轨迹身份无效。",
            serde_json::json!({
                "runId": trace.run_id,
                "conversationId": trace.conversation_id,
                "assistantMessageId": trace.assistant_message_id,
                "terminalStatus": trace.terminal_status,
            }),
        ));
    }
    if visible_trace_item_count > trace.items.len()
        || visible_trace_item_count > 0
            && !trace.items[visible_trace_item_count - 1].is_safe_compaction_boundary()
    {
        return Err(AgentError::structured(
            "context_compaction_visibility_mismatch",
            "上下文压缩请求的模型可见轨迹边界无效。",
            serde_json::json!({
                "visibleTraceItemCount": visible_trace_item_count,
                "persistedTraceItemCount": trace.items.len(),
            }),
        ));
    }
    if let ContextJournalCursor::TraceItem {
        assistant_message_id,
        sequence,
    } = covered_through
    {
        if assistant_message_id == expected_assistant_message_id
            && !trace.items[..visible_trace_item_count]
                .iter()
                .any(|item| item.sequence() == *sequence)
        {
            return Err(AgentError::structured(
                "context_compaction_unseen_trace_item",
                "上下文压缩不能覆盖主模型尚未看过的工具轨迹。",
                serde_json::json!({
                    "assistantMessageId": assistant_message_id,
                    "sequence": sequence,
                    "visibleTraceItemCount": visible_trace_item_count,
                }),
            ));
        }
    }
    Ok(())
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
        agent_input_json: persisted_pending_agent_input_json(
            &record.agent_input,
            record.snapshot.status,
        ),
        created_at: record.snapshot.created_at,
        updated_at,
    }
}

fn persisted_pending_agent_input_json(
    agent_input: &AgentChatInput,
    status: PendingActionStatus,
) -> String {
    let mut persisted_agent_input = agent_input.clone();
    persisted_agent_input.api_token.clear();
    // Pending actions must survive restart, while an approved action may still be executing and
    // need its continuation input. Once the action is terminal, the live continuation owns any
    // remaining in-memory copy; the durable row only retains Skill identity and revision metadata.
    if pending_status_redacts_run_scoped_input(status) {
        if let Some(activation) = persisted_agent_input.skill_activation.as_mut() {
            for skill in &mut activation.skills {
                skill.instructions.clear();
            }
        }
        if let Some(checkpoint) = persisted_agent_input.resume_checkpoint.as_mut() {
            for item in &mut checkpoint.context_items {
                let is_skill_instructions = item
                    .sources
                    .iter()
                    .any(|source| source == "skill_instructions")
                    || item
                        .origin
                        .as_ref()
                        .is_some_and(|origin| origin.kind == "skill");
                if is_skill_instructions {
                    item.content.clear();
                }
            }
        }
    }
    serialize_json(&persisted_agent_input)
}

fn pending_status_redacts_run_scoped_input(status: PendingActionStatus) -> bool {
    match status {
        PendingActionStatus::Pending | PendingActionStatus::Approved => false,
        PendingActionStatus::Rejected
        | PendingActionStatus::Cancelled
        | PendingActionStatus::Completed
        | PendingActionStatus::Failed => true,
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

    let conversation_model_id = agent_input
        .context
        .as_ref()
        .and_then(|context| context.conversation_id.as_deref())
        .and_then(|conversation_id| storage.load_conversation(conversation_id).ok().flatten())
        .and_then(|conversation| conversation.model_id);
    let model = conversation_model_id
        .as_deref()
        .and_then(|model_id| settings.models.iter().find(|model| model.id == model_id))
        .or_else(|| {
            settings.models.iter().find(|model| {
                model.id == agent_input.model || model_provider_path(model) == agent_input.model
            })
        });

    // Pending actions deliberately persist without API tokens. On restoration, resolve the
    // same model-specific-or-global pair used by a fresh run so approval continuation cannot
    // silently switch providers after an app restart.
    if let Some(model) = model {
        if let Ok(connection) = settings.effective_connection_for(model) {
            agent_input.api_url = connection.api_url;
            agent_input.api_token = connection.api_token;
        }
    }
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
        ProjectRecord,
    };
    use mycopilot_core::{
        AgentActivatedSkill, AgentCommandRequest, AgentCommandRiskLevel, AgentPermissions,
        AgentSkillActivation, AgentUsageSummaryRange, AgentWorkspaceContext,
        ContextCompactionGeneration, ContextCompactionPrefix, ContextCompactionSourceItem,
        ContextCompactionSummary, ContextCompactionSummaryDraft, ContextJournalCursor,
        ConversationTraceToolResultStatus, ConversationTurnTraceItem,
        ConversationTurnTraceTerminalStatus, CONTEXT_COMPACTION_SUMMARY_SCHEMA_VERSION,
        CONVERSATION_TURN_TRACE_SCHEMA_VERSION,
    };
    use serde_json::json;
    use std::fs;
    use tempfile::tempdir;

    fn completed_output_for_terminal_gate() -> AgentChatOutput {
        AgentChatOutput {
            content: "finished".to_string(),
            status: AgentRunStatus::Completed,
            run_id: "run-terminal-gate".to_string(),
            events: Vec::new(),
            tool_definitions: Vec::new(),
            todo: None,
            usage: None,
            finish_reason: Some("stop".to_string()),
            proposed_actions: Vec::new(),
            conversation_turn_trace: None,
        }
    }

    #[test]
    fn terminal_event_gate_defers_settled_state_and_done_until_commit() {
        let gate = AgentTerminalEventGate::default();
        let message = AgentEvent::Message {
            run_id: "run-terminal-gate".to_string(),
            content: "still streaming".to_string(),
        };
        assert!(matches!(
            gate.route(message),
            Some(AgentEvent::Message { .. })
        ));

        assert!(gate
            .route(AgentEvent::State {
                run_id: "run-terminal-gate".to_string(),
                state: mycopilot_core::AgentStateSnapshot {
                    status: AgentRunStatus::Completed,
                    active_run_id: None,
                    last_error: None,
                    updated_at: 1,
                },
            })
            .is_none());
        assert!(gate
            .route(terminal_done_event(&completed_output_for_terminal_gate()))
            .is_none());

        let committed = gate.take_after_persistence(&completed_output_for_terminal_gate());
        assert!(matches!(committed.as_slice(), [
            AgentEvent::State { state, .. },
            AgentEvent::Done { success: true, .. }
        ] if state.status == AgentRunStatus::Completed));
    }

    #[test]
    fn terminal_event_gate_does_not_delay_approval_waiting_done() {
        let gate = AgentTerminalEventGate::default();
        let routed = gate.route(AgentEvent::Done {
            run_id: "run-terminal-gate".to_string(),
            success: false,
            status: Some(AgentRunStatus::WaitingForApproval),
            content: None,
            usage: None,
            finish_reason: None,
            proposed_actions: Vec::new(),
        });

        assert!(matches!(
            routed,
            Some(AgentEvent::Done {
                status: Some(AgentRunStatus::WaitingForApproval),
                ..
            })
        ));
    }

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

    fn test_compaction_draft(
        prefix: &ContextCompactionPrefix,
        id: &str,
        content: &str,
        source_input_tokens: u64,
        created_at: i64,
    ) -> ContextCompactionSummaryDraft {
        assert!(source_input_tokens > 20);
        ContextCompactionSummaryDraft {
            id: id.to_string(),
            source_revision: prefix.source_revision.clone(),
            content: content.to_string(),
            continuity: mycopilot_core::ContextContinuitySnapshot::from_prefix(prefix).unwrap(),
            generation: ContextCompactionGeneration::test(),
            source_input_tokens,
            summary_input_tokens: 10,
            continuity_input_tokens: 10,
            replacement_input_tokens: 20,
            created_at,
        }
    }

    #[test]
    fn compaction_cannot_cross_the_model_visible_trace_boundary() {
        let trace = ConversationTurnTrace {
            schema_version: CONVERSATION_TURN_TRACE_SCHEMA_VERSION,
            run_id: "run-visible-boundary".to_string(),
            conversation_id: "conversation-visible-boundary".to_string(),
            assistant_message_id: "assistant-visible-boundary".to_string(),
            terminal_status: ConversationTurnTraceTerminalStatus::InProgress,
            terminal_error: None,
            truncated: false,
            items: vec![
                ConversationTurnTraceItem::AssistantNarration {
                    sequence: 0,
                    content: "I will read the file.".to_string(),
                    truncated: false,
                },
                ConversationTurnTraceItem::ToolCall {
                    sequence: 1,
                    call_id: "read-visible-boundary".to_string(),
                    tool: "read_file".to_string(),
                    operation: json!({ "path": "README.md" }),
                    approval_status: AgentApprovalStatus::NotRequired,
                    truncated: false,
                },
                ConversationTurnTraceItem::ToolResult {
                    sequence: 2,
                    call_id: "read-visible-boundary".to_string(),
                    tool: "read_file".to_string(),
                    status: ConversationTraceToolResultStatus::Succeeded,
                    success: true,
                    observation: json!({ "content": "contents" }),
                    approval_status: AgentApprovalStatus::NotRequired,
                    error: None,
                    truncated: false,
                },
            ],
        };
        let result_cursor = ContextJournalCursor::trace_item("assistant-visible-boundary", 2);

        let unseen = validate_compaction_model_visible_boundary(
            &result_cursor,
            Some(&trace),
            "run-visible-boundary",
            "conversation-visible-boundary",
            "assistant-visible-boundary",
            1,
        )
        .unwrap_err();
        assert_eq!(unseen.code(), Some("context_compaction_unseen_trace_item"));

        let split_exchange = validate_compaction_model_visible_boundary(
            &ContextJournalCursor::trace_item("assistant-visible-boundary", 0),
            Some(&trace),
            "run-visible-boundary",
            "conversation-visible-boundary",
            "assistant-visible-boundary",
            2,
        )
        .unwrap_err();
        assert_eq!(
            split_exchange.code(),
            Some("context_compaction_visibility_mismatch")
        );

        validate_compaction_model_visible_boundary(
            &result_cursor,
            Some(&trace),
            "run-visible-boundary",
            "conversation-visible-boundary",
            "assistant-visible-boundary",
            3,
        )
        .unwrap();
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
                api_url_override: None,
                api_token_override: None,
                supports_image: false,
                context_window_tokens: Some(128_000),
                input_price: "0.01".to_string(),
                output_price: "0.02".to_string(),
                enabled: true,
            }],
        }
    }

    fn write_test_skill(workspace: &Path, body: &str) {
        let skill_directory = workspace.join(".agents").join("skills").join("reviewer");
        fs::create_dir_all(&skill_directory).unwrap();
        fs::write(
            skill_directory.join("SKILL.md"),
            format!(
                "---\nname: repository-reviewer\ndescription: DESCRIPTION_DISCOVERY_ONLY\n---\n{body}\n"
            ),
        )
        .unwrap();
    }

    fn skill_turn_input(
        project_id: &str,
        selection: mycopilot_protocol_rs::SkillSelectionDto,
        conversation_id: &str,
    ) -> AgentConversationTurnInput {
        AgentConversationTurnInput {
            conversation_id: Some(conversation_id.to_string()),
            project_id: Some(project_id.to_string()),
            model_id: "model-1".to_string(),
            context_window_indicator_enabled: true,
            content: "Review this repository.".to_string(),
            attachments: Vec::new(),
            skills: vec![selection],
            title: None,
            user_message_id: Some(format!("user-{conversation_id}")),
            assistant_message_id: Some(format!("assistant-{conversation_id}")),
            max_tokens: None,
            temperature: None,
            prompt_preferences: None,
            permissions: AgentPermissions::default(),
        }
    }

    #[test]
    fn conversation_turn_resolves_skill_snapshot_before_persisting_the_run() {
        const INSTRUCTIONS: &str = "SKILL_SERVER_MARKER: inspect evidence before editing.";
        let fixture = tempdir().unwrap();
        let workspace = fixture.path().join("workspace");
        write_test_skill(&workspace, INSTRUCTIONS);
        let storage =
            Arc::new(StorageService::open(&fixture.path().join("storage.sqlite")).unwrap());
        storage.save_model_settings(test_model_settings()).unwrap();
        storage
            .save_project(ProjectRecord {
                id: "project-skills".to_string(),
                name: "Skill workspace".to_string(),
                path: Some(workspace.to_string_lossy().into_owned()),
                created_at: 1,
                pinned_at: None,
            })
            .unwrap();
        let skills = SkillsService::new();
        let catalog = skills.list_workspace("project-skills", &workspace).unwrap();
        let descriptor = catalog.skills().first().unwrap();
        let selection = mycopilot_protocol_rs::SkillSelectionDto {
            id: descriptor.id().as_str().to_string(),
            revision: descriptor.revision().as_str().to_string(),
        };

        let prepared = prepare_conversation_turn(
            &storage,
            &skills,
            skill_turn_input("project-skills", selection, "conversation-skills"),
            "run-skills",
        )
        .unwrap();

        let activation = prepared.agent_input.skill_activation.as_ref().unwrap();
        assert_eq!(activation.skills.len(), 1);
        assert_eq!(activation.skills[0].instructions.trim(), INSTRUCTIONS);
        assert_eq!(prepared.output.activated_skills.len(), 1);
        assert_eq!(
            prepared.output.skill_activation_revision.as_deref(),
            Some(activation.activation_revision.as_str())
        );
        let public_output = serde_json::to_string(&prepared.output).unwrap();
        assert!(!public_output.contains(INSTRUCTIONS));
        assert!(!public_output.contains("DESCRIPTION_DISCOVERY_ONLY"));

        let mut without_skill = prepared.agent_input.clone();
        without_skill.skill_activation = None;
        assert_eq!(
            conversation_context_configuration_revision(&prepared.agent_input).unwrap(),
            conversation_context_configuration_revision(&without_skill).unwrap()
        );
    }

    #[test]
    fn bundled_skill_crosses_the_production_turn_boundary_without_public_instruction_leakage() {
        const BUNDLED_INSTRUCTION_MARKER: &str = "Treat Application trust as package provenance";
        let fixture = tempdir().unwrap();
        let workspace = fixture.path().join("workspace");
        fs::create_dir_all(&workspace).unwrap();
        let storage =
            Arc::new(StorageService::open(&fixture.path().join("storage.sqlite")).unwrap());
        storage.save_model_settings(test_model_settings()).unwrap();
        storage
            .save_project(ProjectRecord {
                id: "project-bundled-skill".to_string(),
                name: "Bundled Skill workspace".to_string(),
                path: Some(workspace.to_string_lossy().into_owned()),
                created_at: 1,
                pinned_at: None,
            })
            .unwrap();

        let skills = SkillsService::new().with_bundled_source().unwrap();
        let catalog = skills
            .list_with_workspace("project-bundled-skill", &workspace)
            .unwrap();
        let descriptor = catalog
            .skills()
            .iter()
            .find(|skill| skill.source_kind() == mycopilot_core::skills::SkillSourceKind::Bundled)
            .expect("production catalog must expose the bundled auditor");
        let selection = mycopilot_protocol_rs::SkillSelectionDto {
            id: descriptor.id().as_str().to_string(),
            revision: descriptor.revision().as_str().to_string(),
        };

        let prepared = prepare_conversation_turn(
            &storage,
            &skills,
            skill_turn_input(
                "project-bundled-skill",
                selection,
                "conversation-bundled-skill",
            ),
            "run-bundled-skill",
        )
        .unwrap();

        let activation = prepared.agent_input.skill_activation.as_ref().unwrap();
        assert_eq!(activation.skills.len(), 1);
        assert_eq!(
            activation.skills[0].id,
            "bundled:application:repository-evidence-auditor"
        );
        assert!(activation.skills[0]
            .instructions
            .contains(BUNDLED_INSTRUCTION_MARKER));
        assert_eq!(prepared.output.activated_skills.len(), 1);
        assert_eq!(
            prepared.output.activated_skills[0].source.kind,
            mycopilot_protocol_rs::SkillSourceKindDto::Bundled
        );

        let public_output = serde_json::to_string(&prepared.output).unwrap();
        assert!(!public_output.contains(BUNDLED_INSTRUCTION_MARKER));
        let persisted = storage
            .load_conversation("conversation-bundled-skill")
            .unwrap()
            .unwrap();
        assert!(!serde_json::to_string(&persisted)
            .unwrap()
            .contains(BUNDLED_INSTRUCTION_MARKER));
    }

    #[test]
    fn stale_skill_selection_fails_before_conversation_mutation() {
        let fixture = tempdir().unwrap();
        let workspace = fixture.path().join("workspace");
        write_test_skill(&workspace, "first revision");
        let storage =
            Arc::new(StorageService::open(&fixture.path().join("storage.sqlite")).unwrap());
        storage.save_model_settings(test_model_settings()).unwrap();
        storage
            .save_project(ProjectRecord {
                id: "project-stale-skill".to_string(),
                name: "Skill workspace".to_string(),
                path: Some(workspace.to_string_lossy().into_owned()),
                created_at: 1,
                pinned_at: None,
            })
            .unwrap();
        let skills = SkillsService::new();
        let catalog = skills
            .list_workspace("project-stale-skill", &workspace)
            .unwrap();
        let descriptor = catalog.skills().first().unwrap();
        let selection = mycopilot_protocol_rs::SkillSelectionDto {
            id: descriptor.id().as_str().to_string(),
            revision: descriptor.revision().as_str().to_string(),
        };
        write_test_skill(&workspace, "second revision");

        let error = match prepare_conversation_turn(
            &storage,
            &skills,
            skill_turn_input("project-stale-skill", selection, "conversation-stale-skill"),
            "run-stale-skill",
        ) {
            Ok(_) => panic!("a stale Skill selection must fail before preparing the run"),
            Err(error) => error,
        };

        let data = error.skill_activation().unwrap();
        assert_eq!(data.code, "stale");
        assert_eq!(data.recovery, "refreshCatalog");
        assert!(data.expected_revision.is_some());
        assert!(data.actual_revision.is_some());
        assert!(storage
            .load_conversation("conversation-stale-skill")
            .unwrap()
            .is_none());
    }

    #[test]
    fn existing_conversation_rejects_cross_project_skill_turn_and_preview() {
        let fixture = tempdir().unwrap();
        let workspace_a = fixture.path().join("workspace-a");
        let workspace_b = fixture.path().join("workspace-b");
        fs::create_dir_all(&workspace_a).unwrap();
        write_test_skill(&workspace_b, "SKILL_PROJECT_B_MARKER");
        let storage =
            Arc::new(StorageService::open(&fixture.path().join("storage.sqlite")).unwrap());
        storage.save_model_settings(test_model_settings()).unwrap();
        for (id, name, path) in [
            ("project-a", "Project A", &workspace_a),
            ("project-b", "Project B", &workspace_b),
        ] {
            storage
                .save_project(ProjectRecord {
                    id: id.to_string(),
                    name: name.to_string(),
                    path: Some(path.to_string_lossy().into_owned()),
                    created_at: 1,
                    pinned_at: None,
                })
                .unwrap();
        }
        storage
            .save_conversation(ChatConversationRecord {
                id: "conversation-project-boundary".to_string(),
                project_id: Some("project-a".to_string()),
                model_id: Some("model-1".to_string()),
                title: "Project-bound conversation".to_string(),
                messages: vec![ChatMessageRecord {
                    id: "user-existing-project-a".to_string(),
                    role: "user".to_string(),
                    content: "History from project A.".to_string(),
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

        let skills = Arc::new(SkillsService::new());
        let descriptor = skills
            .list_workspace("project-b", &workspace_b)
            .unwrap()
            .skills()[0]
            .clone();
        let selection = mycopilot_protocol_rs::SkillSelectionDto {
            id: descriptor.id().as_str().to_string(),
            revision: descriptor.revision().as_str().to_string(),
        };
        let turn_error = match prepare_conversation_turn(
            &storage,
            &skills,
            skill_turn_input(
                "project-b",
                selection.clone(),
                "conversation-project-boundary",
            ),
            "run-project-boundary",
        ) {
            Ok(_) => panic!("an ordinary turn must not migrate an existing conversation"),
            Err(error) => error,
        };
        assert!(turn_error.message().contains("不能迁移会话项目"));
        assert!(turn_error.skill_activation().is_none());
        let unchanged = storage
            .load_conversation("conversation-project-boundary")
            .unwrap()
            .unwrap();
        assert_eq!(unchanged.project_id.as_deref(), Some("project-a"));
        assert_eq!(unchanged.messages.len(), 1);

        let service = AgentService::new(Arc::clone(&storage)).with_skills_service(skills);
        let preview_error = service
            .get_context_window_snapshot(AgentContextWindowSnapshotInput {
                conversation_id: Some("conversation-project-boundary".to_string()),
                project_id: Some("project-b".to_string()),
                model_id: "model-1".to_string(),
                max_tokens: Some(30_000),
                prompt_preferences: None,
                permissions: AgentPermissions::default(),
                skills: vec![selection],
            })
            .unwrap_err();
        assert!(preview_error.message().contains("不能迁移会话项目"));
    }

    #[test]
    fn conversation_turn_and_pending_restore_use_the_model_connection_override() {
        let fixture = tempdir().unwrap();
        let storage =
            Arc::new(StorageService::open(&fixture.path().join("storage.sqlite")).unwrap());
        let mut settings = test_model_settings();
        settings.api_url.clear();
        settings.api_token.clear();
        settings.models[0].api_url_override = Some("https://model.example/v1".to_string());
        settings.models[0].api_token_override = Some("model-token".to_string());
        storage.save_model_settings(settings).unwrap();

        let prepared = prepare_conversation_turn(
            &storage,
            &SkillsService::new(),
            AgentConversationTurnInput {
                conversation_id: Some("conversation-model-override".to_string()),
                project_id: None,
                model_id: "model-1".to_string(),
                context_window_indicator_enabled: true,
                content: "Hello".to_string(),
                attachments: Vec::new(),
                skills: Vec::new(),
                title: None,
                user_message_id: Some("user-model-override".to_string()),
                assistant_message_id: Some("assistant-model-override".to_string()),
                max_tokens: None,
                temperature: None,
                prompt_preferences: None,
                permissions: AgentPermissions::default(),
            },
            "run-model-override",
        )
        .unwrap();

        assert_eq!(prepared.agent_input.api_url, "https://model.example/v1");
        assert_eq!(prepared.agent_input.api_token, "model-token");

        let mut persisted_input = prepared.agent_input;
        persisted_input.api_url = "https://stale.example/v1".to_string();
        persisted_input.api_token.clear();
        let restored = restore_agent_input_secrets(&storage, persisted_input);
        assert_eq!(restored.api_url, "https://model.example/v1");
        assert_eq!(restored.api_token, "model-token");
    }

    fn test_context_compaction_generator() -> ContextCompactionSummaryGenerator {
        Arc::new(|request, cancellation| {
            Box::pin(async move {
                cancellation.check()?;
                let observation = mycopilot_core::ModelRequestObservation {
                    schema_version: mycopilot_core::MODEL_REQUEST_OBSERVATION_SCHEMA_VERSION,
                    id: format!("model-request-{}", request.operation_id),
                    run_id: request.run_id.clone(),
                    conversation_id: Some(request.conversation_id.clone()),
                    assistant_message_id: Some(request.assistant_message_id.clone()),
                    operation_id: Some(request.operation_id.clone()),
                    request_index: request.request_index,
                    purpose: mycopilot_core::ModelRequestPurpose::ContextCompaction,
                    model: "model-1".to_string(),
                    api_style: mycopilot_core::AgentApiStyle::OpenAiCompatible,
                    status: mycopilot_core::ModelRequestObservationStatus::Completed,
                    estimate: None,
                    actual_usage: None,
                    finish_reason: Some("stop".to_string()),
                    error_code: None,
                    error_message: None,
                    started_at: 1,
                    completed_at: 2,
                };
                Ok(AgentContextCompactionGenerationOutput {
                    draft: ContextCompactionSummaryDraft {
                        id: format!(
                            "test-summary-{}",
                            ID_COUNTER.fetch_add(1, Ordering::Relaxed)
                        ),
                        source_revision: request.prefix.source_revision.clone(),
                        content: "Test summary of the completed historical turn.".to_string(),
                        continuity: request.continuity,
                        generation: ContextCompactionGeneration::test(),
                        source_input_tokens: request.source_input_tokens,
                        summary_input_tokens: 10,
                        continuity_input_tokens: 10,
                        replacement_input_tokens: 20,
                        created_at: now_ms(),
                    },
                    observation,
                })
            })
        })
    }

    #[test]
    fn production_compaction_services_install_the_current_model_generator() {
        let fixture = tempdir().unwrap();
        let storage =
            Arc::new(StorageService::open(&fixture.path().join("storage.sqlite")).unwrap());
        let service = AgentService::new(storage);
        let agent_input = serde_json::from_value::<AgentChatInput>(json!({
            "apiUrl": "https://example.test/v1/chat/completions",
            "apiToken": "secret",
            "model": "model-1",
            "apiStyle": "open_ai_compatible",
            "contextWindowTokens": 128000,
            "maxTokens": 4000,
            "messages": []
        }))
        .unwrap();
        let (notifications, _receiver) = tokio::sync::mpsc::unbounded_channel();

        let _services = service.context_compaction_services(
            "run-production-generator",
            "conversation-production-generator",
            "assistant-production-generator",
            agent_input,
            notifications,
        );
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
            &SkillsService::new(),
            AgentConversationTurnInput {
                conversation_id: Some("conversation-history".to_string()),
                project_id: None,
                model_id: "model-1".to_string(),
                context_window_indicator_enabled: true,
                content: "What changed?".to_string(),
                attachments: Vec::new(),
                skills: Vec::new(),
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
        assert_eq!(prepared.agent_input.messages[0].created_at, Some(1));
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
        assert_eq!(prepared.agent_input.messages[1].created_at, Some(2));
        assert_eq!(prepared.agent_input.messages[2].content, "What changed?");
        assert!(prepared.agent_input.messages[2].created_at.is_some());
        let serialized = serde_json::to_string(&prepared.agent_input.messages).unwrap();
        assert!(serialized.contains("src/history.rs"));
        assert!(!serialized.contains("FRONTEND_TIMELINE_MUST_NOT_ENTER_CONTEXT"));
        assert!(!serialized.contains("frontend/fake.rs"));
    }

    #[test]
    fn next_turn_loads_active_summary_and_only_the_uncovered_tail() {
        let fixture = tempdir().unwrap();
        let storage = StorageService::open(&fixture.path().join("storage.sqlite")).unwrap();
        storage.save_model_settings(test_model_settings()).unwrap();
        storage
            .save_conversation(ChatConversationRecord {
                id: "conversation-summary".to_string(),
                project_id: None,
                model_id: Some("model-1".to_string()),
                title: "Summary history".to_string(),
                messages: vec![
                    ChatMessageRecord {
                        id: "user-old".to_string(),
                        role: "user".to_string(),
                        content: "Old request".to_string(),
                        created_at: 1,
                        status: Some("sent".to_string()),
                        attachments: Vec::new(),
                        agent_run_json: None,
                        ui_state_json: None,
                    },
                    ChatMessageRecord {
                        id: "assistant-old".to_string(),
                        role: "assistant".to_string(),
                        content: "Old answer".to_string(),
                        created_at: 2,
                        status: Some("sent".to_string()),
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
        let prefix = storage
            .prepare_context_compaction_prefix(
                "conversation-summary",
                &ContextJournalCursor::message("assistant-old"),
            )
            .unwrap();
        storage
            .commit_context_compaction_prefix(
                &prefix,
                test_compaction_draft(
                    &prefix,
                    "summary-active",
                    "The old request was completed.",
                    100,
                    3,
                ),
                "assistant-old",
            )
            .unwrap();

        let prepared = prepare_conversation_turn(
            &storage,
            &SkillsService::new(),
            AgentConversationTurnInput {
                conversation_id: Some("conversation-summary".to_string()),
                project_id: None,
                model_id: "model-1".to_string(),
                context_window_indicator_enabled: true,
                content: "Continue".to_string(),
                attachments: Vec::new(),
                skills: Vec::new(),
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

        assert_eq!(prepared.agent_input.messages.len(), 1);
        assert_eq!(prepared.agent_input.messages[0].content, "Continue");
        assert_eq!(
            prepared
                .agent_input
                .context_compaction_summary
                .as_ref()
                .map(|summary| summary.id.as_str()),
            Some("summary-active")
        );
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
    fn compaction_projection_hides_covered_prefix_but_keeps_raw_conversation_intact() {
        let conversation = ChatConversationRecord {
            id: "conversation-compacted".to_string(),
            project_id: None,
            model_id: None,
            title: "Compacted".to_string(),
            messages: vec![
                ChatMessageRecord {
                    id: "user-old".to_string(),
                    role: "user".to_string(),
                    content: "old request".to_string(),
                    created_at: 1,
                    status: Some("sent".to_string()),
                    attachments: Vec::new(),
                    agent_run_json: None,
                    ui_state_json: None,
                },
                ChatMessageRecord {
                    id: "assistant-old".to_string(),
                    role: "assistant".to_string(),
                    content: "old answer".to_string(),
                    created_at: 2,
                    status: Some("sent".to_string()),
                    attachments: Vec::new(),
                    agent_run_json: None,
                    ui_state_json: None,
                },
                ChatMessageRecord {
                    id: "user-tail".to_string(),
                    role: "user".to_string(),
                    content: "new request".to_string(),
                    created_at: 3,
                    status: Some("sent".to_string()),
                    attachments: Vec::new(),
                    agent_run_json: None,
                    ui_state_json: None,
                },
            ],
            created_at: 1,
            updated_at: 3,
            pinned_at: None,
            archived_at: None,
            unread_at: None,
        };
        let summary_prefix = ContextCompactionPrefix {
            conversation_id: conversation.id.clone(),
            source_revision: "revision-1".to_string(),
            covered_through: ContextJournalCursor::message("assistant-old"),
            previous_summary: None,
            source_items: vec![
                ContextCompactionSourceItem::Message {
                    cursor: ContextJournalCursor::message("user-old"),
                    role: "user".to_string(),
                    content: "old request".to_string(),
                    created_at: 1,
                    status: Some("sent".to_string()),
                    terminal_status: None,
                    terminal_error: None,
                },
                ContextCompactionSourceItem::Message {
                    cursor: ContextJournalCursor::message("assistant-old"),
                    role: "assistant".to_string(),
                    content: "old answer".to_string(),
                    created_at: 2,
                    status: Some("sent".to_string()),
                    terminal_status: None,
                    terminal_error: None,
                },
            ],
        };
        let summary = ContextCompactionSummary {
            schema_version: CONTEXT_COMPACTION_SUMMARY_SCHEMA_VERSION,
            id: "summary-1".to_string(),
            conversation_id: conversation.id.clone(),
            source_revision: summary_prefix.source_revision.clone(),
            previous_summary_id: None,
            covered_through: summary_prefix.covered_through.clone(),
            content: "old turn summary".to_string(),
            continuity: mycopilot_core::ContextContinuitySnapshot::from_prefix(&summary_prefix)
                .unwrap(),
            generation: ContextCompactionGeneration::test(),
            source_input_tokens: 100,
            summary_input_tokens: 10,
            continuity_input_tokens: 10,
            replacement_input_tokens: 20,
            created_at: 4,
        };

        let projected =
            conversation_history_messages_with_compaction(&conversation, &[], Some(&summary), &[]);

        assert_eq!(conversation.messages.len(), 3);
        assert_eq!(projected.len(), 1);
        assert_eq!(projected[0].content, "new request");
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
                    api_url_override: None,
                    api_token_override: None,
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
                skills: Vec::new(),
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
    fn cached_context_preview_measures_skill_without_polluting_durable_revision() {
        let fixture = tempdir().unwrap();
        let workspace = fixture.path().join("workspace");
        write_test_skill(&workspace, "SKILL_PREVIEW_MARKER: verify the repository.");
        let storage =
            Arc::new(StorageService::open(&fixture.path().join("storage.sqlite")).unwrap());
        storage.save_model_settings(test_model_settings()).unwrap();
        storage
            .save_project(ProjectRecord {
                id: "project-preview-skill".to_string(),
                name: "Preview workspace".to_string(),
                path: Some(workspace.to_string_lossy().into_owned()),
                created_at: 1,
                pinned_at: None,
            })
            .unwrap();
        storage
            .save_conversation(ChatConversationRecord {
                id: "conversation-preview-skill".to_string(),
                project_id: Some("project-preview-skill".to_string()),
                model_id: Some("model-1".to_string()),
                title: "Preview".to_string(),
                messages: vec![ChatMessageRecord {
                    id: "user-preview-skill".to_string(),
                    role: "user".to_string(),
                    content: "Review the repository.".to_string(),
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
        let skills = Arc::new(SkillsService::new());
        let catalog = skills
            .list_workspace("project-preview-skill", &workspace)
            .unwrap();
        let descriptor = catalog.skills().first().unwrap();
        let service =
            AgentService::new(Arc::clone(&storage)).with_skills_service(Arc::clone(&skills));
        let base_input = AgentContextWindowSnapshotInput {
            conversation_id: Some("conversation-preview-skill".to_string()),
            project_id: Some("project-preview-skill".to_string()),
            model_id: "model-1".to_string(),
            max_tokens: Some(30_000),
            prompt_preferences: None,
            permissions: AgentPermissions::default(),
            skills: Vec::new(),
        };
        let plain = service
            .get_context_window_snapshot(base_input.clone())
            .unwrap()
            .snapshot
            .unwrap();
        let selected = service
            .get_context_window_snapshot(AgentContextWindowSnapshotInput {
                skills: vec![mycopilot_protocol_rs::SkillSelectionDto {
                    id: descriptor.id().as_str().to_string(),
                    revision: descriptor.revision().as_str().to_string(),
                }],
                ..base_input
            })
            .unwrap()
            .snapshot
            .unwrap();

        assert_eq!(selected.persistent_revision, plain.persistent_revision);
        assert!(selected.run_transient_input_tokens > 0);
        assert!(selected.request_input_tokens > plain.request_input_tokens);
    }

    #[test]
    fn committed_test_summary_rebuilds_the_shared_durable_snapshot() {
        let fixture = tempdir().unwrap();
        let storage =
            Arc::new(StorageService::open(&fixture.path().join("storage.sqlite")).unwrap());
        storage.save_model_settings(test_model_settings()).unwrap();
        storage
            .save_conversation(ChatConversationRecord {
                id: "conversation-capacity-summary".to_string(),
                project_id: None,
                model_id: Some("model-1".to_string()),
                title: "Capacity summary".to_string(),
                messages: vec![
                    ChatMessageRecord {
                        id: "user-long".to_string(),
                        role: "user".to_string(),
                        content: "u".repeat(12_000),
                        created_at: 1,
                        status: Some("sent".to_string()),
                        attachments: Vec::new(),
                        agent_run_json: None,
                        ui_state_json: None,
                    },
                    ChatMessageRecord {
                        id: "assistant-long".to_string(),
                        role: "assistant".to_string(),
                        content: "a".repeat(12_000),
                        created_at: 2,
                        status: Some("sent".to_string()),
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
        let snapshot_input = AgentContextWindowSnapshotInput {
            conversation_id: Some("conversation-capacity-summary".to_string()),
            project_id: None,
            model_id: "model-1".to_string(),
            max_tokens: Some(30_000),
            prompt_preferences: None,
            permissions: AgentPermissions::default(),
            skills: Vec::new(),
        };
        let before = service
            .get_context_window_snapshot(snapshot_input.clone())
            .unwrap()
            .snapshot
            .unwrap();

        let prefix = storage
            .prepare_context_compaction_prefix(
                "conversation-capacity-summary",
                &ContextJournalCursor::message("assistant-long"),
            )
            .unwrap();
        storage
            .commit_context_compaction_prefix(
                &prefix,
                test_compaction_draft(
                    &prefix,
                    "summary-capacity",
                    "The prior request was completed.",
                    before.durable_input_tokens,
                    3,
                ),
                "assistant-long",
            )
            .unwrap();
        service.invalidate_conversation_context_state("conversation-capacity-summary");

        let after = service
            .get_context_window_snapshot(snapshot_input)
            .unwrap()
            .snapshot
            .unwrap();
        assert!(after.durable_input_tokens < before.durable_input_tokens);
        assert_eq!(
            storage
                .load_conversation("conversation-capacity-summary")
                .unwrap()
                .unwrap()
                .messages
                .len(),
            2
        );
    }

    #[tokio::test]
    async fn compaction_host_prepares_generates_commits_and_rebuilds_running_state() {
        let fixture = tempdir().unwrap();
        let storage =
            Arc::new(StorageService::open(&fixture.path().join("storage.sqlite")).unwrap());
        storage.save_model_settings(test_model_settings()).unwrap();
        storage
            .save_conversation(ChatConversationRecord {
                id: "conversation-compaction-host".to_string(),
                project_id: None,
                model_id: Some("model-1".to_string()),
                title: "Compaction host".to_string(),
                messages: vec![
                    ChatMessageRecord {
                        id: "user-old".to_string(),
                        role: "user".to_string(),
                        content: "An old request with substantial detail.".repeat(200),
                        created_at: 1,
                        status: Some("sent".to_string()),
                        attachments: Vec::new(),
                        agent_run_json: None,
                        ui_state_json: None,
                    },
                    ChatMessageRecord {
                        id: "assistant-old".to_string(),
                        role: "assistant".to_string(),
                        content: "The old request was completed with detailed results.".repeat(200),
                        created_at: 2,
                        status: Some("sent".to_string()),
                        attachments: Vec::new(),
                        agent_run_json: None,
                        ui_state_json: None,
                    },
                    ChatMessageRecord {
                        id: "user-current".to_string(),
                        role: "user".to_string(),
                        content: "Continue the work.".to_string(),
                        created_at: 3,
                        status: Some("sent".to_string()),
                        attachments: Vec::new(),
                        agent_run_json: None,
                        ui_state_json: None,
                    },
                    ChatMessageRecord {
                        id: "assistant-current".to_string(),
                        role: "assistant".to_string(),
                        content: THINKING_PLACEHOLDER.to_string(),
                        created_at: 4,
                        status: Some("pending".to_string()),
                        attachments: Vec::new(),
                        agent_run_json: None,
                        ui_state_json: None,
                    },
                ],
                created_at: 1,
                updated_at: 4,
                pinned_at: None,
                archived_at: None,
                unread_at: None,
            })
            .unwrap();
        storage
            .append_in_progress_conversation_turn_trace(
                &ConversationTurnTrace {
                    schema_version: CONVERSATION_TURN_TRACE_SCHEMA_VERSION,
                    run_id: "run-compaction-host".to_string(),
                    conversation_id: "conversation-compaction-host".to_string(),
                    assistant_message_id: "assistant-current".to_string(),
                    terminal_status: ConversationTurnTraceTerminalStatus::InProgress,
                    terminal_error: None,
                    truncated: false,
                    items: Vec::new(),
                },
                4,
                4,
            )
            .unwrap();
        let service = AgentService::new(storage.clone())
            .with_context_compaction_summary_generator(test_context_compaction_generator());
        let agent_input = serde_json::from_value::<AgentChatInput>(json!({
            "apiUrl": "https://example.test/v1/chat/completions",
            "apiToken": "secret",
            "model": "model-1",
            "contextWindowTokens": 16000,
            "contextWindowIndicatorEnabled": true,
            "maxTokens": 1000,
            "assistantMessageId": "assistant-current",
            "skillActivation": {
                "activationRevision": "skill-activation-sha256-v1:compaction",
                "skills": [{
                    "id": "workspace:project:compaction-review",
                    "name": "compaction-review",
                    "revision": "skill-package-sha256-v1:compaction",
                    "source": "workspace:project",
                    "instructions": "SKILL_COMPACTION_OVERLAY_MARKER"
                }]
            },
            "context": {
                "conversationId": "conversation-compaction-host"
            },
            "messages": []
        }))
        .unwrap();
        let (notifications, mut receiver) = tokio::sync::mpsc::unbounded_channel();
        let services = service.context_compaction_services(
            "run-compaction-host",
            "conversation-compaction-host",
            "assistant-current",
            agent_input,
            notifications,
        );
        let cancellation = AgentCancellationToken::new();
        let prepare_request = mycopilot_core::AgentContextCompactionPrepareRequest {
            run_id: "run-compaction-host".to_string(),
            conversation_id: "conversation-compaction-host".to_string(),
            assistant_message_id: "assistant-current".to_string(),
            expected_previous_summary_id: None,
            covered_through: ContextJournalCursor::message("assistant-old"),
            visible_trace_item_count: 0,
            source_input_tokens: 5_000,
            target_replacement_tokens: 750,
        };
        let receipt_plan = mycopilot_core::ContextCompactionReceiptPlan {
            context_revision: "0000000000000001".to_string(),
            persistent_revision: "0000000000000001".to_string(),
            request_input_tokens: 6_000,
            available_input_tokens: Some(6_000),
            request_trigger_input_tokens: Some(5_000),
            request_target_input_tokens: Some(1_000),
            request_pressure: true,
            durable_input_tokens: 5_000,
            durable_capacity_tokens: Some(6_000),
            durable_trigger_input_tokens: Some(5_000),
            durable_target_input_tokens: Some(750),
            durable_pressure: true,
            source_input_tokens: 5_000,
            target_replacement_tokens: 750,
            expected_reclaimed_tokens: 4_250,
            planned_reclaimed_tokens: 4_250,
            projected_request_input_tokens: 1_750,
            projected_durable_input_tokens: 750,
            best_effort: false,
            protected_input_tokens: 0,
            protected_reasons: std::collections::BTreeMap::new(),
            atomic_unit_count: 2,
            previous_summary_id: None,
            covered_through: prepare_request.covered_through.clone(),
        };
        let planned_receipt = mycopilot_core::ContextCompactionReceipt {
            schema_version: mycopilot_core::CONTEXT_COMPACTION_RECEIPT_SCHEMA_VERSION,
            operation_id: "operation-compaction-host".to_string(),
            run_id: prepare_request.run_id.clone(),
            conversation_id: prepare_request.conversation_id.clone(),
            assistant_message_id: prepare_request.assistant_message_id.clone(),
            request_index: 1,
            attempt_index: 1,
            model: "model-1".to_string(),
            api_style: mycopilot_core::AgentApiStyle::OpenAiCompatible,
            status: mycopilot_core::ContextCompactionReceiptStatus::InProgress,
            stage: mycopilot_core::ContextCompactionReceiptStage::Planned,
            plan: receipt_plan.clone(),
            source_revision: None,
            generation_observation_id: None,
            summary_id: None,
            result: None,
            error: None,
            started_at: 1,
            updated_at: 1,
            completed_at: None,
        };
        services
            .record_receipt(planned_receipt, None)
            .await
            .unwrap();
        let prefix = match services
            .prepare(prepare_request.clone(), cancellation.clone())
            .await
            .unwrap()
        {
            AgentContextCompactionPrepareOutcome::Ready(prefix) => prefix,
            AgentContextCompactionPrepareOutcome::Refresh(_) => {
                panic!("fresh plan unexpectedly required a refresh")
            }
        };
        let generated = services
            .generate(
                AgentContextCompactionGenerationRequest {
                    operation_id: "operation-compaction-host".to_string(),
                    run_id: prepare_request.run_id.clone(),
                    conversation_id: prepare_request.conversation_id.clone(),
                    assistant_message_id: prepare_request.assistant_message_id.clone(),
                    request_index: 1,
                    prefix: prefix.clone(),
                    continuity: mycopilot_core::ContextContinuitySnapshot::from_prefix(&prefix)
                        .unwrap(),
                    source_input_tokens: prepare_request.source_input_tokens,
                    target_replacement_tokens: prepare_request.target_replacement_tokens,
                },
                cancellation.clone(),
            )
            .await
            .unwrap();
        let expected_summary_id = generated.draft.id.clone();
        let applied_receipt = mycopilot_core::ContextCompactionReceipt {
            schema_version: mycopilot_core::CONTEXT_COMPACTION_RECEIPT_SCHEMA_VERSION,
            operation_id: "operation-compaction-host".to_string(),
            run_id: prepare_request.run_id.clone(),
            conversation_id: prepare_request.conversation_id.clone(),
            assistant_message_id: prepare_request.assistant_message_id.clone(),
            request_index: 1,
            attempt_index: 1,
            model: "model-1".to_string(),
            api_style: mycopilot_core::AgentApiStyle::OpenAiCompatible,
            status: mycopilot_core::ContextCompactionReceiptStatus::Applied,
            stage: mycopilot_core::ContextCompactionReceiptStage::Completed,
            plan: receipt_plan,
            source_revision: Some(prefix.source_revision.clone()),
            generation_observation_id: Some(generated.observation.id.clone()),
            summary_id: Some(expected_summary_id.clone()),
            result: Some(mycopilot_core::ContextCompactionReceiptResult {
                summary_id: expected_summary_id.clone(),
                source_input_tokens: generated.draft.source_input_tokens,
                summary_input_tokens: generated.draft.summary_input_tokens,
                continuity_input_tokens: generated.draft.continuity_input_tokens,
                replacement_input_tokens: generated.draft.replacement_input_tokens,
                reclaimed_input_tokens: generated
                    .draft
                    .source_input_tokens
                    .saturating_sub(generated.draft.replacement_input_tokens),
            }),
            error: None,
            started_at: 1,
            updated_at: now_ms(),
            completed_at: Some(now_ms()),
        };
        let committed = services
            .commit(
                AgentContextCompactionCommitRequest {
                    run_id: prepare_request.run_id,
                    conversation_id: prepare_request.conversation_id,
                    assistant_message_id: prepare_request.assistant_message_id,
                    visible_trace_item_count: prepare_request.visible_trace_item_count,
                    prefix,
                    draft: generated.draft,
                    receipt: applied_receipt,
                    observation: generated.observation,
                },
                cancellation,
            )
            .await
            .unwrap();
        match committed {
            AgentContextCompactionCommitOutcome::Applied { summary_id, .. } => {
                assert_eq!(summary_id, expected_summary_id)
            }
            AgentContextCompactionCommitOutcome::Refresh(_) => {
                panic!("fresh summary unexpectedly became stale")
            }
        }

        let active = storage
            .get_active_context_compaction_summary("conversation-compaction-host")
            .unwrap()
            .unwrap();
        assert_eq!(active.id, expected_summary_id);
        assert_eq!(
            active.covered_through,
            ContextJournalCursor::message("assistant-old")
        );
        let audit = service
            .get_context_compaction_audit(AgentContextCompactionAuditInput {
                conversation_id: "conversation-compaction-host".to_string(),
                operation_id: Some("operation-compaction-host".to_string()),
                limit: Some(1),
            })
            .unwrap()
            .report;
        assert_eq!(audit.reports.len(), 1);
        assert_eq!(
            audit.reports[0].receipt.status,
            mycopilot_core::ContextCompactionReceiptStatus::Applied
        );
        assert_eq!(
            audit.reports[0]
                .summary
                .as_ref()
                .map(|summary| summary.relation),
            Some(mycopilot_core::ContextCompactionSummaryRelation::Active)
        );
        assert!(audit.reports[0].generation_observation.is_some());
        assert_eq!(audit.estimation_error_groups.len(), 1);
        assert_eq!(
            storage
                .load_conversation("conversation-compaction-host")
                .unwrap()
                .unwrap()
                .messages
                .len(),
            4
        );
        let states = service
            .conversation_context_states
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let state = states.get("conversation-compaction-host").unwrap();
        assert_eq!(state.active_run_id.as_deref(), Some("run-compaction-host"));
        assert!(!state.terminal);
        drop(states);
        let context_event = receiver.try_recv().unwrap();
        assert_eq!(
            context_event["params"]["type"].as_str(),
            Some("context_window_updated")
        );
        assert!(
            context_event["params"]["snapshot"]["runTransientInputTokens"]
                .as_u64()
                .is_some_and(|tokens| tokens > 0)
        );
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
            "skillActivation": {
                "activationRevision": "skill-activation-sha256-v1:live",
                "skills": [{
                    "id": "workspace:project:live-review",
                    "name": "live-review",
                    "revision": "skill-package-sha256-v1:live",
                    "source": "workspace:project",
                    "instructions": "SKILL_LIVE_OVERLAY_MARKER"
                }]
            },
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
        let skill_tokens = initial["params"]["snapshot"]["runTransientInputTokens"]
            .as_u64()
            .unwrap();
        assert!(skill_tokens > 0);

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
        assert_eq!(
            narrated["params"]["snapshot"]["runTransientInputTokens"],
            skill_tokens
        );
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
        assert_eq!(
            closed["params"]["snapshot"]["runTransientInputTokens"],
            skill_tokens
        );

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
    fn terminal_cache_rebuild_drops_the_completed_run_skill_overlay() {
        let fixture = tempdir().unwrap();
        let storage =
            Arc::new(StorageService::open(&fixture.path().join("storage.sqlite")).unwrap());
        storage
            .save_conversation(ChatConversationRecord {
                id: "conversation-terminal-skill".to_string(),
                project_id: None,
                model_id: Some("model-1".to_string()),
                title: "Terminal Skill".to_string(),
                messages: vec![
                    ChatMessageRecord {
                        id: "user-terminal-skill".to_string(),
                        role: "user".to_string(),
                        content: "Review the completed run.".to_string(),
                        created_at: 1,
                        status: Some("sent".to_string()),
                        attachments: Vec::new(),
                        agent_run_json: None,
                        ui_state_json: None,
                    },
                    ChatMessageRecord {
                        id: "assistant-terminal-skill".to_string(),
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
        let trace = completed_conversation_trace_without_items(
            "run-terminal-skill",
            "conversation-terminal-skill",
            "assistant-terminal-skill",
        );
        storage
            .finalize_chat_message_with_conversation_trace(
                "conversation-terminal-skill",
                "assistant-terminal-skill",
                "The review is complete.",
                Some("sent"),
                "completed",
                &trace,
                2,
                3,
            )
            .unwrap();

        let service = AgentService::new(storage);
        let agent_input = serde_json::from_value::<AgentChatInput>(json!({
            "apiUrl": "https://example.test/v1/chat/completions",
            "apiToken": "secret",
            "model": "model-1",
            "contextWindowTokens": 128000,
            "contextWindowIndicatorEnabled": true,
            "maxTokens": 30000,
            "skillActivation": {
                "activationRevision": "skill-activation-sha256-v1:terminal",
                "skills": [{
                    "id": "workspace:project:terminal-review",
                    "name": "terminal-review",
                    "revision": "skill-package-sha256-v1:terminal",
                    "source": "workspace:project",
                    "instructions": "SKILL_TERMINAL_OVERLAY_MARKER"
                }]
            },
            "context": {
                "conversationId": "conversation-terminal-skill"
            },
            "messages": []
        }))
        .unwrap();

        let snapshot = service
            .finalize_conversation_context_state(
                &agent_input,
                "run-terminal-skill",
                "conversation-terminal-skill",
                "assistant-terminal-skill",
                "The review is complete.",
            )
            .unwrap()
            .unwrap();

        assert_eq!(snapshot.run_transient_input_tokens, 0);
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
            version: 2,
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
                    origin: None,
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
                    origin: None,
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
            model_visible_trace_item_count: 0,
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
    fn terminal_pending_action_persistence_redacts_only_skill_instruction_bodies() {
        const MARKER: &str = "PENDING_SKILL_INSTRUCTION_BODY_MUST_NOT_SURVIVE";
        let mut agent_input = serde_json::from_value::<AgentChatInput>(json!({
            "apiUrl": "https://example.test/v1/chat/completions",
            "apiToken": "secret",
            "model": "test-model",
            "messages": []
        }))
        .unwrap();
        agent_input.skill_activation = Some(AgentSkillActivation {
            activation_revision: "activation-revision".to_string(),
            skills: vec![AgentActivatedSkill {
                id: "bundled:application:repository-evidence-auditor".to_string(),
                name: "repository-evidence-auditor".to_string(),
                revision: "package-revision".to_string(),
                source: "bundled:application".to_string(),
                instructions: MARKER.to_string(),
            }],
        });
        agent_input.resume_checkpoint = Some(AgentRunCheckpoint {
            version: 2,
            run_id: "run-skill-redaction".to_string(),
            context_items: vec![
                mycopilot_core::AgentContextCheckpointItem {
                    role: "user".to_string(),
                    content: format!("<backend_activated_skill>{MARKER}</backend_activated_skill>"),
                    images: Vec::new(),
                    tool_call_id: None,
                    tool_calls: Vec::new(),
                    is_error: false,
                    sources: vec!["skill_instructions".to_string()],
                    scope: "run".to_string(),
                    retention: "retained".to_string(),
                    group: None,
                    origin: Some(mycopilot_core::AgentContextCheckpointOrigin {
                        kind: "skill".to_string(),
                        id: "bundled:application:repository-evidence-auditor".to_string(),
                    }),
                },
                mycopilot_core::AgentContextCheckpointItem {
                    role: "system".to_string(),
                    content: "NON_SKILL_CHECKPOINT_CONTENT".to_string(),
                    images: Vec::new(),
                    tool_call_id: None,
                    tool_calls: Vec::new(),
                    is_error: false,
                    sources: vec!["runtime_guard".to_string()],
                    scope: "run".to_string(),
                    retention: "retained".to_string(),
                    group: None,
                    origin: None,
                },
            ],
            next_model_request_index: 1,
            queued_tool_calls: Vec::new(),
            suppressed_narration: false,
            extension_snapshots: Vec::new(),
            pending_tool_call_id: "action-skill-redaction".to_string(),
            conversation_trace_items: Vec::new(),
            next_conversation_trace_sequence: 0,
            conversation_trace_truncated: false,
            model_visible_trace_item_count: 0,
        });
        let mut record = PendingActionRecord {
            snapshot: PendingAgentActionSnapshot {
                action_id: "action-skill-redaction".to_string(),
                action_type: "tool_call".to_string(),
                tool_name: "approval_tool".to_string(),
                tool_call_id: Some("action-skill-redaction".to_string()),
                run_id: "run-skill-redaction".to_string(),
                conversation_id: Some("conversation-skill-redaction".to_string()),
                assistant_message_id: Some("assistant-skill-redaction".to_string()),
                action: AgentProposedAction::ToolCall {
                    call: AgentToolCall {
                        id: "action-skill-redaction".to_string(),
                        tool: "approval_tool".to_string(),
                        args: json!({}),
                        approval_status: AgentApprovalStatus::Required,
                        reason: None,
                    },
                },
                created_at: 1,
                status: PendingActionStatus::Pending,
            },
            agent_input,
        };

        for status in [PendingActionStatus::Pending, PendingActionStatus::Approved] {
            record.snapshot.status = status;
            let persisted = pending_storage_record(&record, 2);
            assert!(persisted.agent_input_json.contains(MARKER));
        }

        for status in [
            PendingActionStatus::Completed,
            PendingActionStatus::Rejected,
            PendingActionStatus::Cancelled,
            PendingActionStatus::Failed,
        ] {
            record.snapshot.status = status;
            let persisted = pending_storage_record(&record, 3);
            assert!(!persisted.agent_input_json.contains(MARKER));
            assert!(!persisted.agent_input_json.contains("secret"));
            let restored: AgentChatInput =
                serde_json::from_str(&persisted.agent_input_json).unwrap();
            let activation = restored.skill_activation.unwrap();
            assert_eq!(activation.activation_revision, "activation-revision");
            assert_eq!(activation.skills.len(), 1);
            assert_eq!(
                activation.skills[0].id,
                "bundled:application:repository-evidence-auditor"
            );
            assert_eq!(activation.skills[0].revision, "package-revision");
            assert_eq!(activation.skills[0].source, "bundled:application");
            assert!(activation.skills[0].instructions.is_empty());
            let checkpoint = restored.resume_checkpoint.unwrap();
            assert!(checkpoint.context_items[0].content.is_empty());
            assert_eq!(
                checkpoint.context_items[0].sources,
                vec!["skill_instructions"]
            );
            assert_eq!(
                checkpoint.context_items[0].origin.as_ref().unwrap().id,
                "bundled:application:repository-evidence-auditor"
            );
            assert_eq!(
                checkpoint.context_items[1].content,
                "NON_SKILL_CHECKPOINT_CONTENT"
            );
        }

        record.snapshot.status = PendingActionStatus::Pending;
        assert!(pending_storage_record(&record, 4)
            .agent_input_json
            .contains(MARKER));
    }

    #[test]
    fn missing_pending_transition_row_fails_closed_without_terminal_success() {
        const MARKER: &str = "MISSING_TRANSITION_SKILL_BODY";
        let fixture = tempdir().unwrap();
        let storage =
            Arc::new(StorageService::open(&fixture.path().join("storage.sqlite")).unwrap());
        let service = AgentService::new(Arc::clone(&storage));
        let mut agent_input = serde_json::from_value::<AgentChatInput>(json!({
            "apiUrl": "https://example.test/v1/chat/completions",
            "apiToken": "secret",
            "model": "test-model",
            "messages": []
        }))
        .unwrap();
        agent_input.skill_activation = Some(AgentSkillActivation {
            activation_revision: "activation-missing-row".to_string(),
            skills: vec![AgentActivatedSkill {
                id: "bundled:application/repository-evidence-auditor".to_string(),
                name: "repository-evidence-auditor".to_string(),
                revision: "package-missing-row".to_string(),
                source: "bundled:application".to_string(),
                instructions: MARKER.to_string(),
            }],
        });
        let call = AgentToolCall {
            id: "action-missing-transition-row".to_string(),
            tool: "approval_tool".to_string(),
            args: json!({}),
            approval_status: AgentApprovalStatus::Required,
            reason: None,
        };
        service.store_pending_action(
            "run-missing-transition-row",
            "conversation-missing-transition-row",
            "assistant-missing-transition-row",
            AgentProposedAction::ToolCall { call: call.clone() },
            agent_input,
        );
        assert!(storage.list_pending_agent_actions().unwrap()[0]
            .agent_input_json
            .contains(MARKER));

        // Reproduce a cross-boundary missing-row race: durable conversation deletion has removed
        // the pending row while this service instance still owns its pre-deletion memory snapshot.
        storage
            .delete_conversation("conversation-missing-transition-row")
            .unwrap();
        let (notifications, mut receiver) = tokio::sync::mpsc::unbounded_channel();
        let error = service.approve_action(&call.id, notifications).unwrap_err();
        assert!(error.contains("实际更新 0 条"));
        assert!(receiver.try_recv().is_err());
        assert_eq!(
            service
                .pending_actions
                .lock()
                .unwrap_or_else(|lock_error| lock_error.into_inner())[&call.id]
                .snapshot
                .status,
            PendingActionStatus::Pending
        );

        let reloaded = AgentService::new(storage);
        assert!(reloaded.list_pending_actions().is_empty());
    }

    #[test]
    fn cancel_finalize_failure_atomically_restores_pending_payload() {
        const MARKER: &str = "CANCEL_ROLLBACK_SKILL_BODY";
        let fixture = tempdir().unwrap();
        let storage =
            Arc::new(StorageService::open(&fixture.path().join("storage.sqlite")).unwrap());
        let service = AgentService::new(Arc::clone(&storage));
        let call = AgentToolCall {
            id: "action-cancel-rollback".to_string(),
            tool: "approval_tool".to_string(),
            args: json!({}),
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
        agent_input.skill_activation = Some(AgentSkillActivation {
            activation_revision: "activation-cancel-rollback".to_string(),
            skills: vec![AgentActivatedSkill {
                id: "bundled:application/repository-evidence-auditor".to_string(),
                name: "repository-evidence-auditor".to_string(),
                revision: "package-cancel-rollback".to_string(),
                source: "bundled:application".to_string(),
                instructions: MARKER.to_string(),
            }],
        });
        agent_input.resume_checkpoint = Some(AgentRunCheckpoint {
            version: 2,
            run_id: "run-cancel-rollback".to_string(),
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
                operation: json!({}),
                approval_status: AgentApprovalStatus::Required,
                truncated: false,
            }],
            next_conversation_trace_sequence: 1,
            conversation_trace_truncated: false,
            model_visible_trace_item_count: 0,
        });
        // These durable owner rows intentionally do not exist, forcing final trace persistence to
        // fail after the action has first transitioned to cancelled.
        service.store_pending_action(
            "run-cancel-rollback",
            "conversation-cancel-rollback-missing",
            "assistant-cancel-rollback-missing",
            AgentProposedAction::ToolCall { call: call.clone() },
            agent_input,
        );

        assert!(service.cancel_action(&call.id).is_err());
        assert_eq!(
            service.list_pending_actions()[0].status,
            PendingActionStatus::Pending
        );
        let persisted = storage.list_pending_agent_actions().unwrap();
        assert_eq!(persisted.len(), 1);
        assert_eq!(persisted[0].status, "pending");
        assert!(persisted[0].agent_input_json.contains(MARKER));
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
            version: 2,
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
            model_visible_trace_item_count: 0,
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
            version: 2,
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
            model_visible_trace_item_count: 0,
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
            version: 2,
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
            model_visible_trace_item_count: 0,
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
