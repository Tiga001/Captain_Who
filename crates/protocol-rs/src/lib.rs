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

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillsListRequest {
    pub project_id: String,
}

pub const SKILL_CATALOG_SCHEMA_VERSION: u32 = 4;
pub const SKILL_MUTATION_SCHEMA_VERSION: u32 = 1;
pub const SKILL_INSTALLATION_ERROR_CODE: i64 = -32010;

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

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
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

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
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
    pub code: String,
    pub recovery: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub skill_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expected_revision: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub actual_revision: Option<String>,
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
