use super::*;

#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct AgentChatMessage {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message_id: Option<String>,
    pub role: String,
    pub content: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_at: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conversation_turn_trace: Option<ConversationTurnTrace>,
    /// Backend-only uncompressed model projection for this assistant turn. Renderer clients never
    /// author this field; Core validates it against the durable trace before use.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub conversation_model_context_items: Vec<ConversationModelContextItem>,
    /// Host-only compaction projection: this assistant's final message/terminal is already covered.
    #[serde(default, skip_serializing_if = "conversation_completion_not_covered")]
    pub conversation_completion_covered: bool,
}

fn conversation_completion_not_covered(value: &bool) -> bool {
    !*value
}

#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct AgentToolContinuation {
    pub call: AgentToolCall,
    pub result: AgentToolResult,
}

#[derive(Debug, Deserialize, Serialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AgentInputAttachmentKind {
    File,
    Image,
}

#[derive(Debug, Deserialize, Serialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AgentInputAttachmentEncoding {
    Utf8,
    Base64,
    /// Opaque, Host-issued reference to a completed local attachment import.
    Managed,
}

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AttachmentImportInput {
    pub id: String,
    pub kind: AgentInputAttachmentKind,
    pub name: String,
    pub mime_type: Option<String>,
    pub size_bytes: u64,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AttachmentInputPreview {
    pub mime_type: String,
    pub data: String,
}

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AgentInputAttachment {
    pub id: String,
    pub kind: AgentInputAttachmentKind,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mime_type: Option<String>,
    pub size_bytes: u64,
    pub encoding: AgentInputAttachmentEncoding,
    pub data: String,
    /// Streamed content identity assigned by the managed attachment importer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_sha256: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub truncated: Option<bool>,
}

#[derive(Debug, Deserialize, Serialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AgentGuidanceStatus {
    Queued,
    Applied,
    Rejected,
    Abandoned,
}

impl AgentGuidanceStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Applied => "applied",
            Self::Rejected => "rejected",
            Self::Abandoned => "abandoned",
        }
    }

    pub(crate) fn from_str(value: &str) -> Option<Self> {
        match value {
            "queued" => Some(Self::Queued),
            "applied" => Some(Self::Applied),
            "rejected" => Some(Self::Rejected),
            "abandoned" => Some(Self::Abandoned),
            _ => None,
        }
    }
}

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AgentSteerRunInput {
    pub conversation_id: String,
    pub expected_run_id: String,
    pub client_message_id: String,
    pub content: String,
    #[serde(default)]
    pub attachments: Vec<AgentInputAttachment>,
    /// Host-issued folder references carried with queued guidance. Selected roots receive read
    /// access; writes continue to follow the run's global permission.
    #[serde(default)]
    pub folder_references: Vec<crate::AgentFolderReference>,
}

#[derive(Debug, Deserialize, Serialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AgentSteerRunResultStatus {
    Queued,
    Applied,
    Rejected,
}

