mod agent;
mod agent_support;
mod git_dispatcher;
mod skill_installation_workflow_adapter;
mod skill_source_resolution_adapter;
mod skills_adapter;
mod skills_dispatcher;
#[cfg(test)]
mod skills_installation_tests;
#[cfg(test)]
mod skills_test_support;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use agent::{AgentConversationTurnInput, AgentService, AgentServiceError};
use git_dispatcher::{GitDispatcher, GitJobPriority};
use mycopilot_core::git_review::{GitReviewFileMutationAction, GitReviewScope, GitReviewService};
use mycopilot_core::skills::{
    GitHubAcquisitionTransport, GitHubInstallationSourceResolver, GitHubSkillAcquirer,
    GitHubWorkflowAcquisitionAdapter, LocalSkillInstallRequest, LocalSkillUpdateRequest,
    ReqwestGitHubTransport, SkillId, SkillInstallationId, SkillInstallationMutation,
    SkillInstallationOperation, SkillInstallationRevision, SkillInstallationService,
    SkillInstallationServiceError, SkillInstallationWorkflow, SkillRevision,
    SkillSourceResolutionService, SkillUninstallExactRequest, SkillUninstallRequest, SkillsService,
    SKILL_INSTALLATION_REVISION_PREFIX,
};
use mycopilot_core::storage::models::{
    AgentPromptPreferencesRecord, ChatConversationMetaRecord, ChatMessageRecord,
    ChatMessageStateRecord, ChatSearchInput, ComposerDraftRecord, ForkConversationInput,
    ModelSettingsRecord, ProjectRecord, UiPreferencesRecord,
};
use mycopilot_core::storage::service::StorageService;
use mycopilot_core::{AgentUsageClearInput, AgentUsageSummaryInput};
use mycopilot_protocol_rs::{
    error, error_with_data, success, AgentActionIdRequest, AgentCancelRunRequest,
    AgentCancelRunResponse, AgentFileDraftReadRequest, AgentRejectActionRequest, CorePingRequest,
    CorePingResponse, CoreShutdownResponse, GitRepositoryInspectRequest,
    GitReviewFileContentRequest, GitReviewFileDiffRequest, GitReviewFileMutationRequest,
    GitReviewSummaryRequest, JsonRpcId, JsonRpcRequest, SkillsCancelPreparationRequest,
    SkillsCancelSourceResolutionRequest, SkillsChangedNotification, SkillsChangedReasonDto,
    SkillsCommitInstallationRequest, SkillsInspectInstallationRequest, SkillsInstallLocalRequest,
    SkillsListManagementRequest, SkillsListRequest, SkillsResolveInstallationSourceRequest,
    SkillsSetEnabledRequest, SkillsUninstallRequest, SkillsUpdateLocalRequest,
    AGENT_APPROVE_ACTION_METHOD, AGENT_CANCEL_ACTION_METHOD, AGENT_CANCEL_RUN_METHOD,
    AGENT_CLEAR_USAGE_RECORDS_METHOD, AGENT_GET_CONTEXT_COMPACTION_AUDIT_METHOD,
    AGENT_GET_CONTEXT_WINDOW_SNAPSHOT_METHOD, AGENT_GET_FILE_WRITE_DIFF_METHOD,
    AGENT_GET_USAGE_SUMMARY_METHOD, AGENT_LIST_PENDING_ACTIONS_METHOD,
    AGENT_READ_FILE_DRAFT_METHOD, AGENT_REJECT_ACTION_METHOD, AGENT_START_CONVERSATION_TURN_METHOD,
    CORE_PING_METHOD, CORE_SHUTDOWN_METHOD, GIT_GET_REVIEW_FILE_CONTENT_METHOD,
    GIT_GET_REVIEW_FILE_DIFF_METHOD, GIT_GET_REVIEW_SUMMARY_METHOD, GIT_INSPECT_REPOSITORY_METHOD,
    GIT_MUTATE_REVIEW_FILE_METHOD, OFFICE_GET_STATUS_METHOD, SEARCH_SEARCH_CHATS_METHOD,
    SKILLS_CANCEL_PREPARATION_METHOD, SKILLS_CANCEL_SOURCE_RESOLUTION_METHOD,
    SKILLS_CHANGED_NOTIFICATION_METHOD, SKILLS_COMMIT_INSTALLATION_METHOD,
    SKILLS_INSPECT_INSTALLATION_METHOD, SKILLS_INSTALL_LOCAL_METHOD, SKILLS_LIST_MANAGEMENT_METHOD,
    SKILLS_LIST_METHOD, SKILLS_RESOLVE_INSTALLATION_SOURCE_METHOD, SKILLS_SET_ENABLED_METHOD,
    SKILLS_UNINSTALL_METHOD, SKILLS_UPDATE_LOCAL_METHOD, SKILL_INSPECTION_ERROR_CODE,
    SKILL_INSTALLATION_ERROR_CODE, SKILL_MANAGEMENT_ERROR_CODE, SKILL_MANAGEMENT_SCHEMA_VERSION,
    SKILL_SOURCE_RESOLUTION_ERROR_CODE, STORAGE_DELETE_CHAT_MESSAGES_METHOD,
    STORAGE_DELETE_CONVERSATION_METHOD, STORAGE_DELETE_PROJECT_METHOD,
    STORAGE_FORK_CONVERSATION_METHOD, STORAGE_LOAD_AGENT_PROMPT_PREFERENCES_METHOD,
    STORAGE_LOAD_ATTACHMENT_IMAGE_METHOD, STORAGE_LOAD_COMPOSER_DRAFTS_METHOD,
    STORAGE_LOAD_CONVERSATIONS_METHOD, STORAGE_LOAD_CONVERSATION_METAS_METHOD,
    STORAGE_LOAD_CONVERSATION_METHOD, STORAGE_LOAD_INPUT_ATTACHMENTS_METHOD,
    STORAGE_LOAD_MODEL_SETTINGS_METHOD, STORAGE_LOAD_PROJECTS_METHOD,
    STORAGE_LOAD_UI_PREFERENCES_METHOD, STORAGE_SAVE_AGENT_PROMPT_PREFERENCES_METHOD,
    STORAGE_SAVE_CHAT_MESSAGE_STATE_METHOD, STORAGE_SAVE_COMPOSER_DRAFT_METHOD,
    STORAGE_SAVE_CONVERSATION_META_METHOD, STORAGE_SAVE_MODEL_SETTINGS_METHOD,
    STORAGE_SAVE_PROJECT_METHOD, STORAGE_SAVE_UI_PREFERENCES_METHOD,
    STORAGE_UPSERT_CHAT_MESSAGES_METHOD,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use skill_installation_workflow_adapter::{
    absent_cancellation_response, cancellation_preparation_id, cancellation_response,
    commit_request, commit_response, dispatch_failure, is_missing_preparation, preparation_request,
    preview_response, workflow_failure, SkillInspectionFailure,
};
use skill_source_resolution_adapter::{
    cancellation_resolution_id, resolution_dispatch_failure, resolution_failure,
    resolution_request, resolution_response, source_resolution_cancellation_response,
    SkillSourceResolutionFailure,
};
use skills_adapter::{
    enabled_catalog_response, installation_failure, management_response, mutation_response,
    set_enabled_response, SkillManagementFailure,
};
use skills_dispatcher::{mutation_admission_error_response, SkillMutationTarget, SkillsDispatcher};
use tokio::io::{self, AsyncBufRead, AsyncBufReadExt, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::sync::{mpsc, oneshot};

mod server;
use server::*;

fn main() -> io::Result<()> {
    if mycopilot_core::office::office_browser_proxy_mode_requested() {
        std::process::exit(mycopilot_core::office::run_office_browser_proxy());
    }
    // The production GitHub adapter owns reqwest's blocking client. Build and retain every
    // blocking dependency outside Tokio: reqwest deliberately panics when its blocking client is
    // constructed inside an async runtime, and its final drop joins an internal runtime thread.
    let bootstrap = CoreServerBootstrap::initialize()?;
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|error| {
            io::Error::other(format!("failed to initialize async runtime: {error}"))
        })?;
    let result = runtime.block_on(run_core_server(&bootstrap));

    // Keep the bootstrap owner alive until after Tokio has stopped so the blocking GitHub client
    // is also destroyed in a synchronous context. Runtime is declared after bootstrap, but the
    // explicit order documents and protects this lifecycle invariant.
    drop(runtime);
    drop(bootstrap);
    result
}

#[cfg(test)]
mod server_tests;
