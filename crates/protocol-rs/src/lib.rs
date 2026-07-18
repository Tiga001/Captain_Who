use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const CORE_PING_METHOD: &str = "core.ping";
pub const CORE_SHUTDOWN_METHOD: &str = "core.shutdown";
pub const AGENT_CANCEL_RUN_METHOD: &str = "agent.cancelRun";
pub const AGENT_START_CONVERSATION_TURN_METHOD: &str = "agent.startConversationTurn";
pub const AGENT_GET_CONTEXT_WINDOW_SNAPSHOT_METHOD: &str = "agent.getContextWindowSnapshot";
pub const AGENT_GET_CONTEXT_COMPACTION_AUDIT_METHOD: &str = "agent.getContextCompactionAudit";
pub const AGENT_LIST_PENDING_ACTIONS_METHOD: &str = "agent.listPendingActions";
pub const AGENT_APPROVE_ACTION_METHOD: &str = "agent.approveAction";
pub const AGENT_REJECT_ACTION_METHOD: &str = "agent.rejectAction";
pub const AGENT_CANCEL_ACTION_METHOD: &str = "agent.cancelAction";
pub const AGENT_GET_USAGE_SUMMARY_METHOD: &str = "agent.getUsageSummary";
pub const AGENT_CLEAR_USAGE_RECORDS_METHOD: &str = "agent.clearUsageRecords";
pub const AGENT_READ_FILE_DRAFT_METHOD: &str = "agent.readFileDraft";
pub const AGENT_GET_FILE_WRITE_DIFF_METHOD: &str = "agent.getFileWriteDiff";
pub const AGENT_EVENT_NOTIFICATION_METHOD: &str = "agent.event";
pub const SEARCH_SEARCH_CHATS_METHOD: &str = "search.searchChats";
pub const SKILLS_LIST_METHOD: &str = "skills.list";
pub const SKILLS_INSTALL_LOCAL_METHOD: &str = "skills.installLocal";
pub const SKILLS_UPDATE_LOCAL_METHOD: &str = "skills.updateLocal";
pub const SKILLS_UNINSTALL_METHOD: &str = "skills.uninstall";
pub const SKILLS_INSPECT_INSTALLATION_METHOD: &str = "skills.inspectInstallation";
pub const SKILLS_COMMIT_INSTALLATION_METHOD: &str = "skills.commitInstallation";
pub const SKILLS_CANCEL_PREPARATION_METHOD: &str = "skills.cancelPreparation";
pub const SKILLS_LIST_MANAGEMENT_METHOD: &str = "skills.listManagement";
pub const SKILLS_SET_ENABLED_METHOD: &str = "skills.setEnabled";
pub const SKILLS_CHANGED_NOTIFICATION_METHOD: &str = "skills.changed";
pub const GIT_INSPECT_REPOSITORY_METHOD: &str = "git.inspectRepository";
pub const GIT_GET_REVIEW_SUMMARY_METHOD: &str = "git.getReviewSummary";
pub const GIT_GET_REVIEW_FILE_DIFF_METHOD: &str = "git.getReviewFileDiff";
pub const GIT_GET_REVIEW_FILE_CONTENT_METHOD: &str = "git.getReviewFileContent";
pub const GIT_MUTATE_REVIEW_FILE_METHOD: &str = "git.mutateReviewFile";
pub const STORAGE_LOAD_MODEL_SETTINGS_METHOD: &str = "storage.loadModelSettings";
pub const STORAGE_SAVE_MODEL_SETTINGS_METHOD: &str = "storage.saveModelSettings";
pub const STORAGE_LOAD_AGENT_PROMPT_PREFERENCES_METHOD: &str = "storage.loadAgentPromptPreferences";
pub const STORAGE_SAVE_AGENT_PROMPT_PREFERENCES_METHOD: &str = "storage.saveAgentPromptPreferences";
pub const STORAGE_LOAD_PROJECTS_METHOD: &str = "storage.loadProjects";
pub const STORAGE_SAVE_PROJECT_METHOD: &str = "storage.saveProject";
pub const STORAGE_DELETE_PROJECT_METHOD: &str = "storage.deleteProject";
pub const STORAGE_LOAD_CONVERSATIONS_METHOD: &str = "storage.loadConversations";
pub const STORAGE_SAVE_CONVERSATION_META_METHOD: &str = "storage.saveConversationMeta";
pub const STORAGE_DELETE_CONVERSATION_METHOD: &str = "storage.deleteConversation";
pub const STORAGE_DELETE_CHAT_MESSAGES_METHOD: &str = "storage.deleteChatMessages";
pub const STORAGE_FORK_CONVERSATION_METHOD: &str = "storage.forkConversation";
pub const STORAGE_UPSERT_CHAT_MESSAGES_METHOD: &str = "storage.upsertChatMessages";
pub const STORAGE_SAVE_CHAT_MESSAGE_STATE_METHOD: &str = "storage.saveChatMessageState";
pub const STORAGE_LOAD_COMPOSER_DRAFTS_METHOD: &str = "storage.loadComposerDrafts";
pub const STORAGE_SAVE_COMPOSER_DRAFT_METHOD: &str = "storage.saveComposerDraft";
pub const STORAGE_LOAD_UI_PREFERENCES_METHOD: &str = "storage.loadUiPreferences";
pub const STORAGE_SAVE_UI_PREFERENCES_METHOD: &str = "storage.saveUiPreferences";
pub const STORAGE_LOAD_ATTACHMENT_IMAGE_METHOD: &str = "storage.loadAttachmentImage";
pub const STORAGE_LOAD_INPUT_ATTACHMENTS_METHOD: &str = "storage.loadInputAttachments";