#[derive(Debug, Deserialize, Serialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AgentSteerRunRejectionCode {
    RunNotSteerable,
    RunInterrupted,
    ConversationMismatch,
    IdentityConflict,
    AttachmentsNotSupported,
    ModelDoesNotSupportAttachments,
    AttachmentValidationFailed,
    AttachmentLimitExceeded,
    AttachmentPersistenceFailed,
}

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AgentSteerRunOutput {
    pub guidance_id: String,
    pub status: AgentSteerRunResultStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rejection_code: Option<AgentSteerRunRejectionCode>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

/// Validated, run-scoped input consumed by the agent loop at a safe sampling boundary.
///
/// The Host owns durable admission and attachment persistence. The runtime only receives inputs
/// that have already been associated with the expected active run.
#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AgentSteerInput {
    pub guidance_id: String,
    pub client_message_id: String,
    pub content: String,
    #[serde(default)]
    pub attachments: Vec<AgentInputAttachment>,
    /// Host-issued folder references carried with queued guidance. Selected roots receive read
    /// access; writes continue to follow the run's global permission.
    #[serde(default)]
    pub folder_references: Vec<crate::AgentFolderReference>,
    /// Host-authoritative attachment library including this guidance's persisted attachments.
    ///
    /// The runtime installs this snapshot only when the guidance is applied at a safe model
    /// boundary. Merely admitting an RPC must never expand the active tool context.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attachment_library: Option<AgentAttachmentLibraryContext>,
    pub created_at: i64,
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct AgentChatOutput {
    pub content: String,
    pub status: AgentRunStatus,
    pub run_id: String,
    pub events: Vec<AgentEvent>,
    pub tool_definitions: Vec<AgentToolDefinition>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub todo: Option<AgentTodoState>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage: Option<AgentUsage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub finish_reason: Option<String>,
    pub proposed_actions: Vec<AgentProposedAction>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub conversation_turn_trace: Option<ConversationTurnTrace>,
}

#[derive(Debug, Deserialize, Serialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AgentRunStatus {
    Idle,
    Running,
    WaitingForApproval,
    WaitingForUserInput,
    Completed,
    Failed,
    Cancelled,
}

#[derive(Debug, Deserialize, Serialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AgentApiStyle {
    OpenAiCompatible,
    AnthropicCompatible,
}

#[derive(Debug, Deserialize, Serialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AgentSearchMode {
    Auto,
    Disabled,
    Tavily,
}

/// Controls which local paths read-only tools may inspect.
///
/// `WorkspaceOnly` restricts file reads and searches to the selected workspace plus registered
/// attachment paths. `All` allows absolute local paths and supported system aliases such as
/// `@home`, `@desktop`, `@documents`, and `@downloads`.
#[derive(Debug, Deserialize, Serialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AgentReadPermission {
    WorkspaceOnly,
    All,
}

/// Controls where file-changing tools may write.
///
/// `Denied` blocks every file-changing call at runtime and host boundaries. Stable file-edit
/// tools remain in the model-visible tool prefix so changing the composer permission does not
/// invalidate that prefix; visibility never grants write authority. Dynamic capabilities may
/// still be omitted when they have no permitted operation. `WorkspaceOnly` allows safe writes
/// only inside the selected workspace. `All` also allows safe writes outside the workspace
/// through absolute paths or supported system aliases.
#[derive(Debug, Deserialize, Serialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AgentWritePermission {
    Denied,
    WorkspaceOnly,
    All,
}

/// Controls whether approved command proposals require a human click.
///
/// Approval and command safety are separate inputs. In guarded mode, `AutoApprove` applies only to
/// commands the policy permits automatically; high-impact commands may still require an explicit
/// user approval. Structural validation, cwd checks, timeout, cancellation, and always-denied
/// operations apply to every path.
#[derive(Debug, Deserialize, Serialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AgentCommandPermission {
    RequireApproval,
    AutoApprove,
}

/// Controls which command safety policy applies after a command has been authorized.
///
/// `Guarded` limits automatic execution and routes high-impact commands to explicit approval.
/// `FullAccess` permits automatic high-impact commands except operations classified as always
/// denied. This is intentionally independent from [`AgentCommandPermission`], which expresses the
/// user's normal approval preference.
#[derive(Debug, Deserialize, Serialize, Clone, Copy, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum AgentCommandSafetyPolicy {
    #[default]
    Guarded,
    FullAccess,
}

/// Controls whether structured file-write proposals require a human click.
///
/// `AutoApprove` only skips the approval prompt. It still runs through the same safe patch
/// or document executor, write scope checks, path checks, and revision conflict checks as manual
/// approval. This policy covers every tool registered in the file-write permission domain,
/// including Office document writers.
#[derive(Debug, Deserialize, Serialize, Clone, Copy, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum AgentPatchPermission {
    #[default]
    RequireApproval,
    AutoApprove,
}

