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

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct AgentRunUsageContext {
    pub(crate) conversation_id: String,
    pub(crate) assistant_message_id: String,
    pub(crate) run_id: String,
    pub(crate) project_id: Option<String>,
    pub(crate) model_id: String,
    pub(crate) model_name: String,
    pub(crate) provider_usage_semantics: ProviderUsageSemantics,
    pub(crate) input_price: Option<String>,
    pub(crate) cached_input_price: Option<String>,
    pub(crate) output_price: Option<String>,
    pub(crate) started_at: i64,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct AgentRunUsageState {
    pub(crate) context: AgentRunUsageContext,
    pub(crate) usage: Option<AgentUsage>,
    pub(crate) status: AgentRunStatus,
    pub(crate) error: Option<String>,
}

pub(crate) struct ActionExecutionDecision {
    pub(crate) status: String,
    pub(crate) final_pending_status: PendingActionStatus,
    pub(crate) file_change_result: Option<AgentFileChangeResult>,
    pub(crate) file_change: Option<AgentTurnFileChange>,
    pub(crate) committed_file_change_action: Option<AgentProposedAction>,
    pub(crate) direct_file_change_finalization: Option<DirectFileChangeFinalization>,
    pub(crate) tool_result: AgentToolResult,
}

pub(crate) struct DirectFileChangeFinalization {
    pub(crate) target: mycopilot_core::file_change::ResolvedFileChangeTarget,
    pub(crate) journal: mycopilot_core::file_change::FileChangeDeleteJournal,
    pub(crate) finalized_at: u64,
}

impl DirectFileChangeFinalization {
    pub(crate) fn finalize(
        mut self,
    ) -> Result<mycopilot_core::file_change::FileChangeDeleteJournal, String> {
        mycopilot_core::file_change::FileChangeCommitter
            .finalize_delete(&self.target, &mut self.journal, self.finalized_at)
            .map_err(|error| error.to_string())?;
        Ok(self.journal)
    }
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
    pub file_change_result: Option<AgentFileChangeResult>,
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
pub struct AgentFileChangeContentPage {
    pub file_change: AgentFileChangeSnapshot,
    pub content: String,
    pub offset: usize,
    pub next_offset: Option<usize>,
    pub truncated: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentFileChangeDiffPage {
    pub transaction_id: String,
    pub patch: String,
    pub offset: usize,
    pub next_offset: Option<usize>,
    pub truncated: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentFileChangeHistoryDiffPage {
    pub conversation_id: String,
    pub assistant_message_id: String,
    pub run_id: String,
    pub tool_call_id: String,
    pub patch: String,
    pub offset: usize,
    pub next_offset: Option<usize>,
    pub truncated: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentConversationTurnInput {
    pub conversation_id: Option<String>,
    pub project_id: Option<String>,
    pub model_id: String,
    #[serde(default = "context_window_indicator_enabled_by_default")]
    pub context_window_indicator_enabled: bool,
    pub content: String,
    #[serde(default)]
    pub attachments: Vec<AgentInputAttachment>,
    /// Host-issued folder references. Selected roots receive read access; writes continue to
    /// follow the run's global permission. Contents are resolved lazily by Core tools.
    #[serde(default)]
    pub folder_references: Vec<mycopilot_core::AgentFolderReference>,
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

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentConversationTurnRewriteInput {
    pub request_id: String,
    pub source_user_message_id: String,
    pub source_assistant_message_id: String,
    pub turn: AgentConversationTurnInput,
}

fn context_window_indicator_enabled_by_default() -> bool {
    true
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentProviderTransitionPreflightInput {
    pub conversation_id: String,
    pub target_model_id: String,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AgentProviderTransitionDecision {
    Compatible,
    RequiresCompaction,
    Blocked,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AgentProviderTransitionReason {
    SameProtocol,
    NoIncompatibleHistory,
    ApiProviderChanged,
    ProviderProtocolChanged,
    ActiveRun,
    PendingApproval,
    UnsupportedTarget,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AgentProviderTransitionPreflightOutput {
    pub conversation_id: String,
    pub target_model_id: String,
    pub decision: AgentProviderTransitionDecision,
    pub reason: AgentProviderTransitionReason,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub operation_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub transition_token: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentProviderTransitionStartInput {
    pub conversation_id: String,
    pub target_model_id: String,
    pub transition_token: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentProviderTransitionGetStatusInput {
    pub conversation_id: String,
    pub operation_id: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AgentProviderTransitionOperationStatus {
    Running,
    Completed,
    Failed,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AgentProviderTransitionRecovery {
    Retry,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AgentProviderTransitionOperationError {
    pub code: String,
    pub message: String,
    pub recovery: AgentProviderTransitionRecovery,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AgentProviderTransitionOperation {
    pub schema_version: u32,
    pub operation_id: String,
    pub conversation_id: String,
    pub target_model_id: String,
    /// Immutable presentation snapshots only; never authoritative model identities.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_model_display_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target_model_display_name: Option<String>,
    pub status: AgentProviderTransitionOperationStatus,
    pub started_at: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub completed_at: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub conversation_updated_at: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub covered_through_message_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<AgentProviderTransitionOperationError>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AgentProviderTransitionGetStatusOutput {
    pub operations: Vec<AgentProviderTransitionOperation>,
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
    pub model_config_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub snapshot: Option<AgentContextWindowSnapshot>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
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
    data: Option<Value>,
}

impl AgentServiceError {
    pub fn message(&self) -> &str {
        &self.message
    }

    pub fn skill_activation(&self) -> Option<&SkillActivationErrorData> {
        self.skill_activation.as_deref()
    }

    pub fn data(&self) -> Option<&Value> {
        self.data.as_ref()
    }

    pub(crate) fn structured(message: impl Into<String>, data: Value) -> Self {
        Self {
            message: message.into(),
            skill_activation: None,
            data: Some(data),
        }
    }
}

impl From<String> for AgentServiceError {
    fn from(message: String) -> Self {
        Self {
            message,
            skill_activation: None,
            data: None,
        }
    }
}

impl From<SkillActivationFailure> for AgentServiceError {
    fn from(failure: SkillActivationFailure) -> Self {
        let message = failure.to_string();
        Self {
            message,
            skill_activation: Some(failure.into_data()),
            data: None,
        }
    }
}

impl std::fmt::Display for AgentServiceError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for AgentServiceError {}
