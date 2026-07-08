// Support types and helper functions for core-server agent orchestration.
use crate::agent::{AGENT_EVENT_NAME, ID_COUNTER, THINKING_PLACEHOLDER};
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::time::{SystemTime, UNIX_EPOCH};

use mycopilot_core::command::AgentCommandExecutionResult;
use mycopilot_core::patch::apply_unified_diff_in_workspace;
use mycopilot_core::storage::models::{
    AgentPromptPreferencesRecord, ChatConversationRecord, ChatMessageAttachmentRecord,
    ChatMessageRecord, ModelConfigRecord, ProjectRecord,
};
use mycopilot_core::storage::service::StorageService;
use mycopilot_core::{
    AgentApprovalDecisionStatus, AgentChatInput, AgentChatMessage, AgentChatOutput,
    AgentCommandRequest, AgentDiffProposal, AgentEvent, AgentInputAttachment,
    AgentInputAttachmentEncoding, AgentInputAttachmentKind, AgentPatchResult,
    AgentPatchResultStatus, AgentPermissions, AgentPromptDetailLevel, AgentPromptPreferences,
    AgentPromptTone, AgentPromptWorkMode, AgentProposedAction, AgentRunContext, AgentRunMode,
    AgentRunStatus, AgentSearchConfig, AgentSearchMode, AgentToolCall, AgentToolResult, AgentUsage,
    AgentWorkspaceContext,
};
use mycopilot_protocol_rs::AGENT_EVENT_NOTIFICATION_METHOD;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

#[derive(Clone)]
pub(super) struct PendingActionRecord {
    pub(super) snapshot: PendingAgentActionSnapshot,
    pub(super) agent_input: AgentChatInput,
}

#[derive(Debug, Clone)]
pub(super) struct AgentRunUsageContext {
    pub(super) conversation_id: String,
    pub(super) assistant_message_id: String,
    pub(super) run_id: String,
    pub(super) project_id: Option<String>,
    pub(super) model_id: String,
    pub(super) model_name: String,
    pub(super) provider_path: Option<String>,
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
    pub(super) tool_result: AgentToolResult,
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

pub(super) fn pending_status_label(status: PendingActionStatus) -> &'static str {
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

pub(super) struct PreparedConversationTurn {
    pub(super) output: AgentConversationTurnOutput,
    pub(super) agent_input: AgentChatInput,
    pub(super) usage_context: AgentRunUsageContext,
}

pub(super) fn prepare_conversation_turn(
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
    if !model.supports_image
        && input
            .attachments
            .iter()
            .any(|attachment| attachment.kind == AgentInputAttachmentKind::Image)
    {
        return Err(format!(
            "当前模型「{}」不支持图片输入，请切换支持图片的模型后再发送。",
            model.display_name
        ));
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

pub(super) fn model_provider_path(model: &ModelConfigRecord) -> String {
    model
        .provider_path
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(model.id.as_str())
        .to_string()
}

pub(super) fn normalized_optional(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
}

pub(super) fn non_empty(value: String) -> Option<String> {
    let value = value.trim().to_string();
    if value.is_empty() || value == "tvly-my-copilot-search-key" {
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
        AgentProposedAction::Command { command } => command.id.clone(),
    }
}

pub(super) fn action_type_for_action(action: &AgentProposedAction) -> &'static str {
    match action {
        AgentProposedAction::ToolCall { .. } => "tool_call",
        AgentProposedAction::Diff { .. } => "diff",
        AgentProposedAction::Command { .. } => "command",
    }
}

pub(super) fn tool_name_for_action(action: &AgentProposedAction) -> String {
    match action {
        AgentProposedAction::ToolCall { call } => call.tool.clone(),
        AgentProposedAction::Diff { .. } => "apply_patch".to_string(),
        AgentProposedAction::Command { .. } => "run_command".to_string(),
    }
}

pub(super) fn tool_call_for_action(action: &AgentProposedAction) -> AgentToolCall {
    match action {
        AgentProposedAction::ToolCall { call } => call.clone(),
        AgentProposedAction::Diff { diff } => diff_tool_call(diff),
        AgentProposedAction::Command { command } => command_tool_call(command),
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

pub(super) fn action_execution_for_decision(
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

pub(super) fn rejected_action_execution(
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
        tool_result,
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
