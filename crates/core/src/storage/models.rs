//! Persisted records and validated request types, grouped by storage domain.

mod agent;
mod browser;
mod composer;
mod conversations;
mod model_configuration;
mod preferences;
mod projects;

pub use crate::file_change::FileChangeRunGrantRecord;
pub use agent::{
    AgentActionAuditRecord, AgentFileChangeChunkRecord, AgentFileChangeOperationRecord,
    AgentFileChangeRecord, AgentFileChangeRuntimeState, AgentPendingActionRecord,
    AgentRunGuidanceRecord, AgentUnsettledFileEffect, AgentUsageRecordInsert,
    ManualContextCompactionOperation, ManualContextCompactionUsageRecord,
    McpApprovalEnvelopeRecord,
};
pub use browser::{
    BrowserDownloadListInput, BrowserDownloadLocationMode, BrowserDownloadRecord,
    BrowserDownloadRegistration, BrowserDownloadSettingsRecord, BrowserDownloadSettingsUpdate,
    BrowserDownloadSource, BrowserHistoryDeleteInput, BrowserHistoryListInput,
    BrowserHistoryMetadataUpdate, BrowserHistoryRecord, BrowserHistoryRegistration,
    BrowserLinkOpenTarget, BrowserOwnedDataClearInput, BrowserOwnedDataClearOutput,
    BrowserOwnedDataRangeInput, BrowserOwnedDataSummary, BrowserPreferencesRecord,
    BrowserPreferencesUpdate, BROWSER_DATA_SCHEMA_VERSION, BROWSER_DOWNLOAD_SCHEMA_VERSION,
};
pub use composer::{ComposerDraftRecord, CURRENT_COMPOSER_PERMISSION_MODE_VERSION};
pub use conversations::{
    AttachmentImageRecord, AttachmentRecord, ChatConversationMetaRecord, ChatConversationRecord,
    ChatConversationViewRecord, ChatMessageAttachmentRecord, ChatMessageRecord,
    ChatMessageStateRecord, ChatSearchInput, ChatSearchMatchKind, ChatSearchResult,
    ConversationContinuationOriginRecord, ConversationForkPoint, ForkConversationRequest,
};
pub use model_configuration::{
    normalize_model_display_name, CredentialMutation, CredentialStatus,
    ImageGenerationProfileRecord, ModelConfigEditorRecord, ModelConfigRecord,
    ModelConfigSaveRequest, ModelConnectionConfig, ModelExecutionStatus, ModelSettingsEditorRecord,
    ModelSettingsRecord, ModelSettingsSaveError, ModelSettingsSaveRequest, ModelSettingsSnapshot,
    ProviderProfileUpdate, DEFAULT_MODEL_CONTEXT_WINDOW_TOKENS, MODEL_DISPLAY_NAME_MAX_BYTES,
    MODEL_PROVIDER_API_URL_MAX_BYTES,
};
pub(crate) use model_configuration::{
    validated_connection, StoredModelConfigRecord, StoredModelSettingsRecord,
    StoredModelSettingsSnapshot,
};
pub use preferences::{AgentPromptPreferencesRecord, UiPreferencesRecord};
pub use projects::{
    project_folder_alias_from_path, ProjectFolderRecord, ProjectFolderRole, ProjectRecord,
    MAX_PROJECT_FOLDERS,
};

use serde::Deserialize;

fn deserialize_required_nullable<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(deserializer)
}

#[cfg(test)]
#[path = "models/tests/model_connection.rs"]
mod model_connection_tests;
#[cfg(test)]
#[path = "models/tests/security.rs"]
mod security_tests;
