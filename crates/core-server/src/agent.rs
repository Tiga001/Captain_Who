use crate::agent_support::*;
pub use crate::agent_support::{
    AgentActionExecutionOutput, AgentConversationTurnInput, AgentConversationTurnOutput,
    AgentFileDraftContentPage, AgentFileWriteDiffPage, PendingActionStatus,
    PendingAgentActionSnapshot,
};
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::atomic::AtomicU64;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use mycopilot_core::command::{run_approved_command, AgentCommandExecutionResult, CommandRunState};
use mycopilot_core::file_write::{file_draft_snapshot, file_write_diff};
use mycopilot_core::storage::models::{
    AgentActionAuditRecord, AgentPendingActionRecord, AgentUsageRecordInsert,
};
use mycopilot_core::storage::service::StorageService;
use mycopilot_core::{
    next_run_id, send_chat_with_host_executor, AgentApprovalDecision, AgentApprovalDecisionStatus,
    AgentApprovalStatus, AgentCancellationToken, AgentChatInput, AgentChatOutput, AgentError,
    AgentEvent, AgentEventEmitter, AgentHostActionExecutor, AgentPatchResult, AgentProposedAction,
    AgentResult, AgentRunStatus, AgentToolCall, AgentToolContinuation, AgentToolResult, AgentUsage,
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

            let host_executor = service.host_action_executor(
                pending_agent_input.clone(),
                worker_run_id.clone(),
                Some(worker_conversation_id.clone()),
                Some(worker_assistant_message_id.clone()),
            );
            let result = send_chat_with_host_executor(
                prepared.agent_input,
                worker_run_id.clone(),
                emitter,
                cancellation_token,
                host_executor,
                service.storage.clone(),
            )
            .await;

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
                    let _ = service.persist_final_assistant_output(
                        &worker_conversation_id,
                        &worker_assistant_message_id,
                        &agent_output,
                    );
                }
                Err(error) => {
                    let usage = error.usage().cloned();
                    let message = error.to_string();
                    let _ = service.persist_assistant_error(
                        &worker_conversation_id,
                        &worker_assistant_message_id,
                        &message,
                        usage.clone(),
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
                        usage,
                        finish_reason: None,
                        proposed_actions: Vec::new(),
                    }));
                }
            }

            drop(deleting_projects);
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
        let completed_at = now_ms();
        for context in contexts {
            let _ = self.storage.update_chat_message_run_terminal_state(
                &context.conversation_id,
                &context.assistant_message_id,
                status_for_run(AgentRunStatus::Cancelled),
                run_status_label(AgentRunStatus::Cancelled),
                completed_at,
            );
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
                if cancellation_token.is_cancelled()
                    || agent_input_project_id(&agent_input)
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

        let (agent_input, final_pending_status) = {
            let deleting_projects = self
                .deleting_projects
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            if agent_input_project_id(&record.agent_input)
                .is_some_and(|project_id| deleting_projects.contains(project_id))
            {
                self.discard_usage_context(&run_id);
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
            (agent_input, final_pending_status)
        };

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
        if self.is_agent_input_project_deleting(&record.agent_input) {
            self.discard_usage_context(&run_id);
            return;
        }
        let cancellation_token = AgentCancellationToken::new();
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

        let host_executor = self.host_action_executor(
            agent_input.clone(),
            run_id.clone(),
            record.snapshot.conversation_id.clone(),
            record.snapshot.assistant_message_id.clone(),
        );
        let result = send_chat_with_host_executor(
            agent_input,
            run_id.clone(),
            emitter,
            cancellation_token,
            host_executor,
            self.storage.clone(),
        )
        .await;

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
                let usage = error.usage().cloned();
                let message = error.to_string();
                if let (Some(conversation_id), Some(assistant_message_id)) = (
                    record.snapshot.conversation_id.as_deref(),
                    record.snapshot.assistant_message_id.as_deref(),
                ) {
                    let _ = self.persist_assistant_error(
                        conversation_id,
                        assistant_message_id,
                        &message,
                        usage.clone(),
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
                    usage,
                    finish_reason: None,
                    proposed_actions: Vec::new(),
                }));
                self.update_pending_status(&record.snapshot.action_id, PendingActionStatus::Failed);
            }
        }

        drop(deleting_projects);
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

    fn discard_usage_context(&self, run_id: &str) {
        let mut contexts = self
            .usage_contexts
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        contexts.remove(run_id);
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
        self.persist_run_usage(
            &output.run_id,
            output.status,
            output.usage.clone(),
            output.finish_reason.clone(),
        )?;
        let completed_at = now_ms();
        if output.status == AgentRunStatus::Cancelled {
            return self.storage.update_chat_message_run_terminal_state(
                conversation_id,
                assistant_message_id,
                status_for_run(output.status),
                run_status_label(output.status),
                completed_at,
            );
        }
        self.storage.update_chat_message_status_and_content(
            conversation_id,
            assistant_message_id,
            &output.content,
            status_for_run(output.status),
            completed_at,
        )?;
        if matches!(
            output.status,
            AgentRunStatus::Completed | AgentRunStatus::Failed
        ) {
            self.storage.update_chat_message_run_terminal_state(
                conversation_id,
                assistant_message_id,
                status_for_run(output.status),
                run_status_label(output.status),
                completed_at,
            )?;
        }
        Ok(())
    }

    fn persist_assistant_error(
        &self,
        conversation_id: &str,
        assistant_message_id: &str,
        message: &str,
        usage: Option<AgentUsage>,
    ) -> Result<(), String> {
        if let Some(run_id) = self.find_usage_run_id(conversation_id, assistant_message_id) {
            self.persist_run_usage(
                &run_id,
                AgentRunStatus::Failed,
                usage,
                Some(message.to_string()),
            )?;
        }
        let completed_at = now_ms();
        self.storage.update_chat_message_status_and_content(
            conversation_id,
            assistant_message_id,
            message,
            Some("error"),
            completed_at,
        )?;
        self.storage.update_chat_message_run_terminal_state(
            conversation_id,
            assistant_message_id,
            Some("error"),
            run_status_label(AgentRunStatus::Failed),
            completed_at,
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
    use mycopilot_core::storage::models::ChatConversationRecord;
    use mycopilot_core::AgentUsageSummaryRange;
    use tempfile::tempdir;

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
}