/// Controls whether Host-authenticated built-in Skill and capability execution requires a human
/// click. `AutoApprove` skips only the prompt; every independent scope, manifest, revision, digest,
/// path, and runtime safety check remains authoritative.
#[derive(Debug, Deserialize, Serialize, Clone, Copy, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum AgentBuiltinExecutionPermission {
    #[default]
    RequireApproval,
    AutoApprove,
}

#[derive(Debug, Deserialize, Serialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentPermissions {
    pub read: AgentReadPermission,
    pub write: AgentWritePermission,
    pub command: AgentCommandPermission,
    pub command_safety: AgentCommandSafetyPolicy,
    pub patch: AgentPatchPermission,
    pub builtin_execution: AgentBuiltinExecutionPermission,
}

impl Default for AgentPermissions {
    fn default() -> Self {
        Self {
            read: AgentReadPermission::WorkspaceOnly,
            write: AgentWritePermission::Denied,
            command: AgentCommandPermission::RequireApproval,
            command_safety: AgentCommandSafetyPolicy::Guarded,
            patch: AgentPatchPermission::RequireApproval,
            builtin_execution: AgentBuiltinExecutionPermission::RequireApproval,
        }
    }
}

impl AgentPermissions {
    /// Returns the component-wise intersection of two permission ceilings.
    ///
    /// This is deliberately a meet operation rather than an override: a child, template, project,
    /// or dynamic policy can only retain or remove authority already present in the other input.
    pub fn meet(self, ceiling: Self) -> Self {
        Self {
            read: match (self.read, ceiling.read) {
                (AgentReadPermission::All, AgentReadPermission::All) => AgentReadPermission::All,
                _ => AgentReadPermission::WorkspaceOnly,
            },
            write: match (self.write, ceiling.write) {
                (AgentWritePermission::All, AgentWritePermission::All) => AgentWritePermission::All,
                (AgentWritePermission::Denied, _) | (_, AgentWritePermission::Denied) => {
                    AgentWritePermission::Denied
                }
                _ => AgentWritePermission::WorkspaceOnly,
            },
            command: match (self.command, ceiling.command) {
                (AgentCommandPermission::AutoApprove, AgentCommandPermission::AutoApprove) => {
                    AgentCommandPermission::AutoApprove
                }
                _ => AgentCommandPermission::RequireApproval,
            },
            command_safety: match (self.command_safety, ceiling.command_safety) {
                (AgentCommandSafetyPolicy::FullAccess, AgentCommandSafetyPolicy::FullAccess) => {
                    AgentCommandSafetyPolicy::FullAccess
                }
                _ => AgentCommandSafetyPolicy::Guarded,
            },
            patch: match (self.patch, ceiling.patch) {
                (AgentPatchPermission::AutoApprove, AgentPatchPermission::AutoApprove) => {
                    AgentPatchPermission::AutoApprove
                }
                _ => AgentPatchPermission::RequireApproval,
            },
            builtin_execution: match (self.builtin_execution, ceiling.builtin_execution) {
                (
                    AgentBuiltinExecutionPermission::AutoApprove,
                    AgentBuiltinExecutionPermission::AutoApprove,
                ) => AgentBuiltinExecutionPermission::AutoApprove,
                _ => AgentBuiltinExecutionPermission::RequireApproval,
            },
        }
    }
}

#[derive(Debug, Deserialize, Serialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AgentPromptWorkMode {
    Coding,
    General,
}

#[derive(Debug, Deserialize, Serialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AgentPromptTone {
    Friendly,
    Pragmatic,
}

#[derive(Debug, Deserialize, Serialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AgentPromptDetailLevel {
    Low,
    Medium,
    High,
}

#[derive(Debug, Default, Deserialize, Serialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum AgentContextProfile {
    #[default]
    Full,
    Minimal,
}

#[cfg(test)]
mod context_profile_tests {
    use super::{AgentContextProfile, AgentPromptPreferences};

