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
pub const SKILLS_RESOLVE_INSTALLATION_SOURCE_METHOD: &str = "skills.resolveInstallationSource";
pub const SKILLS_CANCEL_SOURCE_RESOLUTION_METHOD: &str = "skills.cancelSourceResolution";
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
    pub run_id: String,
    pub action_id: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentRejectActionRequest {
    pub run_id: String,
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
pub const SKILL_SOURCE_RESOLUTION_SCHEMA_VERSION: u32 = 2;
pub const SKILL_INSTALLATION_ERROR_CODE: i64 = -32010;
pub const SKILL_INSPECTION_ERROR_CODE: i64 = -32011;
pub const SKILL_MANAGEMENT_ERROR_CODE: i64 = -32012;
pub const SKILL_SOURCE_RESOLUTION_ERROR_CODE: i64 = -32013;

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
    /// Exact installation lifecycle revision returned by `skills.listManagement`.
    /// The server still accepts a package revision for legacy callers.
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SkillAcquisitionSourceDto {
    LocalDirectory {
        directory: String,
    },
    GithubRepository {
        owner: String,
        repository: String,
        reference: Option<SkillGithubReferenceDto>,
        subdirectory: Option<String>,
    },
    ResolvedCandidate {
        resolution_id: SkillResolutionIdDto,
        candidate_id: String,
    },
    InstalledSource {},
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(tag = "kind", deny_unknown_fields)]
enum SkillAcquisitionSourceDtoWire {
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
    #[serde(rename = "resolvedCandidate", rename_all = "camelCase")]
    ResolvedCandidate {
        resolution_id: SkillResolutionIdDto,
        candidate_id: String,
    },
    #[serde(rename = "installedSource")]
    InstalledSource {},
}

impl SkillAcquisitionSourceDto {
    fn validate(&self) -> Result<(), String> {
        match self {
            Self::LocalDirectory { directory } if directory.trim().is_empty() => {
                return Err("local directory must be non-empty".to_string());
            }
            Self::GithubRepository {
                owner,
                repository,
                reference,
                subdirectory,
            } => {
                if owner.trim().is_empty() || repository.trim().is_empty() {
                    return Err("GitHub owner and repository must be non-empty".to_string());
                }
                if subdirectory
                    .as_deref()
                    .is_some_and(|value| value.trim().is_empty())
                {
                    return Err("GitHub subdirectory must be non-empty when present".to_string());
                }
                match reference {
                    Some(SkillGithubReferenceDto::Named { value }) if value.trim().is_empty() => {
                        return Err("GitHub named reference must be non-empty".to_string());
                    }
                    Some(SkillGithubReferenceDto::Commit { sha }) => {
                        SkillResolvedGithubCommitShaDto::parse(sha.clone())
                            .map_err(|reason| reason.to_string())?;
                    }
                    _ => {}
                }
            }
            Self::ResolvedCandidate { candidate_id, .. } if candidate_id.trim().is_empty() => {
                return Err("resolved candidate id must be non-empty".to_string());
            }
            _ => {}
        }
        Ok(())
    }
}

impl From<SkillAcquisitionSourceDtoWire> for SkillAcquisitionSourceDto {
    fn from(value: SkillAcquisitionSourceDtoWire) -> Self {
        match value {
            SkillAcquisitionSourceDtoWire::LocalDirectory { directory } => {
                Self::LocalDirectory { directory }
            }
            SkillAcquisitionSourceDtoWire::GithubRepository {
                owner,
                repository,
                reference,
                subdirectory,
            } => Self::GithubRepository {
                owner,
                repository,
                reference,
                subdirectory,
            },
            SkillAcquisitionSourceDtoWire::ResolvedCandidate {
                resolution_id,
                candidate_id,
            } => Self::ResolvedCandidate {
                resolution_id,
                candidate_id,
            },
            SkillAcquisitionSourceDtoWire::InstalledSource {} => Self::InstalledSource {},
        }
    }
}

impl From<&SkillAcquisitionSourceDto> for SkillAcquisitionSourceDtoWire {
    fn from(value: &SkillAcquisitionSourceDto) -> Self {
        match value {
            SkillAcquisitionSourceDto::LocalDirectory { directory } => Self::LocalDirectory {
                directory: directory.clone(),
            },
            SkillAcquisitionSourceDto::GithubRepository {
                owner,
                repository,
                reference,
                subdirectory,
            } => Self::GithubRepository {
                owner: owner.clone(),
                repository: repository.clone(),
                reference: reference.clone(),
                subdirectory: subdirectory.clone(),
            },
            SkillAcquisitionSourceDto::ResolvedCandidate {
                resolution_id,
                candidate_id,
            } => Self::ResolvedCandidate {
                resolution_id: resolution_id.clone(),
                candidate_id: candidate_id.clone(),
            },
            SkillAcquisitionSourceDto::InstalledSource {} => Self::InstalledSource {},
        }
    }
}

impl Serialize for SkillAcquisitionSourceDto {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        self.validate().map_err(serde::ser::Error::custom)?;
        SkillAcquisitionSourceDtoWire::from(self).serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for SkillAcquisitionSourceDto {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = Self::from(SkillAcquisitionSourceDtoWire::deserialize(deserializer)?);
        value.validate().map_err(serde::de::Error::custom)?;
        Ok(value)
    }
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

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SkillsResolveInstallationSourceRequest {
    pub resolution_id: SkillResolutionIdDto,
    pub locator: SkillInstallationSourceLocatorDto,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SkillsCancelSourceResolutionRequest {
    pub resolution_id: SkillResolutionIdDto,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum SkillSourceResolutionCancellationOutcomeDto {
    Cancelled,
    AlreadyCancelled,
    AlreadyConsumed,
    AlreadyAbsent,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SkillsCancelSourceResolutionResponse {
    pub schema_version: u32,
    pub resolution_id: SkillResolutionIdDto,
    pub outcome: SkillSourceResolutionCancellationOutcomeDto,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(tag = "kind", deny_unknown_fields)]
pub enum SkillInstallationSourceLocatorDto {
    #[serde(rename = "url")]
    Url { url: String },
}

/// A canonical non-nil lower-case UUID generated by the client for one source resolution.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SkillResolutionIdDto(String);

impl SkillResolutionIdDto {
    pub fn parse(value: impl Into<String>) -> Result<Self, &'static str> {
        let value = value.into();
        let bytes = value.as_bytes();
        let valid = bytes.len() == 36
            && bytes.iter().enumerate().all(|(index, byte)| match index {
                8 | 13 | 18 | 23 => *byte == b'-',
                _ => byte.is_ascii_digit() || (b'a'..=b'f').contains(byte),
            })
            && bytes
                .iter()
                .any(|byte| matches!(*byte, b'1'..=b'9' | b'a'..=b'f'));
        if !valid {
            return Err("expected a canonical non-nil lower-case UUID");
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn into_inner(self) -> String {
        self.0
    }
}

impl Serialize for SkillResolutionIdDto {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for SkillResolutionIdDto {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::parse(value).map_err(serde::de::Error::custom)
    }
}

/// A canonical, lower-case, complete Git commit SHA.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SkillResolvedGithubCommitShaDto(String);

impl SkillResolvedGithubCommitShaDto {
    pub fn parse(value: impl Into<String>) -> Result<Self, &'static str> {
        let value = value.into();
        let is_full_lowercase_sha = value.len() == 40
            && value
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte));
        if !is_full_lowercase_sha {
            return Err("expected a lower-case 40-character hexadecimal SHA");
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn into_inner(self) -> String {
        self.0
    }
}

impl Serialize for SkillResolvedGithubCommitShaDto {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for SkillResolvedGithubCommitShaDto {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::parse(value).map_err(serde::de::Error::custom)
    }
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum SkillSourceResolutionProviderDto {
    Github,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum SkillSourceResolutionOutcomeDto {
    Resolved,
    SelectionRequired,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(tag = "kind", deny_unknown_fields)]
pub enum SkillResolvedAcquisitionSourceDto {
    #[serde(rename = "githubRepository", rename_all = "camelCase")]
    GithubRepository {
        owner: String,
        repository: String,
        reference: SkillGithubReferenceDto,
        resolved_commit: SkillResolvedGithubCommitShaDto,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        subdirectory: Option<String>,
    },
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(tag = "kind", deny_unknown_fields)]
pub enum SkillResolvedCandidateAcquisitionDto {
    #[serde(rename = "resolvedCandidate", rename_all = "camelCase")]
    ResolvedCandidate {
        resolution_id: SkillResolutionIdDto,
        candidate_id: String,
    },
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SkillSourceResolutionCandidateDto {
    pub candidate_id: String,
    pub acquisition: SkillResolvedCandidateAcquisitionDto,
    pub source: SkillResolvedAcquisitionSourceDto,
    pub package: SkillPackagePreviewDto,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillsResolveInstallationSourceResponse {
    pub schema_version: u32,
    pub resolution_id: SkillResolutionIdDto,
    pub canonical_url: String,
    pub provider: SkillSourceResolutionProviderDto,
    pub resolved_commit: SkillResolvedGithubCommitShaDto,
    pub expires_at_unix_ms: u64,
    pub outcome: SkillSourceResolutionOutcomeDto,
    pub candidates: Vec<SkillSourceResolutionCandidateDto>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SkillsResolveInstallationSourceResponseWire {
    schema_version: u32,
    resolution_id: SkillResolutionIdDto,
    canonical_url: String,
    provider: SkillSourceResolutionProviderDto,
    resolved_commit: SkillResolvedGithubCommitShaDto,
    expires_at_unix_ms: u64,
    outcome: SkillSourceResolutionOutcomeDto,
    candidates: Vec<SkillSourceResolutionCandidateDto>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SkillsResolveInstallationSourceResponseWireRef<'a> {
    schema_version: u32,
    resolution_id: &'a SkillResolutionIdDto,
    canonical_url: &'a str,
    provider: SkillSourceResolutionProviderDto,
    resolved_commit: &'a SkillResolvedGithubCommitShaDto,
    expires_at_unix_ms: u64,
    outcome: SkillSourceResolutionOutcomeDto,
    candidates: &'a [SkillSourceResolutionCandidateDto],
}

impl Serialize for SkillsResolveInstallationSourceResponse {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        self.validate().map_err(serde::ser::Error::custom)?;
        SkillsResolveInstallationSourceResponseWireRef {
            schema_version: self.schema_version,
            resolution_id: &self.resolution_id,
            canonical_url: &self.canonical_url,
            provider: self.provider,
            resolved_commit: &self.resolved_commit,
            expires_at_unix_ms: self.expires_at_unix_ms,
            outcome: self.outcome,
            candidates: &self.candidates,
        }
        .serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for SkillsResolveInstallationSourceResponse {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let wire = SkillsResolveInstallationSourceResponseWire::deserialize(deserializer)?;
        let response = Self {
            schema_version: wire.schema_version,
            resolution_id: wire.resolution_id,
            canonical_url: wire.canonical_url,
            provider: wire.provider,
            resolved_commit: wire.resolved_commit,
            expires_at_unix_ms: wire.expires_at_unix_ms,
            outcome: wire.outcome,
            candidates: wire.candidates,
        };
        response.validate().map_err(serde::de::Error::custom)?;
        Ok(response)
    }
}

impl SkillsResolveInstallationSourceResponse {
    pub fn validate(&self) -> Result<(), String> {
        if self.schema_version != SKILL_SOURCE_RESOLUTION_SCHEMA_VERSION {
            return Err(format!(
                "unsupported Skill source resolution schema version {}",
                self.schema_version
            ));
        }
        if self.canonical_url.trim().is_empty() {
            return Err("canonicalUrl must be non-empty".to_string());
        }
        if self.expires_at_unix_ms == 0 || self.expires_at_unix_ms > 9_007_199_254_740_991 {
            return Err("expiresAtUnixMs must be a positive JavaScript-safe integer".to_string());
        }
        match self.outcome {
            SkillSourceResolutionOutcomeDto::Resolved if self.candidates.len() != 1 => {
                return Err("resolved outcome requires exactly one candidate".to_string());
            }
            SkillSourceResolutionOutcomeDto::SelectionRequired if self.candidates.len() < 2 => {
                return Err(
                    "selectionRequired outcome requires at least two candidates".to_string()
                );
            }
            _ => {}
        }

        let mut candidate_ids = std::collections::HashSet::new();
        for candidate in &self.candidates {
            if candidate.candidate_id.trim().is_empty() {
                return Err("candidateId must be non-empty".to_string());
            }
            if !candidate_ids.insert(candidate.candidate_id.as_str()) {
                return Err("candidateId values must be unique".to_string());
            }
            let SkillResolvedCandidateAcquisitionDto::ResolvedCandidate {
                resolution_id,
                candidate_id,
            } = &candidate.acquisition;
            if resolution_id != &self.resolution_id {
                return Err(
                    "candidate acquisition resolutionId must match response.resolutionId"
                        .to_string(),
                );
            }
            if candidate_id != &candidate.candidate_id {
                return Err(
                    "candidate acquisition candidateId must match candidate.candidateId"
                        .to_string(),
                );
            }
            if candidate.package.format_version == 0
                || candidate.package.package_revision.trim().is_empty()
                || candidate.package.name.trim().is_empty()
                || candidate.package.description.trim().is_empty()
                || candidate.package.file_count == 0
                || candidate.package.total_bytes == 0
            {
                return Err("candidate package preview is invalid".to_string());
            }
            match &candidate.source {
                SkillResolvedAcquisitionSourceDto::GithubRepository {
                    owner,
                    repository,
                    reference,
                    resolved_commit,
                    subdirectory,
                } => {
                    if owner.trim().is_empty() || repository.trim().is_empty() {
                        return Err("candidate GitHub repository identity is invalid".to_string());
                    }
                    if matches!(subdirectory, Some(value) if value.trim().is_empty()) {
                        return Err(
                            "candidate subdirectory must be non-empty when present".to_string()
                        );
                    }
                    if resolved_commit != &self.resolved_commit {
                        return Err(
                            "candidate resolvedCommit must match response.resolvedCommit"
                                .to_string(),
                        );
                    }
                    match reference {
                        SkillGithubReferenceDto::DefaultBranch {} => {}
                        SkillGithubReferenceDto::Named { value } if value.trim().is_empty() => {
                            return Err("candidate named reference must be non-empty".to_string());
                        }
                        SkillGithubReferenceDto::Named { .. } => {}
                        SkillGithubReferenceDto::Commit { sha } => {
                            let reference_commit =
                                SkillResolvedGithubCommitShaDto::parse(sha.clone())
                                    .map_err(|reason| reason.to_string())?;
                            if &reference_commit != resolved_commit {
                                return Err("candidate commit reference sha must match candidate resolvedCommit".to_string());
                            }
                        }
                    }
                }
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum SkillSourceResolutionPhaseDto {
    Parse,
    Resolve,
    Discover,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum SkillSourceResolutionErrorCodeDto {
    InvalidLocator,
    UnsupportedLocator,
    UnsupportedHost,
    UnsupportedUrlShape,
    RepositoryNotFound,
    ReferenceNotFound,
    PathNotFound,
    AmbiguousReference,
    NoSkillsFound,
    TooManySkills,
    NetworkUnavailable,
    RateLimited,
    RepositoryTooLarge,
    UnsafePackage,
    InvalidPackage,
    ResolutionIdConflict,
    ResolutionNotFoundOrExpired,
    ResolutionConsumed,
    CandidateNotFound,
    CapacityExceeded,
    Cancelled,
    Unavailable,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum SkillSourceResolutionRecoveryDto {
    FixLocator,
    RetryLater,
    NarrowLocator,
    ChooseDifferentSource,
    RetrySameResolution,
    StartNewResolution,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SkillSourceResolutionErrorData {
    #[serde(rename = "type")]
    pub error_type: SkillSourceResolutionErrorTypeDto,
    pub phase: SkillSourceResolutionPhaseDto,
    pub code: SkillSourceResolutionErrorCodeDto,
    pub recovery: SkillSourceResolutionRecoveryDto,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<SkillSourceResolutionProviderDto>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_after_ms: Option<u64>,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum SkillSourceResolutionErrorTypeDto {
    SkillSourceResolution,
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
    ResolutionNotFound,
    ResolutionExpired,
    ResolutionConsumed,
    CandidateNotFound,
    CapacityExceeded,
    InstallationRetired,
    InstallationNotFound,
    InstallationRevisionConflict,
    SourceNotRefreshable,
    PersistedSourceInvalid,
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
    CommitIndeterminate,
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
    NewInstallationIdentity,
    FreeCapacity,
    ContactSupport,
    AcknowledgeWarnings,
    ChooseDifferentSource,
    ResolveAgain,
    RefreshManagement,
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
    #[serde(default, skip_serializing_if = "bool_is_false")]
    pub commit_may_have_succeeded: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skill_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub intended_installation_revision: Option<String>,
}

fn bool_is_false(value: &bool) -> bool {
    !*value
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
    InstallationRetired,
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
    NewInstallationIdentity,
    RefreshCatalog,
    FreeCapacity,
    ContactSupport,
    RepairStore,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum SkillInstallationCapacityDto {
    Installations,
    InstallationDirectory,
    RetiredInstallationIds,
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
        for error in golden["inspectionErrors"].as_array().unwrap() {
            assert_wire_round_trip::<SkillInspectionErrorData>(error);
        }
        for error in golden["managementErrors"].as_array().unwrap() {
            assert_wire_round_trip::<SkillManagementErrorData>(error);
        }
    }

    #[test]
    fn skill_source_resolution_v2_matches_the_shared_wire_golden() {
        let golden: Value = serde_json::from_str(include_str!(
            "../../../packages/protocol/fixtures/skill-source-resolution-v2.json"
        ))
        .unwrap();

        assert_eq!(
            golden["schemaVersion"],
            SKILL_SOURCE_RESOLUTION_SCHEMA_VERSION
        );
        assert_eq!(golden["method"], SKILLS_RESOLVE_INSTALLATION_SOURCE_METHOD);
        assert_eq!(
            golden["cancel"]["method"],
            SKILLS_CANCEL_SOURCE_RESOLUTION_METHOD
        );
        assert_eq!(SKILL_SOURCE_RESOLUTION_ERROR_CODE, -32013);
        for case in golden["cases"].as_array().unwrap() {
            assert_wire_round_trip::<SkillsResolveInstallationSourceRequest>(&case["request"]);
            assert_wire_round_trip::<SkillsResolveInstallationSourceResponse>(&case["response"]);
        }
        for error in golden["errors"].as_array().unwrap() {
            assert_wire_round_trip::<SkillSourceResolutionErrorData>(error);
        }
        for case in golden["cancel"]["cases"].as_array().unwrap() {
            assert_wire_round_trip::<SkillsCancelSourceResolutionRequest>(&case["request"]);
            assert_wire_round_trip::<SkillsCancelSourceResolutionResponse>(&case["response"]);
        }
    }

    #[test]
    fn skill_source_resolution_boundaries_are_strict_and_immutable() {
        let golden: Value = serde_json::from_str(include_str!(
            "../../../packages/protocol/fixtures/skill-source-resolution-v2.json"
        ))
        .unwrap();

        assert!(
            serde_json::from_value::<SkillsResolveInstallationSourceRequest>(serde_json::json!({
                "resolutionId": "11111111-1111-4111-8111-111111111111",
                "locator": {
                    "kind": "url",
                    "url": "https://github.com/openai/example-skills",
                    "credential": "must-not-cross-the-boundary"
                }
            }))
            .is_err()
        );
        assert!(
            serde_json::from_value::<SkillsCancelSourceResolutionRequest>(serde_json::json!({
                "resolutionId": "11111111-1111-4111-8111-111111111111",
                "candidateId": "must-not-cross-the-boundary"
            }))
            .is_err()
        );
        assert!(
            serde_json::from_value::<SkillsCancelSourceResolutionResponse>(serde_json::json!({
                "schemaVersion": 2,
                "resolutionId": "11111111-1111-4111-8111-111111111111",
                "outcome": "forgotten"
            }))
            .is_err()
        );
        assert!(
            serde_json::from_value::<SkillsResolveInstallationSourceRequest>(serde_json::json!({
                "resolutionId": "00000000-0000-0000-0000-000000000000",
                "locator": {
                    "kind": "url",
                    "url": "https://github.com/openai/example-skills"
                }
            }))
            .is_err()
        );
        assert!(
            serde_json::from_value::<SkillsResolveInstallationSourceRequest>(serde_json::json!({
                "resolutionId": "11111111-1111-4111-8111-111111111111",
                "locator": {
                    "kind": "git",
                    "url": "https://github.com/openai/example-skills"
                }
            }))
            .is_err()
        );

        let mut short_commit = golden["cases"][0]["response"].clone();
        short_commit["resolvedCommit"] = serde_json::json!("abc123");
        assert!(
            serde_json::from_value::<SkillsResolveInstallationSourceResponse>(short_commit)
                .is_err()
        );
        let mut uppercase_commit = golden["cases"][0]["response"].clone();
        uppercase_commit["resolvedCommit"] =
            serde_json::json!("ABCDEF0123456789ABCDEF0123456789ABCDEF01");
        assert!(
            serde_json::from_value::<SkillsResolveInstallationSourceResponse>(uppercase_commit)
                .is_err()
        );

        let mut mismatched_candidate_commit = golden["cases"][0]["response"].clone();
        mismatched_candidate_commit["candidates"][0]["source"]["resolvedCommit"] =
            serde_json::json!("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa");
        assert!(
            serde_json::from_value::<SkillsResolveInstallationSourceResponse>(
                mismatched_candidate_commit
            )
            .is_err()
        );

        let mut mismatched_tracking_commit = golden["cases"][0]["response"].clone();
        mismatched_tracking_commit["candidates"][0]["source"]["reference"] = serde_json::json!({
            "kind": "commit",
            "sha": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
        });
        assert!(
            serde_json::from_value::<SkillsResolveInstallationSourceResponse>(
                mismatched_tracking_commit
            )
            .is_err()
        );

        let mut missing_candidate_commit = golden["cases"][0]["response"].clone();
        missing_candidate_commit["candidates"][0]["source"]
            .as_object_mut()
            .unwrap()
            .remove("resolvedCommit");
        assert!(
            serde_json::from_value::<SkillsResolveInstallationSourceResponse>(
                missing_candidate_commit
            )
            .is_err()
        );

        let mut mismatched_resolution_id = golden["cases"][0]["response"].clone();
        mismatched_resolution_id["candidates"][0]["acquisition"]["resolutionId"] =
            serde_json::json!("99999999-9999-4999-8999-999999999999");
        assert!(
            serde_json::from_value::<SkillsResolveInstallationSourceResponse>(
                mismatched_resolution_id
            )
            .is_err()
        );

        let mut mismatched_candidate_id = golden["cases"][0]["response"].clone();
        mismatched_candidate_id["candidates"][0]["acquisition"]["candidateId"] =
            serde_json::json!("different-candidate");
        assert!(
            serde_json::from_value::<SkillsResolveInstallationSourceResponse>(
                mismatched_candidate_id
            )
            .is_err()
        );

        for invalid_resolution_id in [
            "00000000-0000-0000-0000-000000000000",
            "11111111-1111-4111-8111-11111111111A",
            "not-a-uuid",
        ] {
            let mut response = golden["cases"][0]["response"].clone();
            response["resolutionId"] = serde_json::json!(invalid_resolution_id);
            assert!(
                serde_json::from_value::<SkillsResolveInstallationSourceResponse>(response)
                    .is_err()
            );
        }

        let mut invalid_expiry = golden["cases"][0]["response"].clone();
        invalid_expiry["expiresAtUnixMs"] = serde_json::json!(0);
        assert!(
            serde_json::from_value::<SkillsResolveInstallationSourceResponse>(invalid_expiry)
                .is_err()
        );

        let mut invalid_cardinality = golden["cases"][1]["response"].clone();
        invalid_cardinality["outcome"] = serde_json::json!("resolved");
        assert!(
            serde_json::from_value::<SkillsResolveInstallationSourceResponse>(invalid_cardinality)
                .is_err()
        );
        let mut invalid_output: SkillsResolveInstallationSourceResponse =
            serde_json::from_value(golden["cases"][0]["response"].clone()).unwrap();
        invalid_output.outcome = SkillSourceResolutionOutcomeDto::SelectionRequired;
        assert!(serde_json::to_value(invalid_output).is_err());

        let mut duplicate_candidate_ids = golden["cases"][1]["response"].clone();
        let first_candidate_id = duplicate_candidate_ids["candidates"][0]["candidateId"].clone();
        duplicate_candidate_ids["candidates"][1]["candidateId"] = first_candidate_id.clone();
        duplicate_candidate_ids["candidates"][1]["acquisition"]["candidateId"] = first_candidate_id;
        assert!(
            serde_json::from_value::<SkillsResolveInstallationSourceResponse>(
                duplicate_candidate_ids
            )
            .is_err()
        );

        let mut unknown_response_field = golden["cases"][0]["response"].clone();
        unknown_response_field
            .as_object_mut()
            .unwrap()
            .insert("credential".to_string(), serde_json::json!("secret"));
        assert!(
            serde_json::from_value::<SkillsResolveInstallationSourceResponse>(
                unknown_response_field
            )
            .is_err()
        );
        for (field, value) in [
            ("schemaVersion", serde_json::json!(1)),
            ("provider", serde_json::json!("gitlab")),
            ("outcome", serde_json::json!("installed")),
        ] {
            let mut response = golden["cases"][0]["response"].clone();
            response[field] = value;
            assert!(
                serde_json::from_value::<SkillsResolveInstallationSourceResponse>(response)
                    .is_err()
            );
        }

        let mut unknown_error_code = golden["errors"][0].clone();
        unknown_error_code["code"] = serde_json::json!("repositoryMoved");
        assert!(
            serde_json::from_value::<SkillSourceResolutionErrorData>(unknown_error_code).is_err()
        );
        let mut unknown_error_field = golden["errors"][0].clone();
        unknown_error_field
            .as_object_mut()
            .unwrap()
            .insert("internalUrl".to_string(), serde_json::json!("secret"));
        assert!(
            serde_json::from_value::<SkillSourceResolutionErrorData>(unknown_error_field).is_err()
        );
        for (field, value) in [
            ("phase", "install"),
            ("recovery", "retrySamePreparation"),
            ("provider", "gitlab"),
        ] {
            let mut error = golden["errors"][1].clone();
            error[field] = serde_json::json!(value);
            assert!(serde_json::from_value::<SkillSourceResolutionErrorData>(error).is_err());
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
        for invalid_source in [
            serde_json::json!({ "kind": "localDirectory", "directory": "  " }),
            serde_json::json!({
                "kind": "githubRepository",
                "owner": "",
                "repository": "skills"
            }),
            serde_json::json!({
                "kind": "githubRepository",
                "owner": "example",
                "repository": "skills",
                "reference": { "kind": "named", "value": " " }
            }),
            serde_json::json!({
                "kind": "githubRepository",
                "owner": "example",
                "repository": "skills",
                "subdirectory": " "
            }),
        ] {
            assert!(serde_json::from_value::<SkillAcquisitionSourceDto>(invalid_source).is_err());
        }
        assert!(
            serde_json::from_value::<SkillAcquisitionSourceDto>(serde_json::json!({
                "kind": "githubRepository",
                "owner": "example",
                "repository": "skills",
                "reference": { "kind": "named", "value": "main" },
                "subdirectory": "skills/auditor"
            }))
            .is_ok()
        );
        assert!(
            serde_json::from_value::<SkillAcquisitionSourceDto>(serde_json::json!({
                "kind": "githubRepository",
                "owner": "example",
                "repository": "skills",
                "reference": { "kind": "named", "value": "main" },
                "resolvedCommit": "0123456789abcdef0123456789abcdef01234567"
            }))
            .is_err()
        );
        assert!(
            serde_json::from_value::<SkillAcquisitionSourceDto>(serde_json::json!({
                "kind": "resolvedCandidate",
                "resolutionId": "00000000-0000-0000-0000-000000000000",
                "candidateId": "candidate"
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

    #[test]
    fn action_decision_requests_require_run_scoped_identity() {
        let action: AgentActionIdRequest = serde_json::from_value(serde_json::json!({
            "runId": "run-1",
            "actionId": "call-1"
        }))
        .unwrap();
        assert_eq!(action.run_id, "run-1");
        assert_eq!(action.action_id, "call-1");
        assert!(
            serde_json::from_value::<AgentActionIdRequest>(serde_json::json!({
                "actionId": "call-1"
            }))
            .is_err()
        );
        assert!(
            serde_json::from_value::<AgentRejectActionRequest>(serde_json::json!({
                "actionId": "call-1",
                "message": "no"
            }))
            .is_err()
        );
    }
}
