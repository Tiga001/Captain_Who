// Support types and helper functions for core-server agent orchestration.
use crate::agent::{AGENT_EVENT_NAME, ID_COUNTER, THINKING_PLACEHOLDER};
use crate::skills_adapter::{activate_selected_skills, SkillActivationFailure};
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::time::{SystemTime, UNIX_EPOCH};

use mycopilot_core::command::{AgentCommandExecutionResult, CommandPolicyEvaluation};
use mycopilot_core::file_write::{apply_file_write, failed_file_write_result};
use mycopilot_core::patch::apply_unified_diff_in_workspace;
use mycopilot_core::skills::SkillsService;
use mycopilot_core::storage::models::{
    AgentPromptPreferencesRecord, ChatConversationRecord, ChatMessageAttachmentRecord,
    ChatMessageRecord, ProjectRecord,
};
use mycopilot_core::storage::service::StorageService;
use mycopilot_core::{
    AgentApprovalDecisionStatus, AgentChatInput, AgentChatMessage, AgentChatOutput,
    AgentCommandRequest, AgentContextWindowSnapshot, AgentDiffProposal, AgentEvent,
    AgentFileDraftSnapshot, AgentFileWriteProposal, AgentFileWriteResult,
    AgentFileWriteResultStatus, AgentInputAttachment, AgentInputAttachmentEncoding,
    AgentInputAttachmentKind, AgentPatchResult, AgentPatchResultStatus, AgentPermissions,
    AgentPromptDetailLevel, AgentPromptPreferences, AgentPromptTone, AgentPromptWorkMode,
    AgentProposedAction, AgentRunContext, AgentRunStatus, AgentSearchConfig, AgentSearchMode,
    AgentToolCall, AgentToolResult, AgentUsage, AgentWorkspaceContext,
    ContextCompactionAuditBundle, ContextJournalCursor, ConversationTurnTrace,
    ConversationTurnTraceTerminalStatus,
};
use mycopilot_protocol_rs::{
    ActivatedSkillSummaryDto, SkillActivationErrorData, SkillSelectionDto,
    AGENT_EVENT_NOTIFICATION_METHOD,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

#[derive(Clone)]
pub(super) struct PendingActionRecord {
    /// Backend-owned durable identity. Provider tool-call ids are only unique within a run.
    pub(super) storage_id: String,
    pub(super) snapshot: PendingAgentActionSnapshot,
    pub(super) agent_input: AgentChatInput,
}

pub(super) fn agent_input_belongs_to_project(
    agent_input: &AgentChatInput,
    project_id: &str,
) -> bool {
    agent_input
        .context
        .as_ref()
        .and_then(|context| context.project_id.as_deref())
        == Some(project_id)
}

#[derive(Debug, Clone)]
pub(super) struct AgentRunUsageContext {
    pub(super) conversation_id: String,
    pub(super) assistant_message_id: String,
    pub(super) run_id: String,
    pub(super) project_id: Option<String>,
    pub(super) model_id: String,
    pub(super) model_name: String,
    pub(super) input_price: Option<String>,
    pub(super) output_price: Option<String>,
    pub(super) started_at: i64,
}

#[derive(Debug, Clone)]
pub(super) struct AgentRunUsageState {
    pub(super) context: AgentRunUsageContext,
    pub(super) usage: Option<AgentUsage>,
    pub(super) status: AgentRunStatus,
    pub(super) error: Option<String>,
}

pub(super) struct ActionExecutionDecision {
    pub(super) status: String,
    pub(super) final_pending_status: PendingActionStatus,
    pub(super) patch_result: Option<AgentPatchResult>,
    pub(super) file_write_result: Option<AgentFileWriteResult>,
    pub(super) tool_result: AgentToolResult,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PendingActionStatus {
    Pending,
    Approved,
    Executing,
    Rejected,
    Cancelled,
    Completed,
    Failed,
}

pub(super) fn pending_status_label(status: PendingActionStatus) -> &'static str {
    match status {
        PendingActionStatus::Pending => "pending",
        PendingActionStatus::Approved => "approved",
        PendingActionStatus::Executing => "executing",
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
    pub file_write_result: Option<AgentFileWriteResult>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub command_result: Option<AgentCommandExecutionResult>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_result: Option<AgentToolResult>,
    pub agent_output: AgentChatOutput,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentFileDraftContentPage {
    pub draft: AgentFileDraftSnapshot,
    pub content: String,
    pub offset: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_offset: Option<usize>,
    pub truncated: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentFileWriteDiffPage {
    pub draft_id: String,
    pub patch: String,
    pub offset: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_offset: Option<usize>,
    pub truncated: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentConversationTurnInput {
    pub conversation_id: Option<String>,
    pub project_id: Option<String>,
    pub model_id: String,
    #[serde(default = "context_window_indicator_enabled_by_default")]
    pub context_window_indicator_enabled: bool,
    pub content: String,
    #[serde(default)]
    pub attachments: Vec<AgentInputAttachment>,
    #[serde(default)]
    pub skills: Vec<SkillSelectionDto>,
    pub title: Option<String>,
    pub user_message_id: Option<String>,
    pub assistant_message_id: Option<String>,
    pub max_tokens: Option<u32>,
    pub temperature: Option<f32>,
    pub prompt_preferences: Option<AgentPromptPreferences>,
    #[serde(default)]
    pub permissions: AgentPermissions,
}

fn context_window_indicator_enabled_by_default() -> bool {
    true
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentContextWindowSnapshotInput {
    pub conversation_id: Option<String>,
    pub project_id: Option<String>,
    pub model_id: String,
    pub max_tokens: Option<u32>,
    pub prompt_preferences: Option<AgentPromptPreferences>,
    #[serde(default)]
    pub permissions: AgentPermissions,
    #[serde(default)]
    pub skills: Vec<SkillSelectionDto>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentContextWindowSnapshotOutput {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub snapshot: Option<AgentContextWindowSnapshot>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentContextCompactionAuditInput {
    pub conversation_id: String,
    pub operation_id: Option<String>,
    pub limit: Option<usize>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentContextCompactionAuditOutput {
    pub report: ContextCompactionAuditBundle,
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
    pub activated_skills: Vec<ActivatedSkillSummaryDto>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub skill_activation_revision: Option<String>,
}

#[derive(Debug)]
pub struct AgentServiceError {
    message: String,
    skill_activation: Option<Box<SkillActivationErrorData>>,
}

impl AgentServiceError {
    pub fn message(&self) -> &str {
        &self.message
    }

    pub fn skill_activation(&self) -> Option<&SkillActivationErrorData> {
        self.skill_activation.as_deref()
    }
}

impl From<String> for AgentServiceError {
    fn from(message: String) -> Self {
        Self {
            message,
            skill_activation: None,
        }
    }
}

impl From<SkillActivationFailure> for AgentServiceError {
    fn from(failure: SkillActivationFailure) -> Self {
        let message = failure.to_string();
        Self {
            message,
            skill_activation: Some(failure.into_data()),
        }
    }
}

impl std::fmt::Display for AgentServiceError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for AgentServiceError {}

pub(super) struct PreparedConversationTurn {
    pub(super) output: AgentConversationTurnOutput,
    pub(super) agent_input: AgentChatInput,
    pub(super) usage_context: AgentRunUsageContext,
}

pub(super) fn prepare_conversation_turn(
    storage: &StorageService,
    skills_service: &SkillsService,
    input: AgentConversationTurnInput,
    run_id: &str,
) -> Result<PreparedConversationTurn, AgentServiceError> {
    let content = input.content.trim().to_string();
    if content.is_empty() {
        return Err("消息内容不能为空。".to_string().into());
    }

    let model_id = input.model_id.trim().to_string();
    if model_id.is_empty() {
        return Err("modelId 不能为空。".to_string().into());
    }

    let settings = storage
        .load_model_settings()?
        .ok_or_else(|| "请先配置模型 API。".to_string())?;

    let model = settings
        .models
        .iter()
        .find(|model| model.id == model_id)
        .cloned()
        .ok_or_else(|| format!("未找到模型配置：{model_id}"))?;
    if !model.enabled {
        return Err(format!("模型未启用：{model_id}").into());
    }
    // Resolve the complete pair once and carry it through the run. Model-level credentials
    // take priority; otherwise both values come from global settings. This prevents a URL
    // from one provider from ever being combined with a token from another.
    let connection = settings.effective_connection_for(&model)?;
    if !model.supports_image
        && input
            .attachments
            .iter()
            .any(|attachment| attachment.kind == AgentInputAttachmentKind::Image)
    {
        return Err(format!(
            "当前模型「{}」不支持图片输入，请切换支持图片的模型后再发送。",
            model.display_name
        )
        .into());
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

    let existing = storage.load_conversation(&conversation_id)?;
    let resolved_project_id = resolve_conversation_project_id(
        existing.as_ref(),
        normalized_optional(input.project_id.as_deref()),
    )?;
    let project = resolve_project(storage, resolved_project_id.as_deref())?;
    let workspace_root = project
        .as_ref()
        .and_then(|project| project.path.as_deref())
        .map(PathBuf::from);
    let workspace = project
        .as_ref()
        .zip(workspace_root.as_deref())
        .map(|(project, root)| (project.id.as_str(), root));
    let prepared_skills =
        activate_selected_skills(storage, skills_service, workspace, &input.skills)?;

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

    let history_traces = storage.list_conversation_turn_traces(&conversation_id)?;
    let context_compaction_summary =
        storage.get_active_context_compaction_summary(&conversation_id)?;
    let history_messages = conversation_history_messages_with_compaction(
        &conversation,
        &history_traces,
        context_compaction_summary.as_ref(),
        &[user_message_id.as_str(), assistant_message_id.as_str()],
    );

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
    storage.save_input_attachments(
        &conversation_id,
        &user_message_id,
        resolved_project_id.as_deref(),
        &input.attachments,
        timestamp,
    )?;
    let attachment_library = storage
        .build_attachment_library_context(&conversation_id, resolved_project_id.as_deref())?;

    let mut agent_messages = history_messages;
    agent_messages.push(AgentChatMessage {
        message_id: Some(user_message_id.clone()),
        role: "user".to_string(),
        content,
        created_at: Some(timestamp),
        conversation_turn_trace: None,
    });

    let agent_input = AgentChatInput {
        api_url: connection.api_url,
        api_token: connection.api_token,
        model: model.id.clone(),
        api_style: None,
        context_window_tokens: model.context_window_tokens,
        context_window_indicator_enabled: input.context_window_indicator_enabled,
        max_tokens: input.max_tokens,
        temperature: input.temperature,
        stream: Some(true),
        context: Some(AgentRunContext {
            conversation_id: Some(conversation_id.clone()),
            project_id: resolved_project_id.clone(),
            workspace: project.as_ref().map(|project| AgentWorkspaceContext {
                project_id: Some(project.id.clone()),
                display_name: Some(project.name.clone()),
                root_path: project.path.clone(),
            }),
            attachment_library: Some(attachment_library),
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
        resume_checkpoint: None,
        assistant_message_id: Some(assistant_message_id.clone()),
        context_compaction_summary,
        skill_activation: prepared_skills.runtime,
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
            activated_skills: prepared_skills.summaries,
            skill_activation_revision: prepared_skills.revision,
        },
        agent_input,
    })
}

#[cfg(test)]
pub(super) fn conversation_history_messages(
    conversation: &ChatConversationRecord,
    traces: &[ConversationTurnTrace],
    excluded_message_ids: &[&str],
) -> Vec<AgentChatMessage> {
    conversation_history_messages_with_compaction(conversation, traces, None, excluded_message_ids)
}

pub(super) fn conversation_history_messages_with_compaction(
    conversation: &ChatConversationRecord,
    traces: &[ConversationTurnTrace],
    compaction_summary: Option<&mycopilot_core::ContextCompactionSummary>,
    excluded_message_ids: &[&str],
) -> Vec<AgentChatMessage> {
    let traces = traces
        .iter()
        .map(|trace| (trace.assistant_message_id.as_str(), trace))
        .collect::<std::collections::HashMap<_, _>>();
    let covered_boundary = compaction_summary.and_then(|summary| {
        conversation
            .messages
            .iter()
            .position(|message| message.id == summary.covered_through.message_id())
            .map(|index| (index, &summary.covered_through))
    });
    conversation
        .messages
        .iter()
        .enumerate()
        .filter_map(|(index, message)| {
            let trace = traces.get(message.id.as_str()).copied().cloned();
            let Some((boundary_index, cursor)) = covered_boundary else {
                return Some((message, trace));
            };
            if index < boundary_index {
                return None;
            }
            if index > boundary_index {
                return Some((message, trace));
            }
            match cursor {
                ContextJournalCursor::Message { .. } => None,
                ContextJournalCursor::TraceItem { sequence, .. } => {
                    let mut trace = trace?;
                    trace.items.retain(|item| item.sequence() > *sequence);
                    let has_uncovered_completion = trace.terminal_status.is_terminal();
                    (!trace.items.is_empty() || has_uncovered_completion)
                        .then_some((message, Some(trace)))
                }
            }
        })
        .filter(|message| {
            !excluded_message_ids
                .iter()
                .any(|excluded_id| message.0.id == *excluded_id)
        })
        .filter(|(message, trace)| {
            if message.status.as_deref() != Some("pending") {
                return true;
            }
            trace.as_ref().is_some_and(|trace| {
                trace.terminal_status == ConversationTurnTraceTerminalStatus::InProgress
            })
        })
        .filter(|(message, _)| matches!(message.role.as_str(), "user" | "assistant"))
        .filter_map(|(message, trace)| {
            if message.status.as_deref() == Some("error") && trace.is_none() {
                return None;
            }
            let content = if trace.as_ref().is_some_and(|trace| {
                trace.terminal_status == ConversationTurnTraceTerminalStatus::InProgress
            }) || (trace.as_ref().is_some_and(|trace| {
                trace.terminal_status == ConversationTurnTraceTerminalStatus::Cancelled
            }) && message.content.trim() == THINKING_PLACEHOLDER)
            {
                String::new()
            } else {
                message.content.clone()
            };
            if content.trim().is_empty() && trace.is_none() {
                return None;
            }
            Some(AgentChatMessage {
                message_id: Some(message.id.clone()),
                role: message.role.clone(),
                content,
                created_at: Some(message.created_at),
                conversation_turn_trace: trace,
            })
        })
        .collect()
}

pub fn agent_event_notification(event: AgentEvent) -> Value {
    json!({
        "jsonrpc": "2.0",
        "method": AGENT_EVENT_NOTIFICATION_METHOD,
        "params": event
    })
}

pub(super) fn resolve_project(
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

/// Existing conversation history is bound to the workspace that produced it. A normal turn may
/// omit that project id, but it may not migrate the conversation to another project implicitly.
/// Project migration needs a dedicated operation that can validate and update every dependent
/// artifact atomically.
pub(super) fn resolve_conversation_project_id(
    existing: Option<&ChatConversationRecord>,
    requested_project_id: Option<String>,
) -> Result<Option<String>, String> {
    let Some(existing) = existing else {
        return Ok(requested_project_id);
    };
    let stored_project_id = normalized_optional(existing.project_id.as_deref());
    if requested_project_id.is_some() && requested_project_id != stored_project_id {
        return Err(format!(
            "会话 `{}` 已绑定到另一个项目；普通消息不能迁移会话项目。",
            existing.id
        ));
    }
    Ok(stored_project_id)
}

pub(super) fn message_attachments_from_input(
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

pub(super) fn agent_prompt_preferences_from_record(
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

pub(super) fn input_attachment_kind_label(kind: AgentInputAttachmentKind) -> &'static str {
    match kind {
        AgentInputAttachmentKind::File => "file",
        AgentInputAttachmentKind::Image => "image",
    }
}

pub(super) fn search_mode_from_storage(value: &str) -> AgentSearchMode {
    match value {
        "disabled" => AgentSearchMode::Disabled,
        "tavily" => AgentSearchMode::Tavily,
        _ => AgentSearchMode::Auto,
    }
}

pub(super) fn normalized_optional(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
}

pub(super) fn non_empty(value: String) -> Option<String> {
    let value = value.trim().to_string();
    if value.is_empty() {
        None
    } else {
        Some(value)
    }
}

pub(super) fn upsert_message(messages: &mut Vec<ChatMessageRecord>, next: ChatMessageRecord) {
    if let Some(existing) = messages.iter_mut().find(|message| message.id == next.id) {
        *existing = next;
    } else {
        messages.push(next);
    }
}

pub(super) fn status_for_run(status: AgentRunStatus) -> Option<&'static str> {
    match status {
        AgentRunStatus::Completed | AgentRunStatus::Cancelled => Some("sent"),
        AgentRunStatus::WaitingForApproval | AgentRunStatus::Running | AgentRunStatus::Idle => {
            Some("pending")
        }
        AgentRunStatus::Failed => Some("error"),
    }
}

pub(super) fn run_status_label(status: AgentRunStatus) -> &'static str {
    match status {
        AgentRunStatus::Idle => "idle",
        AgentRunStatus::Running => "running",
        AgentRunStatus::WaitingForApproval => "waiting_for_approval",
        AgentRunStatus::Completed => "completed",
        AgentRunStatus::Failed => "failed",
        AgentRunStatus::Cancelled => "cancelled",
    }
}

pub(super) fn is_terminal_run_status(status: AgentRunStatus) -> bool {
    matches!(
        status,
        AgentRunStatus::Completed | AgentRunStatus::Failed | AgentRunStatus::Cancelled
    )
}

pub(super) fn merge_usage(total: &mut Option<AgentUsage>, next: Option<AgentUsage>) {
    let Some(next) = next else {
        return;
    };
    let total_usage = total.get_or_insert(AgentUsage {
        input_tokens: None,
        output_tokens: None,
        output_thinking_tokens: None,
        total_tokens: None,
        cached_input_tokens: None,
        cache_creation_input_tokens: None,
        billable_request_count: None,
    });

    total_usage.input_tokens = add_optional(total_usage.input_tokens, next.input_tokens);
    total_usage.output_tokens = add_optional(total_usage.output_tokens, next.output_tokens);
    total_usage.output_thinking_tokens = add_optional(
        total_usage.output_thinking_tokens,
        next.output_thinking_tokens,
    );
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
                || next.output_thinking_tokens.is_some()
                || next.total_tokens.is_some())
            .then_some(1)
        }),
    );
}

pub(super) fn add_optional(left: Option<u64>, right: Option<u64>) -> Option<u64> {
    match (left, right) {
        (Some(left), Some(right)) => Some(left.saturating_add(right)),
        (Some(left), None) => Some(left),
        (None, Some(right)) => Some(right),
        (None, None) => None,
    }
}

pub(super) fn action_id_for_action(action: &AgentProposedAction) -> String {
    match action {
        AgentProposedAction::ToolCall { call } => call.id.clone(),
        AgentProposedAction::Diff { diff } => diff.id.clone(),
        AgentProposedAction::FileWrite { file_write } => file_write.id.clone(),
        AgentProposedAction::Command { command } => command.id.clone(),
    }
}

/// Stable backend identity for a provider-scoped action id.
pub(super) fn pending_action_storage_id(run_id: &str, action_id: &str) -> String {
    format!("v2:{}:{run_id}:{action_id}", run_id.len())
}

pub(super) fn action_type_for_action(action: &AgentProposedAction) -> &'static str {
    match action {
        AgentProposedAction::ToolCall { .. } => "tool_call",
        AgentProposedAction::Diff { .. } => "diff",
        AgentProposedAction::FileWrite { .. } => "file_write",
        AgentProposedAction::Command { .. } => "command",
    }
}

pub(super) fn tool_name_for_action(action: &AgentProposedAction) -> String {
    match action {
        AgentProposedAction::ToolCall { call } => call.tool.clone(),
        AgentProposedAction::Diff { .. } => "apply_patch".to_string(),
        AgentProposedAction::FileWrite { .. } => "write_file".to_string(),
        AgentProposedAction::Command { .. } => "run_command".to_string(),
    }
}

pub(super) fn tool_call_for_action(action: &AgentProposedAction) -> AgentToolCall {
    match action {
        AgentProposedAction::ToolCall { call } => call.clone(),
        AgentProposedAction::Diff { diff } => diff_tool_call(diff),
        AgentProposedAction::FileWrite { file_write } => file_write_tool_call(file_write),
        AgentProposedAction::Command { command } => command_tool_call(command),
    }
}

pub(super) fn file_write_tool_call(file_write: &AgentFileWriteProposal) -> AgentToolCall {
    AgentToolCall {
        id: file_write.id.clone(),
        tool: "write_file".to_string(),
        args: json!({
            "phase": "finish",
            "draftId": file_write.draft_id,
            "summary": file_write.summary
        }),
        approval_status: file_write.approval_status,
        reason: file_write.summary.clone(),
    }
}

pub(super) fn diff_tool_call(diff: &AgentDiffProposal) -> AgentToolCall {
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

pub(super) fn command_tool_call(command: &AgentCommandRequest) -> AgentToolCall {
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

pub(super) fn tool_result_for_decision(
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
                "status": "unsupported",
                "message": "Generic approved tool calls cannot be executed. Only structured diff, file-write, and command actions are supported."
            })),
            error: Some("不支持执行通用审批工具调用；未修改文件或运行命令。".to_string()),
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

pub(super) fn action_execution_for_decision(
    storage: &StorageService,
    record: &PendingActionRecord,
    call: &AgentToolCall,
    decision_status: AgentApprovalDecisionStatus,
    message: Option<&str>,
) -> ActionExecutionDecision {
    if decision_status == AgentApprovalDecisionStatus::Rejected {
        return rejected_action_execution(storage, record, call, message);
    }

    match &record.snapshot.action {
        AgentProposedAction::Diff { diff } => approved_patch_execution(record, diff),
        AgentProposedAction::FileWrite { file_write } => {
            approved_file_write_execution(storage, &record.agent_input, file_write)
        }
        AgentProposedAction::ToolCall { call } if call.tool == "apply_patch" => {
            ActionExecutionDecision {
                status: "failed".to_string(),
                final_pending_status: PendingActionStatus::Failed,
                patch_result: None,
                file_write_result: None,
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
            file_write_result: None,
            tool_result: tool_result_for_decision(call, decision_status, message),
        },
    }
}

pub(super) fn rejected_action_execution(
    storage: &StorageService,
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
            file_write_result: None,
            tool_result,
        };
    }

    if let AgentProposedAction::FileWrite { file_write } = &record.snapshot.action {
        if let Ok(Some(mut draft)) = storage.get_agent_file_draft(&file_write.draft_id) {
            draft.status = "rejected".to_string();
            draft.updated_at = now_ms();
            let _ = storage.update_agent_file_draft(&draft);
        }
        let result = failed_file_write_result(
            file_write,
            AgentFileWriteResultStatus::Rejected,
            message.unwrap_or("用户拒绝了文件写入。"),
        );
        return ActionExecutionDecision {
            status: "rejected".to_string(),
            final_pending_status: PendingActionStatus::Rejected,
            patch_result: None,
            file_write_result: Some(result.clone()),
            tool_result: file_write_tool_result(&record.snapshot.action_id, true, &result),
        };
    }

    ActionExecutionDecision {
        status: "rejected".to_string(),
        final_pending_status: PendingActionStatus::Rejected,
        patch_result: None,
        file_write_result: None,
        tool_result: tool_result_for_decision(call, AgentApprovalDecisionStatus::Rejected, message),
    }
}

pub(super) fn approved_patch_execution(
    record: &PendingActionRecord,
    diff: &mycopilot_core::AgentDiffProposal,
) -> ActionExecutionDecision {
    approved_patch_execution_for_input(&record.agent_input, &record.snapshot.action_id, diff)
}

pub(super) fn approved_patch_execution_for_input(
    agent_input: &AgentChatInput,
    action_id: &str,
    diff: &mycopilot_core::AgentDiffProposal,
) -> ActionExecutionDecision {
    let workspace_root = workspace_root_optional(agent_input);
    let permissions = permissions_from_input(agent_input);
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
    let tool_result = patch_tool_result(action_id, applied, &patch_result);
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
        file_write_result: None,
        tool_result,
    }
}

pub(super) fn approved_file_write_execution(
    storage: &StorageService,
    agent_input: &AgentChatInput,
    proposal: &AgentFileWriteProposal,
) -> ActionExecutionDecision {
    let Some(mut draft) = storage
        .get_agent_file_draft(&proposal.draft_id)
        .ok()
        .flatten()
    else {
        let result = failed_file_write_result(
            proposal,
            AgentFileWriteResultStatus::Failed,
            "未找到待应用的文件草稿。",
        );
        return file_write_decision(proposal, result);
    };
    if draft.status != "waiting_approval" {
        let result = failed_file_write_result(
            proposal,
            AgentFileWriteResultStatus::Failed,
            format!(
                "文件草稿当前状态为 {}，不再等待此审批，不能重复执行。",
                draft.status
            ),
        );
        return file_write_decision(proposal, result);
    }
    draft.status = "applying".to_string();
    draft.updated_at = now_ms();
    let _ = storage.update_agent_file_draft(&draft);
    let result = apply_file_write(
        workspace_root_optional(agent_input).as_deref(),
        proposal,
        &draft,
        permissions_from_input(agent_input),
    )
    .unwrap_or_else(|error| {
        let status = if error.contains("发生变化") || error.contains("已出现") {
            AgentFileWriteResultStatus::Conflict
        } else {
            AgentFileWriteResultStatus::Failed
        };
        failed_file_write_result(proposal, status, error)
    });
    draft.status = match result.status {
        AgentFileWriteResultStatus::Applied | AgentFileWriteResultStatus::AlreadyApplied => {
            "applied"
        }
        AgentFileWriteResultStatus::Conflict => "conflict",
        AgentFileWriteResultStatus::Rejected => "rejected",
        AgentFileWriteResultStatus::Failed => "failed",
    }
    .to_string();
    draft.updated_at = now_ms();
    let _ = storage.update_agent_file_draft(&draft);
    file_write_decision(proposal, result)
}

fn file_write_decision(
    proposal: &AgentFileWriteProposal,
    result: AgentFileWriteResult,
) -> ActionExecutionDecision {
    let applied = matches!(
        result.status,
        AgentFileWriteResultStatus::Applied | AgentFileWriteResultStatus::AlreadyApplied
    );
    let conflict = result.status == AgentFileWriteResultStatus::Conflict;
    ActionExecutionDecision {
        status: if applied {
            "applied"
        } else if conflict {
            "conflict"
        } else {
            "failed"
        }
        .to_string(),
        final_pending_status: if applied {
            PendingActionStatus::Completed
        } else {
            PendingActionStatus::Failed
        },
        patch_result: None,
        file_write_result: Some(result.clone()),
        tool_result: file_write_tool_result(&proposal.id, applied, &result),
    }
}

pub(super) fn file_write_tool_result(
    action_id: &str,
    ok: bool,
    result: &AgentFileWriteResult,
) -> AgentToolResult {
    AgentToolResult {
        call_id: action_id.to_string(),
        tool: "write_file".to_string(),
        ok,
        result: Some(json!(result)),
        error: if ok { None } else { result.error.clone() },
    }
}

pub(super) fn patch_status_for_error(error: &str) -> AgentPatchResultStatus {
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

pub(super) fn patch_tool_result(
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

pub(super) fn command_tool_result(
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

pub(super) fn failed_command_result(
    request: &AgentCommandRequest,
    error: String,
    policy_evaluation: Option<CommandPolicyEvaluation>,
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
        policy_evaluation,
    }
}

pub(super) fn cancelled_command_result(
    request: &AgentCommandRequest,
) -> AgentCommandExecutionResult {
    AgentCommandExecutionResult {
        command: request.command.clone(),
        cwd: request.cwd.clone().unwrap_or_else(|| ".".to_string()),
        exit_code: None,
        stdout: String::new(),
        stderr: String::new(),
        timed_out: false,
        cancelled: true,
        duration_ms: 0,
        stdout_truncated: false,
        stderr_truncated: false,
        error: None,
        policy_evaluation: None,
    }
}

pub(super) fn workspace_root_optional(input: &AgentChatInput) -> Option<PathBuf> {
    input
        .context
        .as_ref()
        .and_then(|context| context.workspace.as_ref())
        .and_then(|workspace| workspace.root_path.as_ref())
        .map(PathBuf::from)
}

pub(super) fn permissions_from_input(input: &AgentChatInput) -> AgentPermissions {
    input
        .context
        .as_ref()
        .map(|context| context.permissions)
        .unwrap_or_default()
}

pub(super) fn create_conversation_title(message: &str) -> String {
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

pub(super) fn create_id(prefix: &str) -> String {
    let counter = ID_COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("{prefix}-{}-{counter}", now_ms())
}

pub(super) fn safe_path_component(value: &str, fallback: &str) -> String {
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

pub(super) fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or(0)
}

pub(super) fn serialize_json<T: Serialize>(value: &T) -> String {
    serde_json::to_string(value).unwrap_or_else(|error| {
        json!({
            "serializationError": error.to_string()
        })
        .to_string()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn failed_command_tool_result_keeps_the_complete_execution_observation() {
        let execution = AgentCommandExecutionResult {
            command: "python3 -c 'import openpyxl'".to_string(),
            cwd: ".".to_string(),
            exit_code: Some(1),
            stdout: "dependency check started".to_string(),
            stderr: "ModuleNotFoundError: No module named 'openpyxl'".to_string(),
            timed_out: false,
            cancelled: false,
            duration_ms: 25,
            stdout_truncated: false,
            stderr_truncated: true,
            error: None,
            policy_evaluation: None,
        };

        let result = command_tool_result("command-1", false, &execution);
        let observation = result.result.expect("structured command observation");

        assert!(!result.ok);
        assert_eq!(result.error.as_deref(), Some("命令执行失败。"));
        assert_eq!(observation["exitCode"], 1);
        assert_eq!(observation["stdout"], "dependency check started");
        assert_eq!(
            observation["stderr"],
            "ModuleNotFoundError: No module named 'openpyxl'"
        );
        assert_eq!(observation["timedOut"], false);
        assert_eq!(observation["cancelled"], false);
        assert_eq!(observation["stdoutTruncated"], false);
        assert_eq!(observation["stderrTruncated"], true);
    }

    #[test]
    fn policy_rejection_keeps_stable_structured_diagnostics_in_tool_result() {
        use mycopilot_core::command::{
            evaluate_command_policy, CommandAuthorizationSource, CommandPolicyDecision,
        };
        use mycopilot_core::AgentCommandSafetyPolicy;

        let request = AgentCommandRequest {
            id: "command-policy-1".to_string(),
            command: "rm -rf /".to_string(),
            cwd: None,
            timeout_ms: Some(5_000),
            approval_status: mycopilot_core::AgentApprovalStatus::Approved,
            risk_level: None,
            reason: None,
        };
        let evaluation = evaluate_command_policy(
            &request.command,
            AgentCommandSafetyPolicy::FullAccess,
            CommandAuthorizationSource::Automatic,
        );
        assert_eq!(evaluation.decision, CommandPolicyDecision::Deny);
        let execution = failed_command_result(
            &request,
            "命令已被安全策略拒绝。".to_string(),
            Some(evaluation),
        );

        let result = command_tool_result(&request.id, false, &execution);
        let observation = result.result.expect("structured policy observation");

        assert_eq!(observation["policyEvaluation"]["decision"], "deny");
        assert_eq!(
            observation["policyEvaluation"]["code"],
            "command.catastrophic.filesystem_root"
        );
        assert_eq!(
            observation["policyEvaluation"]["findings"][0]["risk"],
            "catastrophic"
        );
    }
}