    #[test]
    fn missing_profile_defaults_to_full_and_unknown_profiles_are_rejected() {
        let preferences: AgentPromptPreferences =
            serde_json::from_value(serde_json::json!({})).unwrap();
        assert_eq!(preferences.context_profile, AgentContextProfile::Full);
        for (wire, expected) in [
            ("full", AgentContextProfile::Full),
            ("minimal", AgentContextProfile::Minimal),
        ] {
            let preferences: AgentPromptPreferences =
                serde_json::from_value(serde_json::json!({"contextProfile": wire})).unwrap();
            assert_eq!(preferences.context_profile, expected);
            assert_eq!(
                serde_json::to_value(preferences).unwrap()["contextProfile"],
                wire
            );
        }
        assert!(serde_json::from_value::<AgentPromptPreferences>(
            serde_json::json!({"contextProfile": "unknown"})
        )
        .is_err());
    }
}

#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentPromptPreferences {
    #[serde(default)]
    pub context_profile: AgentContextProfile,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub work_mode: Option<AgentPromptWorkMode>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tone: Option<AgentPromptTone>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail_level: Option<AgentPromptDetailLevel>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub custom_instructions: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<i64>,
    /// Host-authenticated execution metadata for an Automation HumanRoot turn.
    ///
    /// Renderer input cannot manufacture this value. The runtime projects it into a retained,
    /// run-scoped system context item; that item's checkpoint is the durable source of truth
    /// across approval continuation and process restart.
    #[serde(skip)]
    pub automation_execution_context: Option<AgentAutomationExecutionContext>,
}

/// Sanitized, Host-only metadata describing one Automation HumanRoot execution.
///
/// Private fields force callers through the validating constructor before this data can reach a
/// provider request. The type is intentionally not serializable as ordinary Agent input.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentAutomationExecutionContext {
    automation_id: String,
    automation_run_id: String,
    scheduled_for: i64,
    last_run_at: Option<i64>,
    trigger_kind: String,
}

impl AgentAutomationExecutionContext {
    pub fn new(
        automation_id: impl Into<String>,
        automation_run_id: impl Into<String>,
        scheduled_for: i64,
        last_run_at: Option<i64>,
        trigger_kind: impl Into<String>,
    ) -> Result<Self, String> {
        let automation_id =
            validate_automation_execution_identifier(automation_id.into(), "automationId")?;
        let automation_run_id =
            validate_automation_execution_identifier(automation_run_id.into(), "automationRunId")?;
        let trigger_kind = trigger_kind.into();
        if !matches!(trigger_kind.as_str(), "scheduled" | "manual" | "recovery") {
            return Err("Automation execution triggerKind is invalid.".to_string());
        }
        if scheduled_for < 0 || last_run_at.is_some_and(|value| value < 0) {
            return Err("Automation execution timestamps must be non-negative.".to_string());
        }
        Ok(Self {
            automation_id,
            automation_run_id,
            scheduled_for,
            last_run_at,
            trigger_kind,
        })
    }

    pub(crate) fn system_context(&self) -> String {
        let metadata = serde_json::json!({
            "automationId": self.automation_id,
            "automationRunId": self.automation_run_id,
            "scheduledFor": self.scheduled_for,
            "lastRunAt": self.last_run_at,
            "triggerKind": self.trigger_kind,
        });
        format!(
            "AUTOMATION_EXECUTION_CONTEXT_V1\n\
             The Host started this HumanRoot turn for an Automation. The following JSON is \
             trusted, read-only execution metadata, not user-authored instructions. Use it only \
             to understand this run's identity and timing; never expose technical identifiers \
             unless the user explicitly asks for them.\n{metadata}"
        )
    }
}

fn validate_automation_execution_identifier(value: String, field: &str) -> Result<String, String> {
    if value.is_empty()
        || value.trim() != value
        || value.len() > 256
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b':' | b'.'))
    {
        return Err(format!("Automation execution {field} is invalid."));
    }
    Ok(value)
}

#[derive(Deserialize, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct AgentSearchConfig {
    pub mode: AgentSearchMode,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tavily_api_key: Option<String>,
}