#[derive(Debug, Deserialize)]
pub struct JsonRpcRequest {
    pub jsonrpc: String,
    pub id: JsonRpcId,
    pub method: String,
    #[serde(default)]
    pub params: Option<Value>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(untagged)]
pub enum JsonRpcId {
    String(String),
    Number(i64),
}

#[derive(Debug, Serialize)]
pub struct JsonRpcSuccessResponse<T>
where
    T: Serialize,
{
    pub jsonrpc: &'static str,
    pub id: JsonRpcId,
    pub result: T,
}

#[derive(Debug, Serialize)]
pub struct JsonRpcErrorResponse {
    pub jsonrpc: &'static str,
    pub id: Option<JsonRpcId>,
    pub error: JsonRpcErrorObject,
}

#[derive(Debug, Serialize)]
pub struct JsonRpcErrorObject {
    pub code: i64,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentFileDraftReadRequest {
    pub draft_id: String,
    pub offset: Option<usize>,
    pub max_chars: Option<usize>,
}

#[derive(Debug, Deserialize)]
pub struct CorePingRequest {
    pub message: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CorePingResponse {
    pub message: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub echo: Option<String>,
    pub server_time_ms: u128,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CoreShutdownResponse {
    pub cancelled_runs: usize,
    pub timed_out: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentCancelRunRequest {
    pub run_id: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentCancelRunResponse {
    pub run_id: String,
    pub cancelled: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentActionIdRequest {
    pub action_id: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentRejectActionRequest {
    pub action_id: String,
    pub message: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GitRepositoryInspectRequest {
    pub project_id: String,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SkillsListRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_id: Option<String>,
}

pub const SKILL_CATALOG_SCHEMA_VERSION: u32 = 4;
pub const SKILL_MUTATION_SCHEMA_VERSION: u32 = 1;
pub const SKILL_INSTALLATION_WORKFLOW_SCHEMA_VERSION: u32 = 1;
pub const SKILL_MANAGEMENT_SCHEMA_VERSION: u32 = 1;
pub const SKILL_INSTALLATION_ERROR_CODE: i64 = -32010;
pub const SKILL_INSPECTION_ERROR_CODE: i64 = -32011;
pub const SKILL_MANAGEMENT_ERROR_CODE: i64 = -32012;

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SkillsInstallLocalRequest {
    pub installation_id: String,
    pub directory: String,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SkillsUpdateLocalRequest {
    pub skill_id: String,
    pub expected_revision: String,
    pub directory: String,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SkillsUninstallRequest {
    pub skill_id: String,
    pub expected_revision: String,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SkillsInspectInstallationRequest {
    pub preparation_id: String,
    pub intent: SkillInstallationIntentDto,
    pub source: SkillAcquisitionSourceDto,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(tag = "operation", deny_unknown_fields)]
pub enum SkillInstallationIntentDto {
    #[serde(rename = "install")]
    Install {},
    #[serde(rename = "update", rename_all = "camelCase")]
    Update {
        skill_id: String,
        expected_installation_revision: String,
    },
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(tag = "kind", deny_unknown_fields)]
pub enum SkillAcquisitionSourceDto {
    #[serde(rename = "localDirectory", rename_all = "camelCase")]
    LocalDirectory { directory: String },
    #[serde(rename = "githubRepository", rename_all = "camelCase")]
    GithubRepository {
        owner: String,
        repository: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reference: Option<SkillGithubReferenceDto>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        subdirectory: Option<String>,
    },
    #[serde(rename = "installedSource")]
    InstalledSource {},
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(tag = "kind", deny_unknown_fields)]
pub enum SkillGithubReferenceDto {
    #[serde(rename = "defaultBranch")]
    DefaultBranch {},
    #[serde(rename = "named")]
    Named { value: String },
    #[serde(rename = "commit")]
    Commit { sha: String },
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum SkillPreparationOperationDto {
    Install,
    Update,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SkillInstallationPreviewDto {
    pub schema_version: u32,
    pub preparation_id: String,
    pub preview_revision: String,
    pub operation: SkillPreparationOperationDto,
    pub installation_id: String,
    pub skill_id: String,
    pub package: SkillPackagePreviewDto,
    pub source: SkillPreviewSourceDto,
    pub compatibility: SkillCompatibilityReportDto,
    pub changes: SkillInstallationChangesDto,
    pub expires_at_unix_ms: u64,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SkillPackagePreviewDto {
    pub format_version: u32,
    pub package_revision: String,
    pub name: String,
    pub description: String,
    pub file_count: u64,
    pub total_bytes: u64,
}

/// A presentation-safe source summary. Authority-bearing local paths and
/// credentials must never be placed in this DTO.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(tag = "kind", deny_unknown_fields)]
pub enum SkillPreviewSourceDto {
    #[serde(rename = "localDirectory", rename_all = "camelCase")]
    LocalDirectory {
        display_name: String,
        refreshable: bool,
    },
    #[serde(rename = "githubRepository", rename_all = "camelCase")]
    GithubRepository {
        owner: String,
        repository: String,
        reference: SkillGithubReferenceDto,
        resolved_commit: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        subdirectory: Option<String>,
        refreshable: bool,
    },
    #[serde(rename = "installedSource", rename_all = "camelCase")]
    InstalledSource {
        display_name: String,
        refreshable: bool,
    },
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum SkillCompatibilityStatusDto {
    Compatible,
    CompatibleWithWarnings,
    Unknown,
    Incompatible,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum SkillCompatibilityIssueSeverityDto {
    Warning,
    Error,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SkillCompatibilityIssueDto {
    pub id: String,
    pub code: String,
    pub severity: SkillCompatibilityIssueSeverityDto,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capability: Option<String>,
    pub requires_acknowledgement: bool,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SkillCompatibilityReportDto {
    pub status: SkillCompatibilityStatusDto,
    pub issues: Vec<SkillCompatibilityIssueDto>,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum SkillInstallationChangeDto {
    New,
    Changed,
    Unchanged,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SkillInstallationChangesDto {
    pub content: SkillInstallationChangeDto,
    pub source: SkillInstallationChangeDto,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SkillsCommitInstallationRequest {
    pub preparation_id: String,
    pub preview_revision: String,
    pub accepted_issue_ids: Vec<String>,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum SkillInstallationCommitOutcomeDto {
    Installed,
    AlreadyInstalled,
    Updated,
    AlreadyCurrent,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SkillInstallationCommitResponse {
    pub schema_version: u32,
    pub preparation_id: String,
    pub operation: SkillPreparationOperationDto,
    pub outcome: SkillInstallationCommitOutcomeDto,
    pub installation_id: String,
    pub skill_id: String,
    pub package_revision: String,
    pub installation_revision: String,
    pub changes: SkillInstallationChangesDto,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SkillsCancelPreparationRequest {
    pub preparation_id: String,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum SkillPreparationCancellationOutcomeDto {
    Cancelled,
    AlreadyAbsent,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SkillPreparationCancellationResponse {
    pub schema_version: u32,
    pub preparation_id: String,
    pub outcome: SkillPreparationCancellationOutcomeDto,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum SkillInspectionPhaseDto {
    Inspect,
    Commit,
    Cancel,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum SkillInspectionErrorCodeDto {
    InvalidSource,
    UnsupportedSource,
    SourceNotAccessible,
    ReferenceNotFound,
    SubdirectoryNotFound,
    SourceChangedDuringRead,
    NetworkUnavailable,
    RateLimited,
    RepositoryTooLarge,
    UnsafePackage,
    InvalidPackage,
    Incompatible,
    PreparationNotFound,
    PreparationExpired,
    IdempotencyConflict,
    PreviewMismatch,
    AcknowledgementRequired,
    Cancelled,
    Unavailable,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum SkillInspectionRecoveryDto {
    FixSource,
    RetrySamePreparation,
    RetryLater,
    InspectAgain,
    AcknowledgeWarnings,
    ChooseDifferentSource,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SkillInspectionErrorData {
    #[serde(rename = "type")]
    pub error_type: SkillInspectionErrorTypeDto,
    pub phase: SkillInspectionPhaseDto,
    pub code: SkillInspectionErrorCodeDto,
    pub recovery: SkillInspectionRecoveryDto,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preparation_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub diagnostic_code: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_after_ms: Option<u64>,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum SkillInspectionErrorTypeDto {
    SkillInspection,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SkillsListManagementRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_id: Option<String>,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum SkillManagementOperationDto {
    List,
    SetEnabled,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum SkillManagementErrorCodeDto {
    NotFound,
    NotManageable,
    StateConflict,
    StorageUnavailable,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum SkillManagementRecoveryDto {
    RefreshManagement,
    Retry,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SkillManagementErrorData {
    #[serde(rename = "type")]
    pub error_type: SkillManagementErrorTypeDto,
    pub operation: SkillManagementOperationDto,
    pub code: SkillManagementErrorCodeDto,
    pub recovery: SkillManagementRecoveryDto,
    pub message: String,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum SkillManagementErrorTypeDto {
    SkillManagement,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SkillManagementActionsDto {
    pub can_set_enabled: bool,
    pub can_update: bool,
    pub can_uninstall: bool,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SkillManagementEntryDto {
    pub id: String,
    pub name: String,
    pub description: String,
    pub source: SkillSourceDto,
    pub package_revision: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub installation_revision: Option<String>,
    pub state_revision: String,
    pub enabled: bool,
    pub actions: SkillManagementActionsDto,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub acquisition: Option<SkillPreviewSourceDto>,
    pub compatibility: SkillCompatibilityReportDto,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SkillsListManagementResponse {
    pub schema_version: u32,
    pub management_revision: String,
    pub skills: Vec<SkillManagementEntryDto>,
    pub diagnostics: Vec<SkillDiagnosticDto>,
    pub truncated: bool,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SkillsSetEnabledRequest {
    pub skill_id: String,
    pub expected_state_revision: String,
    pub enabled: bool,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum SkillSetEnabledOutcomeDto {
    Updated,
    AlreadyCurrent,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SkillsSetEnabledResponse {
    pub schema_version: u32,
    pub management_revision: String,
    pub skill_id: String,
    pub state_revision: String,
    pub enabled: bool,
    pub outcome: SkillSetEnabledOutcomeDto,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum SkillsChangedReasonDto {
    Installed,
    Updated,
    Uninstalled,
    EnablementChanged,
    CatalogChanged,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SkillsChangedNotification {
    pub schema_version: u32,
    pub management_revision: String,
    pub reason: SkillsChangedReasonDto,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skill_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SkillMutationResponse {
    schema_version: u32,
    installation_id: String,
    skill_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    revision: Option<String>,
    outcome: SkillMutationOutcomeDto,
}

impl SkillMutationResponse {
    pub fn install(
        installation_id: String,
        skill_id: String,
        revision: String,
        outcome: SkillInstallMutationOutcomeDto,
    ) -> Self {
        Self {
            schema_version: SKILL_MUTATION_SCHEMA_VERSION,
            installation_id,
            skill_id,
            revision: Some(revision),
            outcome: outcome.into(),
        }
    }

    pub fn update(
        installation_id: String,
        skill_id: String,
        revision: String,
        outcome: SkillUpdateMutationOutcomeDto,
    ) -> Self {
        Self {
            schema_version: SKILL_MUTATION_SCHEMA_VERSION,
            installation_id,
            skill_id,
            revision: Some(revision),
            outcome: outcome.into(),
        }
    }

    pub fn removal(
        installation_id: String,
        skill_id: String,
        outcome: SkillRemovalMutationOutcomeDto,
    ) -> Self {
        Self {
            schema_version: SKILL_MUTATION_SCHEMA_VERSION,
            installation_id,
            skill_id,
            revision: None,
            outcome: outcome.into(),
        }
    }
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
enum SkillMutationOutcomeDto {
    Installed,
    AlreadyInstalled,
    Updated,
    AlreadyCurrent,
    Uninstalled,
    AlreadyAbsent,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkillInstallMutationOutcomeDto {
    Installed,
    AlreadyInstalled,
}

impl From<SkillInstallMutationOutcomeDto> for SkillMutationOutcomeDto {
    fn from(value: SkillInstallMutationOutcomeDto) -> Self {
        match value {
            SkillInstallMutationOutcomeDto::Installed => Self::Installed,
            SkillInstallMutationOutcomeDto::AlreadyInstalled => Self::AlreadyInstalled,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkillUpdateMutationOutcomeDto {
    Updated,
    AlreadyCurrent,
}

impl From<SkillUpdateMutationOutcomeDto> for SkillMutationOutcomeDto {
    fn from(value: SkillUpdateMutationOutcomeDto) -> Self {
        match value {
            SkillUpdateMutationOutcomeDto::Updated => Self::Updated,
            SkillUpdateMutationOutcomeDto::AlreadyCurrent => Self::AlreadyCurrent,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkillRemovalMutationOutcomeDto {
    Uninstalled,
    AlreadyAbsent,
}

impl From<SkillRemovalMutationOutcomeDto> for SkillMutationOutcomeDto {
    fn from(value: SkillRemovalMutationOutcomeDto) -> Self {
        match value {
            SkillRemovalMutationOutcomeDto::Uninstalled => Self::Uninstalled,
            SkillRemovalMutationOutcomeDto::AlreadyAbsent => Self::AlreadyAbsent,
        }
    }
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum SkillInstallationOperationDto {
    Install,
    Update,
    Uninstall,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum SkillInstallationErrorCodeDto {
    PreparationFailed,
    InvalidSkill,
    InvalidStore,
    CapacityExceeded,
    InstallationExists,
    InstallationNotFound,
    RevisionConflict,
    StoreCorrupt,
    Io,
    Unavailable,
    CommitIndeterminate,
    Cancelled,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum SkillInstallationRecoveryDto {
    FixLocalSource,
    RetrySameRequest,
    RefreshCatalog,
    FreeCapacity,
    RepairStore,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum SkillInstallationCapacityDto {
    Installations,
    InstallationDirectory,
    Packages,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SkillInstallationErrorData {
    #[serde(rename = "type")]
    pub error_type: SkillInstallationErrorTypeDto,
    pub operation: SkillInstallationOperationDto,
    pub code: SkillInstallationErrorCodeDto,
    pub recovery: SkillInstallationRecoveryDto,
    pub message: String,
    pub commit_may_have_succeeded: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub installation_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub skill_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub diagnostic_code: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub intended_revision: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expected_revision: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub actual_revision: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub capacity: Option<SkillInstallationCapacityDto>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit: Option<usize>,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum SkillInstallationErrorTypeDto {
    SkillInstallation,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SkillSelectionDto {
    pub id: String,
    pub revision: String,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SkillSourceDto {
    pub kind: SkillSourceKindDto,
    pub id: String,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum SkillSourceKindDto {
    Workspace,
    Bundled,
    Installed,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum SkillTrustDto {
    Untrusted,
    Application,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SkillDescriptorDto {
    pub id: String,
    pub name: String,
    pub description: String,
    pub source: SkillSourceDto,
    pub trust: SkillTrustDto,
    pub activation_scope: String,
    pub revision: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub location: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SkillDiagnosticDto {
    pub code: String,
    pub severity: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub skill_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub location: Option<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SkillsListResponse {
    pub schema_version: u32,
    pub catalog_revision: String,
    pub skills: Vec<SkillDescriptorDto>,
    pub diagnostics: Vec<SkillDiagnosticDto>,
    pub truncated: bool,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ActivatedSkillSummaryDto {
    pub id: String,
    pub name: String,
    pub revision: String,
    pub source: SkillSourceDto,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SkillActivationErrorData {
    #[serde(rename = "type")]
    pub error_type: &'static str,
    pub code: SkillActivationErrorCodeDto,
    pub recovery: SkillActivationRecoveryDto,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub skill_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expected_revision: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub actual_revision: Option<String>,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum SkillActivationErrorCodeDto {
    InvalidSelection,
    DuplicateSelection,
    TooManySkills,
    ActivationTooLarge,
    NotFound,
    Stale,
    InvalidSkill,
    Disabled,
    SourceUnavailable,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum SkillActivationRecoveryDto {
    RetrySameSelection,
    RefreshCatalog,
    RejectSelection,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GitReviewSummaryRequest {
    pub project_id: String,
    pub scope: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GitReviewFileDiffRequest {
    pub snapshot_id: String,
    pub file_id: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GitReviewFileContentRequest {
    pub snapshot_id: String,
    pub file_id: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GitReviewFileMutationRequest {
    pub snapshot_id: String,
    pub file_id: String,
    pub action: String,
}

pub fn success<T>(id: JsonRpcId, result: T) -> JsonRpcSuccessResponse<T>
where
    T: Serialize,
{
    JsonRpcSuccessResponse {
        jsonrpc: "2.0",
        id,
        result,
    }
}

pub fn error(id: Option<JsonRpcId>, code: i64, message: impl Into<String>) -> JsonRpcErrorResponse {
    JsonRpcErrorResponse {
        jsonrpc: "2.0",
        id,
        error: JsonRpcErrorObject {
            code,
            message: message.into(),
            data: None,
        },
    }
}

pub fn error_with_data(
    id: Option<JsonRpcId>,
    code: i64,
    message: impl Into<String>,
    data: Value,
) -> JsonRpcErrorResponse {
    JsonRpcErrorResponse {
        jsonrpc: "2.0",
        id,
        error: JsonRpcErrorObject {
            code,
            message: message.into(),
            data: Some(data),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn skill_catalog_v4_serializes_explicit_source_and_trust_unions() {
        let response = SkillsListResponse {
            schema_version: SKILL_CATALOG_SCHEMA_VERSION,
            catalog_revision: "catalog-revision".to_string(),
            skills: vec![SkillDescriptorDto {
                id: "installed:user:0190b0f2-7c50-7cc0-8b25-3bb80f08b334".to_string(),
                name: "repository-evidence-auditor".to_string(),
                description: "Audit repository claims using evidence.".to_string(),
                source: SkillSourceDto {
                    kind: SkillSourceKindDto::Installed,
                    id: "installed:user".to_string(),
                },
                trust: SkillTrustDto::Untrusted,
                activation_scope: "run".to_string(),
                revision: concat!(
                    "skill-package-sha256-v1:",
                    "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
                )
                .to_string(),
                location: Some(
                    concat!(
                        "packages/v1/",
                        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa/",
                        "SKILL.md"
                    )
                    .to_string(),
                ),
            }],
            diagnostics: Vec::new(),
            truncated: false,
        };

        let value = serde_json::to_value(response).unwrap();

        assert_eq!(value["schemaVersion"], 4);
        assert_eq!(value["skills"][0]["source"]["kind"], "installed");
        assert_eq!(value["skills"][0]["source"]["id"], "installed:user");
        assert_eq!(value["skills"][0]["trust"], "untrusted");
    }

    #[test]
    fn skill_catalog_v4_accepts_installed_and_rejects_unknown_enum_values() {
        assert_eq!(
            serde_json::from_str::<SkillSourceKindDto>("\"installed\"").unwrap(),
            SkillSourceKindDto::Installed
        );
        assert!(serde_json::from_str::<SkillSourceKindDto>("\"remote\"").is_err());
        assert!(serde_json::from_str::<SkillTrustDto>("\"userApproved\"").is_err());
    }

    #[test]
    fn skill_mutation_requests_are_strict_camel_case_contracts() {
        assert_eq!(SKILLS_INSTALL_LOCAL_METHOD, "skills.installLocal");
        assert_eq!(SKILLS_UPDATE_LOCAL_METHOD, "skills.updateLocal");
        assert_eq!(SKILLS_UNINSTALL_METHOD, "skills.uninstall");

        let install = serde_json::from_value::<SkillsInstallLocalRequest>(serde_json::json!({
            "installationId": "018f7f31-7a6d-7a21-9e51-ff4b6fa4e38d",
            "directory": "/tmp/local-skill"
        }))
        .unwrap();
        assert_eq!(
            install.installation_id,
            "018f7f31-7a6d-7a21-9e51-ff4b6fa4e38d"
        );
        assert_eq!(install.directory, "/tmp/local-skill");

        let update = serde_json::from_value::<SkillsUpdateLocalRequest>(serde_json::json!({
            "skillId": "installed:user:018f7f31-7a6d-7a21-9e51-ff4b6fa4e38d",
            "expectedRevision": "skill-package-sha256-v1:old",
            "directory": "/tmp/local-skill"
        }))
        .unwrap();
        assert_eq!(
            update.skill_id,
            "installed:user:018f7f31-7a6d-7a21-9e51-ff4b6fa4e38d"
        );
        assert_eq!(update.expected_revision, "skill-package-sha256-v1:old");

        let uninstall = serde_json::from_value::<SkillsUninstallRequest>(serde_json::json!({
            "skillId": "installed:user:018f7f31-7a6d-7a21-9e51-ff4b6fa4e38d",
            "expectedRevision": "skill-package-sha256-v1:current"
        }))
        .unwrap();
        assert_eq!(
            uninstall.skill_id,
            "installed:user:018f7f31-7a6d-7a21-9e51-ff4b6fa4e38d"
        );

        assert!(
            serde_json::from_value::<SkillsInstallLocalRequest>(serde_json::json!({
                "installationId": "018f7f31-7a6d-7a21-9e51-ff4b6fa4e38d",
                "directory": "/tmp/local-skill",
                "storeRoot": "/tmp/attacker-controlled"
            }))
            .is_err()
        );
        assert!(
            serde_json::from_value::<SkillsUpdateLocalRequest>(serde_json::json!({
                "skillId": "installed:user:018f7f31-7a6d-7a21-9e51-ff4b6fa4e38d",
                "directory": "/tmp/local-skill"
            }))
            .is_err()
        );
    }

    #[test]
    fn skill_mutation_v1_serializes_all_idempotent_outcomes() {
        assert_eq!(SKILL_MUTATION_SCHEMA_VERSION, 1);
        let install_outcomes = [
            (SkillInstallMutationOutcomeDto::Installed, "installed"),
            (
                SkillInstallMutationOutcomeDto::AlreadyInstalled,
                "alreadyInstalled",
            ),
        ];

        for (outcome, expected) in install_outcomes {
            let response = SkillMutationResponse::install(
                "018f7f31-7a6d-7a21-9e51-ff4b6fa4e38d".to_string(),
                "installed:user:018f7f31-7a6d-7a21-9e51-ff4b6fa4e38d".to_string(),
                "skill-package-sha256-v1:current".to_string(),
                outcome,
            );
            let value = serde_json::to_value(response).unwrap();

            assert_eq!(value["schemaVersion"], SKILL_MUTATION_SCHEMA_VERSION);
            assert_eq!(value["outcome"], expected);
            assert_eq!(value["revision"], "skill-package-sha256-v1:current");
        }

        let update_outcomes = [
            (SkillUpdateMutationOutcomeDto::Updated, "updated"),
            (
                SkillUpdateMutationOutcomeDto::AlreadyCurrent,
                "alreadyCurrent",
            ),
        ];

        for (outcome, expected) in update_outcomes {
            let response = SkillMutationResponse::update(
                "018f7f31-7a6d-7a21-9e51-ff4b6fa4e38d".to_string(),
                "installed:user:018f7f31-7a6d-7a21-9e51-ff4b6fa4e38d".to_string(),
                "skill-package-sha256-v1:current".to_string(),
                outcome,
            );
            let value = serde_json::to_value(response).unwrap();

            assert_eq!(value["schemaVersion"], SKILL_MUTATION_SCHEMA_VERSION);
            assert_eq!(value["outcome"], expected);
            assert_eq!(value["revision"], "skill-package-sha256-v1:current");
        }

        let removal_outcomes = [
            (SkillRemovalMutationOutcomeDto::Uninstalled, "uninstalled"),
            (
                SkillRemovalMutationOutcomeDto::AlreadyAbsent,
                "alreadyAbsent",
            ),
        ];
        for (outcome, expected) in removal_outcomes {
            let response = SkillMutationResponse::removal(
                "018f7f31-7a6d-7a21-9e51-ff4b6fa4e38d".to_string(),
                "installed:user:018f7f31-7a6d-7a21-9e51-ff4b6fa4e38d".to_string(),
                outcome,
            );
            let value = serde_json::to_value(response).unwrap();

            assert_eq!(value["outcome"], expected);
            assert!(value.get("revision").is_none());
        }
    }

    #[test]
    fn skill_mutation_v1_matches_the_shared_rust_typescript_wire_golden() {
        let golden: serde_json::Value = serde_json::from_str(include_str!(
            "../../../packages/protocol/fixtures/skill-mutation-v1.json"
        ))
        .unwrap();
        assert_eq!(golden["schemaVersion"], SKILL_MUTATION_SCHEMA_VERSION);

        for case in golden["cases"].as_array().unwrap() {
            let expected = &case["response"];
            let installation_id = expected["installationId"].as_str().unwrap().to_string();
            let skill_id = expected["skillId"].as_str().unwrap().to_string();
            let outcome = expected["outcome"].as_str().unwrap();
            let response = match (case["operation"].as_str().unwrap(), outcome) {
                ("install", "installed") => SkillMutationResponse::install(
                    installation_id,
                    skill_id,
                    expected["revision"].as_str().unwrap().to_string(),
                    SkillInstallMutationOutcomeDto::Installed,
                ),
                ("install", "alreadyInstalled") => SkillMutationResponse::install(
                    installation_id,
                    skill_id,
                    expected["revision"].as_str().unwrap().to_string(),
                    SkillInstallMutationOutcomeDto::AlreadyInstalled,
                ),
                ("update", "updated") => SkillMutationResponse::update(
                    installation_id,
                    skill_id,
                    expected["revision"].as_str().unwrap().to_string(),
                    SkillUpdateMutationOutcomeDto::Updated,
                ),
                ("update", "alreadyCurrent") => SkillMutationResponse::update(
                    installation_id,
                    skill_id,
                    expected["revision"].as_str().unwrap().to_string(),
                    SkillUpdateMutationOutcomeDto::AlreadyCurrent,
                ),
                ("uninstall", "uninstalled") => SkillMutationResponse::removal(
                    installation_id,
                    skill_id,
                    SkillRemovalMutationOutcomeDto::Uninstalled,
                ),
                ("uninstall", "alreadyAbsent") => SkillMutationResponse::removal(
                    installation_id,
                    skill_id,
                    SkillRemovalMutationOutcomeDto::AlreadyAbsent,
                ),
                combination => panic!("unexpected shared Skill mutation case {combination:?}"),
            };

            assert_eq!(serde_json::to_value(response).unwrap(), *expected);
        }
    }

    #[test]
    fn skill_installation_workflow_v1_matches_the_shared_wire_golden() {
        let golden: Value = serde_json::from_str(include_str!(
            "../../../packages/protocol/fixtures/skill-installation-workflow-v1.json"
        ))
        .unwrap();
        assert_eq!(
            golden["workflowSchemaVersion"],
            SKILL_INSTALLATION_WORKFLOW_SCHEMA_VERSION
        );
        assert_eq!(
            golden["managementSchemaVersion"],
            SKILL_MANAGEMENT_SCHEMA_VERSION
        );
        assert_eq!(SKILL_MANAGEMENT_ERROR_CODE, -32012);

        for case in golden["inspectCases"].as_array().unwrap() {
            assert_wire_round_trip::<SkillsInspectInstallationRequest>(&case["request"]);
            assert_wire_round_trip::<SkillInstallationPreviewDto>(&case["preview"]);
        }
        assert_wire_round_trip::<SkillsCommitInstallationRequest>(&golden["commit"]["request"]);
        assert_wire_round_trip::<SkillInstallationCommitResponse>(&golden["commit"]["response"]);
        assert_wire_round_trip::<SkillsCancelPreparationRequest>(&golden["cancel"]["request"]);
        assert_wire_round_trip::<SkillPreparationCancellationResponse>(
            &golden["cancel"]["response"],
        );
        assert_wire_round_trip::<SkillsListManagementRequest>(&golden["management"]["listRequest"]);
        assert_wire_round_trip::<SkillsListManagementResponse>(
            &golden["management"]["listResponse"],
        );
        assert_wire_round_trip::<SkillsSetEnabledRequest>(
            &golden["management"]["setEnabledRequest"],
        );
        assert_wire_round_trip::<SkillsSetEnabledResponse>(
            &golden["management"]["setEnabledResponse"],
        );
        assert_wire_round_trip::<SkillsChangedNotification>(&golden["management"]["changed"]);
        assert_wire_round_trip::<SkillInspectionErrorData>(&golden["inspectionError"]);
        for error in golden["managementErrors"].as_array().unwrap() {
            assert_wire_round_trip::<SkillManagementErrorData>(error);
        }
    }

    #[test]
    fn skill_installation_workflow_requests_are_strict_discriminated_unions() {
        assert_eq!(
            SKILLS_INSPECT_INSTALLATION_METHOD,
            "skills.inspectInstallation"
        );
        assert_eq!(
            SKILLS_COMMIT_INSTALLATION_METHOD,
            "skills.commitInstallation"
        );
        assert_eq!(SKILLS_CANCEL_PREPARATION_METHOD, "skills.cancelPreparation");
        assert_eq!(SKILLS_LIST_MANAGEMENT_METHOD, "skills.listManagement");
        assert_eq!(SKILLS_SET_ENABLED_METHOD, "skills.setEnabled");
        assert_eq!(SKILLS_CHANGED_NOTIFICATION_METHOD, "skills.changed");

        let no_workspace = serde_json::from_value::<SkillsListRequest>(serde_json::json!({}))
            .expect("projectId is optional");
        assert_eq!(no_workspace.project_id, None);
        let workspace = serde_json::from_value::<SkillsListRequest>(serde_json::json!({
            "projectId": "project-one"
        }))
        .unwrap();
        assert_eq!(workspace.project_id.as_deref(), Some("project-one"));

        assert!(
            serde_json::from_value::<SkillsInspectInstallationRequest>(serde_json::json!({
                "preparationId": "11111111-1111-4111-8111-111111111111",
                "intent": { "operation": "install", "skillId": "not-allowed" },
                "source": { "kind": "installedSource" }
            }))
            .is_err()
        );
        assert!(
            serde_json::from_value::<SkillsInspectInstallationRequest>(serde_json::json!({
                "preparationId": "11111111-1111-4111-8111-111111111111",
                "intent": { "operation": "install" },
                "source": {
                    "kind": "localDirectory",
                    "directory": "/tmp/skill",
                    "credential": "must-not-cross-the-boundary"
                }
            }))
            .is_err()
        );
        assert!(
            serde_json::from_value::<SkillsInspectInstallationRequest>(serde_json::json!({
                "preparationId": "11111111-1111-4111-8111-111111111111",
                "intent": { "operation": "install" },
                "source": {
                    "kind": "githubRepository",
                    "owner": "example",
                    "repository": "skills",
                    "reference": { "kind": "commit", "sha": "abc", "ref": "main" }
                }
            }))
            .is_err()
        );

        assert!(
            serde_json::from_value::<SkillManagementErrorData>(serde_json::json!({
                "type": "skillManagement",
                "operation": "setEnabled",
                "code": "stateConflict",
                "recovery": "refreshManagement",
                "message": "Refresh the management inventory.",
                "currentStateRevision": "must-not-be-invented"
            }))
            .is_err()
        );
        assert!(
            serde_json::from_value::<SkillManagementErrorData>(serde_json::json!({
                "type": "skillManagement",
                "operation": "setEnabled",
                "code": "stale",
                "recovery": "refreshManagement",
                "message": "Refresh the management inventory."
            }))
            .is_err()
        );
    }

    #[test]
    fn disabled_skill_activation_has_a_stable_reject_selection_contract() {
        let data = SkillActivationErrorData {
            error_type: "skillActivation",
            code: SkillActivationErrorCodeDto::Disabled,
            recovery: SkillActivationRecoveryDto::RejectSelection,
            message: "The selected Skill is disabled.".to_string(),
            skill_id: Some("installed:user:22222222-2222-4222-8222-222222222222".to_string()),
            expected_revision: None,
            actual_revision: None,
        };
        let value = serde_json::to_value(data).unwrap();

        assert_eq!(value["code"], "disabled");
        assert_eq!(value["recovery"], "rejectSelection");
    }

    fn assert_wire_round_trip<T>(value: &Value)
    where
        T: for<'de> Deserialize<'de> + Serialize,
    {
        let decoded: T = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(serde_json::to_value(decoded).unwrap(), *value);
    }

    #[test]
    fn skill_installation_error_data_is_stable_and_never_contains_a_directory() {
        let data = SkillInstallationErrorData {
            error_type: SkillInstallationErrorTypeDto::SkillInstallation,
            operation: SkillInstallationOperationDto::Update,
            code: SkillInstallationErrorCodeDto::CommitIndeterminate,
            recovery: SkillInstallationRecoveryDto::RetrySameRequest,
            message: "The update may already be visible.".to_string(),
            commit_may_have_succeeded: true,
            installation_id: Some("018f7f31-7a6d-7a21-9e51-ff4b6fa4e38d".to_string()),
            skill_id: Some("installed:user:018f7f31-7a6d-7a21-9e51-ff4b6fa4e38d".to_string()),
            diagnostic_code: None,
            intended_revision: Some("skill-package-sha256-v1:new".to_string()),
            expected_revision: Some("skill-package-sha256-v1:old".to_string()),
            actual_revision: None,
            capacity: None,
            limit: None,
        };

        let value = serde_json::to_value(data).unwrap();

        assert_eq!(SKILL_INSTALLATION_ERROR_CODE, -32010);
        assert_eq!(value["type"], "skillInstallation");
        assert_eq!(value["operation"], "update");
        assert_eq!(value["code"], "commitIndeterminate");
        assert_eq!(value["recovery"], "retrySameRequest");
        assert_eq!(value["commitMayHaveSucceeded"], true);
        assert!(value.get("directory").is_none());
        assert!(value.get("actualRevision").is_none());
    }
}
