use super::*;
use serde::Serializer;

#[derive(Clone)]
pub(crate) struct PendingActionRecord {
    /// Backend-owned durable identity. Provider tool-call ids are only unique within a run.
    pub(crate) storage_id: String,
    pub(crate) snapshot: PendingAgentActionSnapshot,
    pub(crate) agent_input: AgentChatInput,
}

pub(crate) fn agent_input_belongs_to_project(
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
pub(crate) struct AgentRunUsageContext {
    pub(crate) conversation_id: String,
    pub(crate) assistant_message_id: String,
    pub(crate) run_id: String,
    pub(crate) project_id: Option<String>,
    pub(crate) model_id: String,
    pub(crate) model_name: String,
    pub(crate) input_price: Option<String>,
    pub(crate) output_price: Option<String>,
    pub(crate) started_at: i64,
}

#[derive(Debug, Clone)]
pub(crate) struct AgentRunUsageState {
    pub(crate) context: AgentRunUsageContext,
    pub(crate) usage: Option<AgentUsage>,
    pub(crate) status: AgentRunStatus,
    pub(crate) error: Option<String>,
}

pub(crate) struct ActionExecutionDecision {
    pub(crate) status: String,
    pub(crate) final_pending_status: PendingActionStatus,
    pub(crate) patch_result: Option<AgentPatchResult>,
    pub(crate) file_write_result: Option<AgentFileWriteResult>,
    pub(crate) file_change: Option<AgentTurnFileChange>,
    pub(crate) tool_result: AgentToolResult,
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

pub(crate) fn pending_status_label(status: PendingActionStatus) -> &'static str {
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
    #[serde(serialize_with = "serialize_renderer_safe_action")]
    pub action: AgentProposedAction,
    pub created_at: i64,
    pub status: PendingActionStatus,
}

fn serialize_renderer_safe_action<S>(
    action: &AgentProposedAction,
    serializer: S,
) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    let mut value = serde_json::to_value(action).map_err(serde::ser::Error::custom)?;
    redact_renderer_mcp_binding_fields(&mut value);
    value.serialize(serializer)
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
    #[serde(serialize_with = "serialize_renderer_safe_agent_output")]
    pub agent_output: AgentChatOutput,
}

fn serialize_renderer_safe_agent_output<S>(
    output: &AgentChatOutput,
    serializer: S,
) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    let mut value = serde_json::to_value(output).map_err(serde::ser::Error::custom)?;
    redact_renderer_mcp_binding_fields(&mut value);
    value.serialize(serializer)
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