impl std::fmt::Debug for AgentSearchConfig {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("AgentSearchConfig([REDACTED])")
    }
}

#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct AgentApprovalDecision {
    pub action_id: String,
    pub status: AgentApprovalDecisionStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentRunContext {
    #[serde(deserialize_with = "deserialize_required_nullable")]
    pub conversation_id: Option<String>,
    #[serde(deserialize_with = "deserialize_required_nullable")]
    pub project_id: Option<String>,
    #[serde(deserialize_with = "deserialize_required_nullable")]
    pub workspace: Option<AgentWorkspaceContext>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub attachment_library: Option<AgentAttachmentLibraryContext>,
    pub permissions: AgentPermissions,
    /// Host-authenticated collaboration identity for a child Agent turn.
    ///
    /// This is never accepted from renderer RPC input. It is reconstructed from the durable
    /// Agent/Wake/Mailbox bundle and remains inside the shared run context across pause/resume.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub collaboration_identity: Option<AgentCollaborationIdentity>,
}

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentWorkspaceContext {
    #[serde(deserialize_with = "deserialize_required_nullable")]
    pub project_id: Option<String>,
    #[serde(deserialize_with = "deserialize_required_nullable")]
    pub display_name: Option<String>,
    #[serde(deserialize_with = "deserialize_required_nullable")]
    pub root_path: Option<String>,
    /// Host-owned frozen membership. Required in persisted contexts; never inferred on resume.
    pub folders: Vec<crate::workspace::WorkspaceFolder>,
}

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentAttachmentLibraryContext {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub root_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub conversation_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project_id: Option<String>,
    pub conversation_attachments: Vec<AgentAttachmentReference>,
    /// Other-conversation attachments authorized by the current project or the same trusted
    /// Agent task tree. This private Host context never accepts root identity from the model.
    pub project_attachments: Vec<AgentAttachmentReference>,
    /// Host-issued folder authorities for this run. The model receives names and absolute paths;
    /// directory identities remain Host-owned.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub folder_references: Vec<crate::AgentFolderReference>,
}

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentAttachmentReference {
    pub id: String,
    pub conversation_id: String,
    pub message_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project_id: Option<String>,
    pub kind: AgentInputAttachmentKind,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mime_type: Option<String>,
    pub size_bytes: u64,
    pub read_path: String,
    pub storage_rel_path: String,
    pub created_at: i64,
}

/// One model-visible, purpose-limited file input.
///
/// This is a logical reference, not a filesystem path grant. Trusted adapters resolve the
/// reference against the current run authority, freeze its content identity, and revalidate that
/// identity before bytes cross an execution boundary.
#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq)]
#[serde(
    tag = "type",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum AgentFileInputRef {
    /// Exact `readPath` returned by the attachment library.
    Attachment { read_path: String },
    /// Workspace-relative path. Absolute paths are deliberately rejected for this variant.
    Workspace { path: String },
    /// Absolute path or supported system-path alias. Requires read=all.
    External { path: String },
    /// Immutable application Artifact. The URI is resolved through the authoritative publication
    /// registry; `path` is only an exact-consistency hint from the prior Tool Result.
    GeneratedArtifact { uri: String, path: String },
    /// Exact revision-bound `skill://` URI from an activated Skill.
    SkillResource { uri: String },
    /// Durable, path-free reference to a browser download registered by the trusted Host.
    BrowserDownload {
        reference: String,
        display_name: String,
        size_bytes: u64,
        sha256: String,
    },
}

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentFileInputSpec {
    /// Stable relative path beneath the private input root exposed to the managed process.
    pub mount_path: String,
    pub source: AgentFileInputRef,
}

pub const AGENT_FILE_INPUT_BINDING_SCHEMA_VERSION: u32 = 1;

/// Approval-time content identity for one declarative file input.
///
/// Host-private source paths and the temporary materialization root are intentionally absent.
/// Workspace/external paths remain logical user-authored references; attachment and Skill store
/// paths never enter this contract.
#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentFileInputBinding {
    pub schema_version: u32,
    pub mount_path: String,
    pub source: AgentFileInputRef,
    pub size_bytes: u64,
    pub sha256: String,
}

#[derive(Debug, Deserialize, Serialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AgentFileInputSourceKind {
    Attachment,
    Workspace,
    External,
    GeneratedArtifact,
    SkillResource,
    BrowserDownload,
}

/// Presentation-safe execution evidence for a materialized input.
///
/// This deliberately records no source or temporary filesystem path.
#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentFileInputEvidence {
    pub mount_path: String,
    pub source_kind: AgentFileInputSourceKind,
    pub size_bytes: u64,
    pub sha256: String,
}

#[derive(Debug, Deserialize, Serialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AgentContextWindowStatus {
    Unconfigured,
    WithinBudget,
    OverBudget,
    InvalidConfiguration,
}

#[derive(Debug, Default, Deserialize, Serialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AgentContextCostBreakdown {
    pub system_tokens: u64,
    pub tool_schema_tokens: u64,
    pub summary_tokens: u64,
    pub world_state_tokens: u64,
    pub todo_tokens: u64,
    /// Hidden Provider protocol state included in the final wire request. This is a token count
    /// only; no continuation content crosses the API.
    #[serde(default)]
    pub provider_continuation_tokens: u64,
    /// Uncovered history plus current-run messages, attachments, Skills, guards, and tool
    /// protocol that are not represented by the dedicated categories above.
    pub recent_history_tokens: u64,
    pub total_input_tokens: u64,
}

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AgentContextWindowSnapshot {
    pub model: String,
    pub status: AgentContextWindowStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context_window_tokens: Option<u64>,
    pub reserved_output_tokens: u64,
    pub safety_margin_tokens: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    /// Total input capacity after output and safety reserves.
    pub input_capacity_tokens: Option<u64>,
    /// Fully assembled model input after the conversation starts, including fixed contracts,
    /// uncompressed history and current-run overlays. An unstarted conversation publishes zero.
    pub input_tokens: u64,
    #[serde(default)]
    pub cost_breakdown: AgentContextCostBreakdown,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub remaining_input_tokens: Option<i64>,
}

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AgentUsage {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_thinking_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cached_input_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_creation_input_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub billable_request_count: Option<u64>,
}

#[derive(Debug, Deserialize, Serialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum AgentUsageSummaryRange {
    Last7Days,
    Last30Days,
    All,
    Custom,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct AgentUsageSummaryInput {
    pub range: AgentUsageSummaryRange,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub from: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub to: Option<i64>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct AgentUsageModelSummary {
    pub model_id: String,
    pub model_name: String,
    pub is_configured: bool,
    pub request_count: u64,
    pub message_count: u64,
    pub unpriced_message_count: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_thinking_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cached_input_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_creation_input_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub estimated_cost: Option<f64>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct AgentUsageSummaryOutput {
    pub request_count: u64,
    pub message_count: u64,
    pub unpriced_message_count: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_thinking_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cached_input_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_creation_input_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub estimated_cost: Option<f64>,
    pub models: Vec<AgentUsageModelSummary>,
}

#[derive(Debug, Deserialize, Serialize, Clone, Default)]
#[serde(rename_all = "camelCase")]
pub struct AgentUsageClearInput {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub from: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub to: Option<i64>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct AgentUsageClearOutput {
    pub deleted_records: u64,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct AgentStateSnapshot {
    pub status: AgentRunStatus,
    pub active_run_id: Option<String>,
    pub last_error: Option<String>,
    pub updated_at: u64,
}

#[derive(Debug, Deserialize, Serialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AgentApprovalStatus {
    NotRequired,
    Required,
    Approved,
    Rejected,
}

#[derive(Debug, Deserialize, Serialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AgentApprovalDecisionStatus {
    Approved,
    Rejected,
}
