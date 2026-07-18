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
    GIT_MUTATE_REVIEW_FILE_METHOD, SEARCH_SEARCH_CHATS_METHOD, SKILLS_CANCEL_PREPARATION_METHOD,
    SKILLS_CANCEL_SOURCE_RESOLUTION_METHOD, SKILLS_CHANGED_NOTIFICATION_METHOD,
    SKILLS_COMMIT_INSTALLATION_METHOD, SKILLS_INSPECT_INSTALLATION_METHOD,
    SKILLS_INSTALL_LOCAL_METHOD, SKILLS_LIST_MANAGEMENT_METHOD, SKILLS_LIST_METHOD,
    SKILLS_RESOLVE_INSTALLATION_SOURCE_METHOD, SKILLS_SET_ENABLED_METHOD, SKILLS_UNINSTALL_METHOD,
    SKILLS_UPDATE_LOCAL_METHOD, SKILL_INSPECTION_ERROR_CODE, SKILL_INSTALLATION_ERROR_CODE,
    SKILL_MANAGEMENT_ERROR_CODE, SKILL_MANAGEMENT_SCHEMA_VERSION,
    SKILL_SOURCE_RESOLUTION_ERROR_CODE, STORAGE_DELETE_CHAT_MESSAGES_METHOD,
    STORAGE_DELETE_CONVERSATION_METHOD, STORAGE_DELETE_PROJECT_METHOD,
    STORAGE_FORK_CONVERSATION_METHOD, STORAGE_LOAD_AGENT_PROMPT_PREFERENCES_METHOD,
    STORAGE_LOAD_ATTACHMENT_IMAGE_METHOD, STORAGE_LOAD_COMPOSER_DRAFTS_METHOD,
    STORAGE_LOAD_CONVERSATIONS_METHOD, STORAGE_LOAD_INPUT_ATTACHMENTS_METHOD,
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

fn main() -> io::Result<()> {
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

struct CoreServerBootstrap {
    storage: Arc<StorageService>,
    agent_service: AgentService,
    skill_services: SkillServices,
    git_review_service: Arc<GitReviewService>,
}

impl CoreServerBootstrap {
    fn initialize() -> io::Result<Self> {
        let database_path = absolute_path(database_path())?;
        let skill_store_root = skill_store_root(&database_path);
        let storage =
            Arc::new(StorageService::open(&database_path).map_err(|error| {
                io::Error::other(format!("failed to initialize storage: {error}"))
            })?);
        let git_review_service = Arc::new(GitReviewService::new());
        let skills_service = Arc::new(
            SkillsService::new()
                .with_bundled_source()
                .and_then(|service| service.with_installed_source(skill_store_root.clone()))
                .map_err(|error| {
                    io::Error::other(format!("failed to initialize Skills: {error}"))
                })?,
        );
        let skill_installation_service = Arc::new(
            SkillInstallationService::new(skill_store_root.clone()).map_err(|error| {
                io::Error::other(format!(
                    "failed to initialize Skill installation service: {error}"
                ))
            })?,
        );
        let mut skill_installation_workflow = SkillInstallationWorkflow::new(
            SkillInstallationService::new(skill_store_root).map_err(|error| {
                io::Error::other(format!(
                    "failed to initialize Skill installation workflow: {error}"
                ))
            })?,
        );
        let github_acquisition_transport: Arc<dyn GitHubAcquisitionTransport> =
            Arc::new(ReqwestGitHubTransport::new().map_err(|error| {
                io::Error::other(format!(
                    "failed to initialize public GitHub Skill transport: {error}"
                ))
            })?);
        let github_acquisition = GitHubWorkflowAcquisitionAdapter::new(Arc::new(
            GitHubSkillAcquirer::with_transport(github_acquisition_transport),
        ));
        skill_installation_workflow
            .register_adapter(Arc::new(github_acquisition))
            .map_err(|error| {
                io::Error::other(format!(
                    "failed to register GitHub Skill acquisition: {error}"
                ))
            })?;
        let mut skill_source_resolution = SkillSourceResolutionService::with_session_store(
            skill_installation_workflow.session_store(),
        );
        let github_resolution_transport: Arc<dyn GitHubAcquisitionTransport> = Arc::new(
            ReqwestGitHubTransport::new_for_source_resolution().map_err(|error| {
                io::Error::other(format!(
                    "failed to initialize GitHub Skill source resolution transport: {error}"
                ))
            })?,
        );
        skill_source_resolution
            .register_resolver(Arc::new(GitHubInstallationSourceResolver::new(
                github_resolution_transport,
            )))
            .map_err(|error| {
                io::Error::other(format!(
                    "failed to register GitHub Skill source resolver: {error}"
                ))
            })?;
        let agent_service =
            AgentService::new(storage.clone()).with_skills_service(Arc::clone(&skills_service));
        let skill_services = SkillServices {
            catalog: skills_service,
            installations: skill_installation_service,
            workflow: Arc::new(skill_installation_workflow),
            source_resolution: Arc::new(skill_source_resolution),
        };
        Ok(Self {
            storage,
            agent_service,
            skill_services,
            git_review_service,
        })
    }
}

async fn run_core_server(bootstrap: &CoreServerBootstrap) -> io::Result<()> {
    let (outbound_tx, outbound_rx) = mpsc::unbounded_channel::<Value>();
    let (finish_outbound_tx, finish_outbound_rx) = oneshot::channel();
    let writer = tokio::spawn(run_outbound_writer(
        io::stdout(),
        outbound_rx,
        finish_outbound_rx,
    ));
    let git_dispatcher = GitDispatcher::new(outbound_tx.clone());
    let skill_dispatcher = SkillsDispatcher::new(outbound_tx.clone());
    let skill_acquisition_dispatcher = SkillsDispatcher::new(outbound_tx.clone());
    let request_dispatchers = RequestDispatchers {
        git: &git_dispatcher,
        skills: &skill_dispatcher,
        skill_acquisition: &skill_acquisition_dispatcher,
    };

    let input_result = run_request_loop(
        BufReader::new(io::stdin()),
        Arc::clone(&bootstrap.storage),
        &bootstrap.agent_service,
        bootstrap.skill_services.clone(),
        Arc::clone(&bootstrap.git_review_service),
        &request_dispatchers,
        &outbound_tx,
    )
    .await;

    // Admission has stopped. Settle accepted filesystem jobs while active agents are cancelled in
    // parallel; queued jobs receive cancellation errors and running jobs get a bounded grace
    // period. The outbound writer remains live for every final response and notification.
    let (
        git_dispatcher_result,
        skill_dispatcher_result,
        skill_acquisition_dispatcher_result,
        (cancelled_runs, timed_out),
    ) = tokio::join!(
        git_dispatcher.shutdown(),
        skill_dispatcher.shutdown(),
        skill_acquisition_dispatcher.shutdown(),
        bootstrap
            .agent_service
            .shutdown_active_runs(Duration::from_secs(2))
    );

    let mut outbound_error = None;
    if let Ok(Some(shutdown_id)) = &input_result {
        if let Err(error) = enqueue_outbound(
            &outbound_tx,
            response_success(
                shutdown_id.clone(),
                CoreShutdownResponse {
                    cancelled_runs,
                    timed_out,
                },
            ),
        ) {
            outbound_error = Some(error);
        }
    }
    drop(outbound_tx);
    // A timed-out agent may still own an outbound sender. Tell the writer to close its receiver
    // and drain everything accepted so far instead of waiting for every producer clone to drop.
    // The shutdown response above is therefore flushed, while late notifications are rejected.
    let _ = finish_outbound_tx.send(());

    let writer_result = writer
        .await
        .map_err(|error| io::Error::other(format!("outbound writer stopped: {error}")))?;
    input_result?;
    git_dispatcher_result.map_err(|error| io::Error::other(error.to_string()))?;
    skill_dispatcher_result.map_err(|error| io::Error::other(error.to_string()))?;
    skill_acquisition_dispatcher_result.map_err(|error| io::Error::other(error.to_string()))?;
    if let Some(error) = outbound_error {
        return Err(error);
    }
    writer_result
}

struct RequestDispatchers<'a> {
    git: &'a GitDispatcher,
    skills: &'a SkillsDispatcher,
    skill_acquisition: &'a SkillsDispatcher,
}

#[derive(Clone)]
struct SkillServices {
    catalog: Arc<SkillsService>,
    installations: Arc<SkillInstallationService>,
    workflow: Arc<SkillInstallationWorkflow>,
    source_resolution: Arc<SkillSourceResolutionService>,
}

async fn run_request_loop<R>(
    input: R,
    storage: Arc<StorageService>,
    agent_service: &AgentService,
    skill_services: SkillServices,
    git_review_service: Arc<GitReviewService>,
    dispatchers: &RequestDispatchers<'_>,
    outbound: &mpsc::UnboundedSender<Value>,
) -> io::Result<Option<JsonRpcId>>
where
    R: AsyncBufRead + Unpin,
{
    let mut lines = input.lines();
    while let Some(line) = lines.next_line().await? {
        if line.trim().is_empty() {
            continue;
        }

        let request = match serde_json::from_str::<JsonRpcRequest>(&line) {
            Ok(request) => request,
            Err(error) => {
                enqueue_outbound(
                    outbound,
                    serde_json::to_value(mycopilot_protocol_rs::error(
                        None,
                        -32700,
                        format!("Parse error: {error}"),
                    ))
                    .expect("JSON-RPC parse error response must serialize"),
                )?;
                continue;
            }
        };

        if request.jsonrpc == "2.0" && request.method == CORE_SHUTDOWN_METHOD {
            return Ok(Some(request.id));
        }

        if request.jsonrpc == "2.0" {
            if let Some(priority) = git_request_priority(&request.method) {
                let request_id = request.id.clone();
                let request_storage = Arc::clone(&storage);
                let request_service = Arc::clone(&git_review_service);
                if let Err(error) =
                    dispatchers
                        .git
                        .try_submit(priority, request_id.clone(), move || {
                            handle_git_request(&request_storage, &request_service, request)
                        })
                {
                    enqueue_outbound(
                        outbound,
                        response_error(Some(request_id), error.code(), error.message()),
                    )?;
                }
                continue;
            }
            if is_skills_method(&request.method) {
                let request_id = request.id.clone();
                let request = match parse_skills_request(request) {
                    Ok(request) => request,
                    Err(response) => {
                        enqueue_outbound(outbound, response)?;
                        continue;
                    }
                };
                let request_storage = Arc::clone(&storage);
                let request_catalog = Arc::clone(&skill_services.catalog);
                let request_installations = Arc::clone(&skill_services.installations);
                let request_workflow = Arc::clone(&skill_services.workflow);
                let request_source_resolution = Arc::clone(&skill_services.source_resolution);
                let request_outbound = outbound.clone();
                let acquisition_lane = request.uses_acquisition_lane();
                let source_resolution_request = request.is_source_resolution();
                let workflow_metadata = request.workflow_metadata();
                let workflow_commit_preparation_id = request.workflow_commit_preparation_id();
                let mutation_metadata = request.mutation_metadata();
                let submit_result = if let Some(preparation_id) = workflow_commit_preparation_id {
                    dispatchers.skills.try_submit_workflow_commit(
                        request_id.clone(),
                        preparation_id,
                        move || {
                            handle_parsed_skills_request(
                                &request_storage,
                                &request_catalog,
                                &request_installations,
                                Some(&request_workflow),
                                Some(&request_source_resolution),
                                Some(&request_outbound),
                                request,
                            )
                        },
                    )
                } else {
                    match mutation_metadata.clone() {
                        Some((operation, target)) => dispatchers.skills.try_submit_mutation(
                            request_id.clone(),
                            operation,
                            target,
                            move || {
                                handle_parsed_skills_request(
                                    &request_storage,
                                    &request_catalog,
                                    &request_installations,
                                    Some(&request_workflow),
                                    Some(&request_source_resolution),
                                    Some(&request_outbound),
                                    request,
                                )
                            },
                        ),
                        None if acquisition_lane => dispatchers.skill_acquisition.try_submit(
                            request_id.clone(),
                            move || {
                                handle_parsed_skills_request(
                                    &request_storage,
                                    &request_catalog,
                                    &request_installations,
                                    Some(&request_workflow),
                                    Some(&request_source_resolution),
                                    Some(&request_outbound),
                                    request,
                                )
                            },
                        ),
                        None => dispatchers.skills.try_submit(request_id.clone(), move || {
                            handle_parsed_skills_request(
                                &request_storage,
                                &request_catalog,
                                &request_installations,
                                Some(&request_workflow),
                                Some(&request_source_resolution),
                                Some(&request_outbound),
                                request,
                            )
                        }),
                    }
                };
                if let Err(error) = submit_result {
                    let response = match mutation_metadata {
                        Some((operation, target)) => {
                            mutation_admission_error_response(request_id, operation, target, error)
                        }
                        None if workflow_metadata.is_some() => {
                            let (phase, preparation_id) =
                                workflow_metadata.expect("checked Skill workflow request metadata");
                            skill_inspection_error_response(
                                request_id,
                                dispatch_failure(phase, preparation_id, error.message()),
                            )
                        }
                        None if source_resolution_request => {
                            skill_source_resolution_error_response(
                                request_id,
                                resolution_dispatch_failure(error.message()),
                            )
                        }
                        None => response_error(Some(request_id), error.code(), error.message()),
                    };
                    enqueue_outbound(outbound, response)?;
                }
                continue;
            }
        }

        let response = handle_request(&storage, agent_service, outbound.clone(), request);
        enqueue_outbound(outbound, response)?;
    }
    Ok(None)
}

async fn run_outbound_writer<W>(
    mut writer: W,
    mut outbound: mpsc::UnboundedReceiver<Value>,
    mut finish: oneshot::Receiver<()>,
) -> io::Result<()>
where
    W: AsyncWrite + Unpin,
{
    loop {
        tokio::select! {
            biased;
            _ = &mut finish => {
                // Closing preserves already queued messages but prevents lingering agent tasks
                // from keeping shutdown open or appending notifications after the final response.
                outbound.close();
                while let Some(message) = outbound.recv().await {
                    write_outbound_message(&mut writer, message).await?;
                }
                return Ok(());
            }
            message = outbound.recv() => {
                let Some(message) = message else {
                    return Ok(());
                };
                write_outbound_message(&mut writer, message).await?;
            }
        }
    }
}

async fn write_outbound_message<W>(writer: &mut W, message: Value) -> io::Result<()>
where
    W: AsyncWrite + Unpin,
{
    writer.write_all(message.to_string().as_bytes()).await?;
    writer.write_all(b"\n").await?;
    writer.flush().await
}

fn enqueue_outbound(outbound: &mpsc::UnboundedSender<Value>, message: Value) -> io::Result<()> {
    outbound
        .send(message)
        .map_err(|_| io::Error::new(io::ErrorKind::BrokenPipe, "outbound writer is unavailable"))
}

fn git_request_priority(method: &str) -> Option<GitJobPriority> {
    match method {
        GIT_MUTATE_REVIEW_FILE_METHOD => Some(GitJobPriority::High),
        GIT_INSPECT_REPOSITORY_METHOD | GIT_GET_REVIEW_SUMMARY_METHOD => {
            Some(GitJobPriority::Medium)
        }
        GIT_GET_REVIEW_FILE_DIFF_METHOD | GIT_GET_REVIEW_FILE_CONTENT_METHOD => {
            Some(GitJobPriority::Low)
        }
        _ => None,
    }
}

fn is_skills_method(method: &str) -> bool {
    matches!(
        method,
        SKILLS_LIST_METHOD
            | SKILLS_INSPECT_INSTALLATION_METHOD
            | SKILLS_RESOLVE_INSTALLATION_SOURCE_METHOD
            | SKILLS_CANCEL_SOURCE_RESOLUTION_METHOD
            | SKILLS_COMMIT_INSTALLATION_METHOD
            | SKILLS_CANCEL_PREPARATION_METHOD
            | SKILLS_INSTALL_LOCAL_METHOD
            | SKILLS_UPDATE_LOCAL_METHOD
            | SKILLS_UNINSTALL_METHOD
            | SKILLS_LIST_MANAGEMENT_METHOD
            | SKILLS_SET_ENABLED_METHOD
    )
}

fn handle_request(
    storage: &StorageService,
    agent_service: &AgentService,
    notification_tx: agent::CoreServerNotificationSender,
    request: JsonRpcRequest,
) -> Value {
    if request.jsonrpc != "2.0" {
        return response_error(Some(request.id), -32600, "Invalid JSON-RPC version");
    }

    match request.method.as_str() {
        CORE_PING_METHOD => handle_core_ping(request.id, request.params),
        AGENT_START_CONVERSATION_TURN_METHOD => handle_agent_start_conversation_turn(
            agent_service,
            notification_tx,
            request.id,
            request.params,
        ),
        AGENT_GET_CONTEXT_WINDOW_SNAPSHOT_METHOD => {
            let input = match parse_params::<agent::AgentContextWindowSnapshotInput>(request.params)
            {
                Ok(input) => input,
                Err(message) => return response_error(Some(request.id), -32602, message),
            };
            match agent_service.get_context_window_snapshot(input) {
                Ok(output) => response_success(request.id, output),
                Err(error) => agent_service_error_response(request.id, error),
            }
        }
        AGENT_GET_CONTEXT_COMPACTION_AUDIT_METHOD => {
            let input =
                match parse_params::<agent::AgentContextCompactionAuditInput>(request.params) {
                    Ok(input) => input,
                    Err(message) => return response_error(Some(request.id), -32602, message),
                };
            match agent_service.get_context_compaction_audit(input) {
                Ok(output) => response_success(request.id, output),
                Err(message) => response_error(Some(request.id), -32000, message),
            }
        }
        AGENT_CANCEL_RUN_METHOD => {
            handle_agent_cancel_run(agent_service, request.id, request.params)
        }
        AGENT_LIST_PENDING_ACTIONS_METHOD => {
            response_success(request.id, agent_service.list_pending_actions())
        }
        AGENT_APPROVE_ACTION_METHOD => {
            handle_agent_approve_action(agent_service, notification_tx, request.id, request.params)
        }
        AGENT_REJECT_ACTION_METHOD => {
            handle_agent_reject_action(agent_service, notification_tx, request.id, request.params)
        }
        AGENT_CANCEL_ACTION_METHOD => {
            handle_agent_cancel_action(agent_service, request.id, request.params)
        }
        AGENT_GET_USAGE_SUMMARY_METHOD => {
            handle_agent_usage_summary(agent_service, request.id, request.params)
        }
        AGENT_CLEAR_USAGE_RECORDS_METHOD => {
            handle_agent_clear_usage_records(agent_service, request.id, request.params)
        }
        AGENT_READ_FILE_DRAFT_METHOD => {
            let input = match parse_params::<AgentFileDraftReadRequest>(request.params) {
                Ok(input) => input,
                Err(message) => return response_error(Some(request.id), -32602, message),
            };
            match agent_service.read_file_draft(&input.draft_id, input.offset, input.max_chars) {
                Ok(output) => response_success(request.id, output),
                Err(message) => response_error(Some(request.id), -32000, message),
            }
        }
        AGENT_GET_FILE_WRITE_DIFF_METHOD => {
            let input = match parse_params::<AgentFileDraftReadRequest>(request.params) {
                Ok(input) => input,
                Err(message) => return response_error(Some(request.id), -32602, message),
            };
            match agent_service.get_file_write_diff(&input.draft_id, input.offset, input.max_chars)
            {
                Ok(output) => response_success(request.id, output),
                Err(message) => response_error(Some(request.id), -32000, message),
            }
        }
        SEARCH_SEARCH_CHATS_METHOD => {
            let input = match parse_params::<ChatSearchInput>(request.params) {
                Ok(input) => input,
                Err(message) => return response_error(Some(request.id), -32602, message),
            };
            storage_response(request.id, storage.search_chats(&input))
        }
        STORAGE_LOAD_MODEL_SETTINGS_METHOD => {
            storage_response(request.id, storage.load_model_settings())
        }
        STORAGE_SAVE_MODEL_SETTINGS_METHOD => {
            let settings = match parse_params::<ModelSettingsRecord>(request.params) {
                Ok(settings) => settings,
                Err(message) => return response_error(Some(request.id), -32602, message),
            };
            let result = storage.save_model_settings(settings).map(|_| json!(null));
            if result.is_ok() {
                agent_service.invalidate_all_conversation_context_states();
            }
            storage_response(request.id, result)
        }
        STORAGE_LOAD_AGENT_PROMPT_PREFERENCES_METHOD => {
            storage_response(request.id, storage.load_agent_prompt_preferences())
        }
        STORAGE_SAVE_AGENT_PROMPT_PREFERENCES_METHOD => {
            let preferences = match parse_params::<AgentPromptPreferencesRecord>(request.params) {
                Ok(preferences) => preferences,
                Err(message) => return response_error(Some(request.id), -32602, message),
            };
            let result = storage.save_agent_prompt_preferences(preferences);
            if result.is_ok() {
                agent_service.invalidate_all_conversation_context_states();
            }
            storage_response(request.id, result)
        }
        STORAGE_LOAD_PROJECTS_METHOD => storage_response(request.id, storage.load_projects()),
        STORAGE_SAVE_PROJECT_METHOD => {
            let project = match parse_params::<ProjectRecord>(request.params) {
                Ok(project) => project,
                Err(message) => return response_error(Some(request.id), -32602, message),
            };
            storage_response(request.id, storage.save_project(project))
        }
        STORAGE_DELETE_PROJECT_METHOD => {
            let input = match parse_params::<ProjectIdRequest>(request.params) {
                Ok(input) => input,
                Err(message) => return response_error(Some(request.id), -32602, message),
            };
            storage_response(
                request.id,
                agent_service
                    .delete_project(&input.project_id)
                    .map(|_| json!(null)),
            )
        }
        STORAGE_LOAD_CONVERSATIONS_METHOD => {
            storage_response(request.id, storage.load_conversations())
        }
        STORAGE_FORK_CONVERSATION_METHOD => {
            let input = match parse_params::<ForkConversationInput>(request.params) {
                Ok(input) => input,
                Err(message) => return response_error(Some(request.id), -32602, message),
            };
            storage_response(request.id, storage.fork_conversation(input))
        }
        STORAGE_LOAD_ATTACHMENT_IMAGE_METHOD => {
            let input = match parse_params::<AttachmentIdRequest>(request.params) {
                Ok(input) => input,
                Err(message) => return response_error(Some(request.id), -32602, message),
            };
            storage_response(
                request.id,
                storage.load_attachment_image(&input.attachment_id),
            )
        }
        STORAGE_LOAD_INPUT_ATTACHMENTS_METHOD => {
            let input = match parse_params::<LoadInputAttachmentsRequest>(request.params) {
                Ok(input) => input,
                Err(message) => return response_error(Some(request.id), -32602, message),
            };
            storage_response(
                request.id,
                storage.load_input_attachments(&input.attachment_ids),
            )
        }
        STORAGE_SAVE_CONVERSATION_META_METHOD => {
            let conversation = match parse_params::<ChatConversationMetaRecord>(request.params) {
                Ok(conversation) => conversation,
                Err(message) => return response_error(Some(request.id), -32602, message),
            };
            storage_response(request.id, storage.save_conversation_meta(conversation))
        }
        STORAGE_DELETE_CONVERSATION_METHOD => {
            let input = match parse_params::<ConversationIdRequest>(request.params) {
                Ok(input) => input,
                Err(message) => return response_error(Some(request.id), -32602, message),
            };
            storage_response(
                request.id,
                agent_service
                    .delete_conversation(&input.conversation_id)
                    .map(|_| json!(null)),
            )
        }
        STORAGE_DELETE_CHAT_MESSAGES_METHOD => {
            let input = match parse_params::<DeleteChatMessagesRequest>(request.params) {
                Ok(input) => input,
                Err(message) => return response_error(Some(request.id), -32602, message),
            };
            storage_response(
                request.id,
                agent_service
                    .delete_chat_messages(&input.conversation_id, &input.message_ids)
                    .map(|_| json!(null)),
            )
        }
        STORAGE_UPSERT_CHAT_MESSAGES_METHOD => {
            let input = match parse_params::<UpsertChatMessagesRequest>(request.params) {
                Ok(input) => input,
                Err(message) => return response_error(Some(request.id), -32602, message),
            };
            storage_response(
                request.id,
                storage.upsert_chat_messages(
                    &input.conversation_id,
                    input.messages,
                    input.position_offset,
                ),
            )
        }
        STORAGE_SAVE_CHAT_MESSAGE_STATE_METHOD => {
            let input = match parse_params::<SaveChatMessageStateRequest>(request.params) {
                Ok(input) => input,
                Err(message) => return response_error(Some(request.id), -32602, message),
            };
            storage_response(
                request.id,
                storage
                    .save_chat_message_state(&input.conversation_id, input.message)
                    .map(|_| json!(null)),
            )
        }
        STORAGE_LOAD_COMPOSER_DRAFTS_METHOD => {
            storage_response(request.id, storage.load_composer_drafts())
        }
        STORAGE_SAVE_COMPOSER_DRAFT_METHOD => {
            let input = match parse_params::<SaveComposerDraftRequest>(request.params) {
                Ok(input) => input,
                Err(message) => return response_error(Some(request.id), -32602, message),
            };
            storage_response(request.id, storage.save_composer_draft(input.draft))
        }
        STORAGE_LOAD_UI_PREFERENCES_METHOD => {
            storage_response(request.id, storage.load_ui_preferences())
        }
        STORAGE_SAVE_UI_PREFERENCES_METHOD => {
            let preferences = match parse_params::<UiPreferencesRecord>(request.params) {
                Ok(preferences) => preferences,
                Err(message) => return response_error(Some(request.id), -32602, message),
            };
            storage_response(request.id, storage.save_ui_preferences(preferences))
        }
        _ => response_error(Some(request.id), -32601, "Method not found"),
    }
}

struct ParsedSkillsRequest {
    id: JsonRpcId,
    operation: ParsedSkillsOperation,
}

enum ParsedSkillsOperation {
    List(SkillsListRequest),
    ListManagement(SkillsListManagementRequest),
    SetEnabled(SkillsSetEnabledRequest),
    InspectInstallation(SkillsInspectInstallationRequest),
    ResolveInstallationSource(SkillsResolveInstallationSourceRequest),
    CancelSourceResolution(SkillsCancelSourceResolutionRequest),
    CommitInstallation(SkillsCommitInstallationRequest),
    CancelPreparation(SkillsCancelPreparationRequest),
    InstallLocal(LocalSkillInstallRequest),
    UpdateLocal(LocalSkillUpdateRequest),
    Uninstall(ParsedSkillUninstallRequest),
}

enum ParsedSkillUninstallRequest {
    Exact(SkillUninstallExactRequest),
    Legacy(SkillUninstallRequest),
}

impl ParsedSkillUninstallRequest {
    fn skill_id(&self) -> &SkillId {
        match self {
            Self::Exact(request) => request.skill_id(),
            Self::Legacy(request) => request.skill_id(),
        }
    }
}

impl ParsedSkillsRequest {
    fn uses_acquisition_lane(&self) -> bool {
        matches!(
            self.operation,
            ParsedSkillsOperation::InspectInstallation(_)
                | ParsedSkillsOperation::ResolveInstallationSource(_)
                | ParsedSkillsOperation::CancelSourceResolution(_)
        )
    }

    fn is_source_resolution(&self) -> bool {
        matches!(
            self.operation,
            ParsedSkillsOperation::ResolveInstallationSource(_)
                | ParsedSkillsOperation::CancelSourceResolution(_)
        )
    }

    fn workflow_metadata(
        &self,
    ) -> Option<(mycopilot_protocol_rs::SkillInspectionPhaseDto, String)> {
        match &self.operation {
            ParsedSkillsOperation::InspectInstallation(request) => Some((
                mycopilot_protocol_rs::SkillInspectionPhaseDto::Inspect,
                request.preparation_id.clone(),
            )),
            ParsedSkillsOperation::CommitInstallation(request) => Some((
                mycopilot_protocol_rs::SkillInspectionPhaseDto::Commit,
                request.preparation_id.clone(),
            )),
            ParsedSkillsOperation::CancelPreparation(request) => Some((
                mycopilot_protocol_rs::SkillInspectionPhaseDto::Cancel,
                request.preparation_id.clone(),
            )),
            _ => None,
        }
    }

    fn workflow_commit_preparation_id(&self) -> Option<String> {
        match &self.operation {
            ParsedSkillsOperation::CommitInstallation(request) => {
                Some(request.preparation_id.clone())
            }
            _ => None,
        }
    }

    fn mutation_metadata(&self) -> Option<(SkillInstallationOperation, SkillMutationTarget)> {
        match &self.operation {
            ParsedSkillsOperation::List(_)
            | ParsedSkillsOperation::ListManagement(_)
            | ParsedSkillsOperation::SetEnabled(_)
            | ParsedSkillsOperation::InspectInstallation(_)
            | ParsedSkillsOperation::ResolveInstallationSource(_)
            | ParsedSkillsOperation::CancelSourceResolution(_)
            | ParsedSkillsOperation::CommitInstallation(_)
            | ParsedSkillsOperation::CancelPreparation(_) => None,
            ParsedSkillsOperation::InstallLocal(request) => Some((
                SkillInstallationOperation::Install,
                SkillMutationTarget::InstallationId(request.installation_id().clone()),
            )),
            ParsedSkillsOperation::UpdateLocal(request) => Some((
                SkillInstallationOperation::Update,
                SkillMutationTarget::SkillId(request.skill_id().clone()),
            )),
            ParsedSkillsOperation::Uninstall(request) => Some((
                SkillInstallationOperation::Uninstall,
                SkillMutationTarget::SkillId(request.skill_id().clone()),
            )),
        }
    }
}

fn parse_skills_request(request: JsonRpcRequest) -> Result<ParsedSkillsRequest, Value> {
    let id = request.id;
    if request.jsonrpc != "2.0" {
        return Err(response_error(Some(id), -32600, "Invalid JSON-RPC version"));
    }

    let operation = match request.method.as_str() {
        SKILLS_LIST_METHOD => ParsedSkillsOperation::List(
            parse_params::<SkillsListRequest>(request.params)
                .map_err(|message| response_error(Some(id.clone()), -32602, message))?,
        ),
        SKILLS_LIST_MANAGEMENT_METHOD => ParsedSkillsOperation::ListManagement(
            parse_params::<SkillsListManagementRequest>(request.params)
                .map_err(|message| response_error(Some(id.clone()), -32602, message))?,
        ),
        SKILLS_SET_ENABLED_METHOD => ParsedSkillsOperation::SetEnabled(
            parse_params::<SkillsSetEnabledRequest>(request.params)
                .map_err(|message| response_error(Some(id.clone()), -32602, message))?,
        ),
        SKILLS_INSPECT_INSTALLATION_METHOD => ParsedSkillsOperation::InspectInstallation(
            parse_params::<SkillsInspectInstallationRequest>(request.params)
                .map_err(|message| response_error(Some(id.clone()), -32602, message))?,
        ),
        SKILLS_RESOLVE_INSTALLATION_SOURCE_METHOD => {
            ParsedSkillsOperation::ResolveInstallationSource(
                parse_params::<SkillsResolveInstallationSourceRequest>(request.params)
                    .map_err(|message| response_error(Some(id.clone()), -32602, message))?,
            )
        }
        SKILLS_CANCEL_SOURCE_RESOLUTION_METHOD => ParsedSkillsOperation::CancelSourceResolution(
            parse_params::<SkillsCancelSourceResolutionRequest>(request.params)
                .map_err(|message| response_error(Some(id.clone()), -32602, message))?,
        ),
        SKILLS_COMMIT_INSTALLATION_METHOD => ParsedSkillsOperation::CommitInstallation(
            parse_params::<SkillsCommitInstallationRequest>(request.params)
                .map_err(|message| response_error(Some(id.clone()), -32602, message))?,
        ),
        SKILLS_CANCEL_PREPARATION_METHOD => ParsedSkillsOperation::CancelPreparation(
            parse_params::<SkillsCancelPreparationRequest>(request.params)
                .map_err(|message| response_error(Some(id.clone()), -32602, message))?,
        ),
        SKILLS_INSTALL_LOCAL_METHOD => {
            let input = parse_params::<SkillsInstallLocalRequest>(request.params)
                .map_err(|message| response_error(Some(id.clone()), -32602, message))?;
            let installation_id = SkillInstallationId::parse(input.installation_id)
                .map_err(|error| invalid_skill_installation_params(&id, error.to_string()))?;
            let directory = parse_absolute_skill_directory(&id, input.directory)?;
            ParsedSkillsOperation::InstallLocal(LocalSkillInstallRequest::new(
                installation_id,
                directory,
            ))
        }
        SKILLS_UPDATE_LOCAL_METHOD => {
            let input = parse_params::<SkillsUpdateLocalRequest>(request.params)
                .map_err(|message| response_error(Some(id.clone()), -32602, message))?;
            let skill_id = SkillId::parse(input.skill_id)
                .map_err(|error| invalid_skill_installation_params(&id, error.to_string()))?;
            let expected_revision = SkillRevision::parse(input.expected_revision)
                .map_err(|error| invalid_skill_installation_params(&id, error.to_string()))?;
            let directory = parse_absolute_skill_directory(&id, input.directory)?;
            ParsedSkillsOperation::UpdateLocal(LocalSkillUpdateRequest::new(
                skill_id,
                expected_revision,
                directory,
            ))
        }
        SKILLS_UNINSTALL_METHOD => {
            let input = parse_params::<SkillsUninstallRequest>(request.params)
                .map_err(|message| response_error(Some(id.clone()), -32602, message))?;
            let skill_id = SkillId::parse(input.skill_id)
                .map_err(|error| invalid_skill_installation_params(&id, error.to_string()))?;
            let uninstall = if input
                .expected_revision
                .starts_with(SKILL_INSTALLATION_REVISION_PREFIX)
            {
                let expected_revision = SkillInstallationRevision::parse(input.expected_revision)
                    .map_err(|error| {
                    invalid_skill_installation_params(&id, error.to_string())
                })?;
                ParsedSkillUninstallRequest::Exact(SkillUninstallExactRequest::new(
                    skill_id,
                    expected_revision,
                ))
            } else {
                let expected_revision = SkillRevision::parse(input.expected_revision)
                    .map_err(|error| invalid_skill_installation_params(&id, error.to_string()))?;
                ParsedSkillUninstallRequest::Legacy(SkillUninstallRequest::new(
                    skill_id,
                    expected_revision,
                ))
            };
            ParsedSkillsOperation::Uninstall(uninstall)
        }
        _ => return Err(response_error(Some(id), -32601, "Method not found")),
    };
    Ok(ParsedSkillsRequest { id, operation })
}

fn invalid_skill_installation_params(id: &JsonRpcId, reason: String) -> Value {
    response_error(
        Some(id.clone()),
        -32602,
        format!("Invalid params: {reason}"),
    )
}

fn parse_absolute_skill_directory(id: &JsonRpcId, value: String) -> Result<PathBuf, Value> {
    let directory = PathBuf::from(value);
    if !directory.is_absolute() {
        return Err(invalid_skill_installation_params(
            id,
            "Skill directory must be an absolute path".to_string(),
        ));
    }
    Ok(directory)
}

#[cfg(test)]
fn handle_skills_request(
    storage: &StorageService,
    skills_service: &SkillsService,
    skill_installation_service: &SkillInstallationService,
    request: JsonRpcRequest,
) -> Value {
    match parse_skills_request(request) {
        Ok(request) => handle_parsed_skills_request(
            storage,
            skills_service,
            skill_installation_service,
            None,
            None,
            None,
            request,
        ),
        Err(response) => response,
    }
}

fn handle_parsed_skills_request(
    storage: &StorageService,
    skills_service: &SkillsService,
    skill_installation_service: &SkillInstallationService,
    skill_installation_workflow: Option<&SkillInstallationWorkflow>,
    skill_source_resolution: Option<&SkillSourceResolutionService>,
    notification_tx: Option<&mpsc::UnboundedSender<Value>>,
    request: ParsedSkillsRequest,
) -> Value {
    match request.operation {
        ParsedSkillsOperation::List(input) => {
            let result = match input.project_id.as_deref() {
                Some(project_id) => {
                    resolve_project_path(storage, project_id).and_then(|workspace| {
                        skills_service
                            .list_with_workspace(project_id, &workspace)
                            .map_err(|error| error.to_string())
                    })
                }
                None => skills_service.list().map_err(|error| error.to_string()),
            };
            match result.and_then(|catalog| enabled_catalog_response(storage, &catalog)) {
                Ok(catalog) => response_success(request.id, catalog),
                Err(message) => response_error(Some(request.id), -32000, message),
            }
        }
        ParsedSkillsOperation::ListManagement(input) => {
            // Reserved for a future project-scoped management extension. The
            // settings-page inventory is global in schema v1.
            let _ = input.project_id;
            let result = skills_service
                .list()
                .map_err(|_| SkillManagementFailure::list_unavailable())
                .and_then(|catalog| {
                    management_response(
                        storage,
                        &catalog,
                        skill_installation_service,
                        skill_installation_workflow,
                    )
                });
            match result {
                Ok(response) => response_success(request.id, response),
                Err(error) => skill_management_error_response(request.id, error),
            }
        }
        ParsedSkillsOperation::SetEnabled(input) => {
            let result = skills_service
                .list()
                .map_err(|_| SkillManagementFailure::set_enabled_unavailable())
                .and_then(|catalog| {
                    set_enabled_response(
                        storage,
                        &catalog,
                        skill_installation_service,
                        skill_installation_workflow,
                        &input,
                    )
                });
            match result {
                Ok((response, changed)) => {
                    if changed {
                        notify_skills_changed(
                            storage,
                            skills_service,
                            skill_installation_service,
                            skill_installation_workflow,
                            notification_tx,
                            SkillsChangedReasonDto::EnablementChanged,
                            Some(input.skill_id),
                        );
                    }
                    response_success(request.id, response)
                }
                Err(error) => skill_management_error_response(request.id, error),
            }
        }
        ParsedSkillsOperation::InspectInstallation(input) => {
            let Some(workflow) = skill_installation_workflow else {
                return response_error(
                    Some(request.id),
                    -32603,
                    "Skill installation workflow is unavailable.",
                );
            };
            let result = preparation_request(input)
                .and_then(|request| {
                    workflow.inspect(&request).map_err(|error| {
                        workflow_failure(
                            mycopilot_protocol_rs::SkillInspectionPhaseDto::Inspect,
                            &error,
                        )
                    })
                })
                .and_then(|preview| preview_response(&preview));
            match result {
                Ok(preview) => response_success(request.id, preview),
                Err(error) => skill_inspection_error_response(request.id, error),
            }
        }
        ParsedSkillsOperation::ResolveInstallationSource(input) => {
            let Some(service) = skill_source_resolution else {
                return skill_source_resolution_error_response(
                    request.id,
                    resolution_dispatch_failure("Skill source resolution is unavailable."),
                );
            };
            let result = resolution_request(input)
                .and_then(|(resolution_id, locator)| {
                    service
                        .resolve_registered(resolution_id, &locator)
                        .map_err(resolution_failure)
                })
                .and_then(|registered| resolution_response(&registered));
            match result {
                Ok(response) => response_success(request.id, response),
                Err(error) => skill_source_resolution_error_response(request.id, error),
            }
        }
        ParsedSkillsOperation::CancelSourceResolution(input) => {
            let Some(service) = skill_source_resolution else {
                return skill_source_resolution_error_response(
                    request.id,
                    resolution_dispatch_failure("Skill source resolution is unavailable."),
                );
            };
            let result = cancellation_resolution_id(input).and_then(|resolution_id| {
                let outcome = service
                    .cancel_registered_resolution(&resolution_id)
                    .map_err(resolution_failure)?;
                source_resolution_cancellation_response(&resolution_id, outcome)
            });
            match result {
                Ok(response) => response_success(request.id, response),
                Err(error) => skill_source_resolution_error_response(request.id, error),
            }
        }
        ParsedSkillsOperation::CommitInstallation(input) => {
            let Some(workflow) = skill_installation_workflow else {
                return response_error(
                    Some(request.id),
                    -32603,
                    "Skill installation workflow is unavailable.",
                );
            };
            let result = commit_request(input).and_then(|commit| {
                workflow.commit(&commit).map_err(|error| {
                    workflow_failure(
                        mycopilot_protocol_rs::SkillInspectionPhaseDto::Commit,
                        &error,
                    )
                })
            });
            match result.and_then(|result| {
                let response = commit_response(&result)?;
                if !result.replayed() {
                    let reason = match result.mutation().operation() {
                        SkillInstallationOperation::Install => {
                            Some(SkillsChangedReasonDto::Installed)
                        }
                        SkillInstallationOperation::Update => Some(SkillsChangedReasonDto::Updated),
                        _ => None,
                    };
                    if let Some(reason) = reason {
                        notify_skills_changed(
                            storage,
                            skills_service,
                            skill_installation_service,
                            skill_installation_workflow,
                            notification_tx,
                            reason,
                            Some(result.mutation().skill_id().as_str().to_string()),
                        );
                    }
                }
                Ok(response)
            }) {
                Ok(response) => response_success(request.id, response),
                Err(error) => skill_workflow_commit_error_response_with_invalidation(
                    storage,
                    skills_service,
                    skill_installation_service,
                    skill_installation_workflow,
                    notification_tx,
                    request.id,
                    error,
                ),
            }
        }
        ParsedSkillsOperation::CancelPreparation(input) => {
            let Some(workflow) = skill_installation_workflow else {
                return response_error(
                    Some(request.id),
                    -32603,
                    "Skill installation workflow is unavailable.",
                );
            };
            let preparation_id = match cancellation_preparation_id(input) {
                Ok(preparation_id) => preparation_id,
                Err(error) => return skill_inspection_error_response(request.id, error),
            };
            match workflow.cancel(&preparation_id) {
                Ok(outcome) => {
                    response_success(request.id, cancellation_response(&preparation_id, outcome))
                }
                Err(error) if is_missing_preparation(&error) => {
                    response_success(request.id, absent_cancellation_response(&preparation_id))
                }
                Err(error) => skill_inspection_error_response(
                    request.id,
                    workflow_failure(
                        mycopilot_protocol_rs::SkillInspectionPhaseDto::Cancel,
                        &error,
                    ),
                ),
            }
        }
        ParsedSkillsOperation::InstallLocal(input) => {
            let result = skill_installation_service.install_local_directory(&input);
            if matches!(
                result.as_ref().map(SkillInstallationMutation::outcome),
                Ok(mycopilot_core::skills::SkillInstallationOutcome::Installed)
            ) {
                let skill_id = result
                    .as_ref()
                    .expect("matched successful installation")
                    .skill_id()
                    .as_str()
                    .to_string();
                notify_skills_changed(
                    storage,
                    skills_service,
                    skill_installation_service,
                    skill_installation_workflow,
                    notification_tx,
                    SkillsChangedReasonDto::Installed,
                    Some(skill_id),
                );
            }
            skill_mutation_response_with_invalidation(
                storage,
                skills_service,
                skill_installation_service,
                skill_installation_workflow,
                notification_tx,
                request.id,
                result,
            )
        }
        ParsedSkillsOperation::UpdateLocal(input) => {
            let result = skill_installation_service.update_local_directory(&input);
            if matches!(
                result.as_ref().map(SkillInstallationMutation::outcome),
                Ok(mycopilot_core::skills::SkillInstallationOutcome::Updated)
            ) {
                notify_skills_changed(
                    storage,
                    skills_service,
                    skill_installation_service,
                    skill_installation_workflow,
                    notification_tx,
                    SkillsChangedReasonDto::Updated,
                    Some(input.skill_id().as_str().to_string()),
                );
            }
            skill_mutation_response_with_invalidation(
                storage,
                skills_service,
                skill_installation_service,
                skill_installation_workflow,
                notification_tx,
                request.id,
                result,
            )
        }
        ParsedSkillsOperation::Uninstall(input) => {
            let result = match &input {
                ParsedSkillUninstallRequest::Exact(request) => {
                    skill_installation_service.uninstall_exact(request)
                }
                ParsedSkillUninstallRequest::Legacy(request) => {
                    skill_installation_service.uninstall(request)
                }
            };
            if result.is_ok() {
                // The managed-store mutation is the authoritative commit. SQLite is
                // a derived preference store: cleanup must never turn a
                // completed uninstall into a false failure. Idempotent
                // uninstall retries repeat the same reconciliation.
                best_effort_delete_skill_enablement(storage, input.skill_id().as_str());
            }
            if matches!(
                result.as_ref().map(SkillInstallationMutation::outcome),
                Ok(mycopilot_core::skills::SkillInstallationOutcome::Uninstalled)
            ) {
                notify_skills_changed(
                    storage,
                    skills_service,
                    skill_installation_service,
                    skill_installation_workflow,
                    notification_tx,
                    SkillsChangedReasonDto::Uninstalled,
                    Some(input.skill_id().as_str().to_string()),
                );
            }
            skill_mutation_response_with_invalidation(
                storage,
                skills_service,
                skill_installation_service,
                skill_installation_workflow,
                notification_tx,
                request.id,
                result,
            )
        }
    }
}

fn best_effort_delete_skill_enablement(storage: &StorageService, skill_id: &str) {
    // Uninstalled installation IDs are durably retired and cannot be reused,
    // so deleting their derived preference row cannot create state-token ABA.
    let _ = storage.delete_skill_enablement_override(skill_id);
}

fn skill_mutation_response_with_invalidation(
    storage: &StorageService,
    skills_service: &SkillsService,
    skill_installation_service: &SkillInstallationService,
    skill_installation_workflow: Option<&SkillInstallationWorkflow>,
    notification_tx: Option<&mpsc::UnboundedSender<Value>>,
    id: JsonRpcId,
    result: Result<SkillInstallationMutation, SkillInstallationServiceError>,
) -> Value {
    if let Err(error) = &result {
        if let Ok(failure) = installation_failure(error) {
            notify_skills_changed_if_commit_outcome_uncertain(
                storage,
                skills_service,
                skill_installation_service,
                skill_installation_workflow,
                notification_tx,
                failure.commit_may_have_succeeded(),
                failure.skill_id(),
            );
        }
    }
    skill_mutation_response(id, result)
}

fn skill_workflow_commit_error_response_with_invalidation(
    storage: &StorageService,
    skills_service: &SkillsService,
    skill_installation_service: &SkillInstallationService,
    skill_installation_workflow: Option<&SkillInstallationWorkflow>,
    notification_tx: Option<&mpsc::UnboundedSender<Value>>,
    id: JsonRpcId,
    failure: SkillInspectionFailure,
) -> Value {
    notify_skills_changed_if_commit_outcome_uncertain(
        storage,
        skills_service,
        skill_installation_service,
        skill_installation_workflow,
        notification_tx,
        failure.commit_may_have_succeeded(),
        failure.skill_id(),
    );
    skill_inspection_error_response(id, failure)
}

fn notify_skills_changed_if_commit_outcome_uncertain(
    storage: &StorageService,
    skills_service: &SkillsService,
    skill_installation_service: &SkillInstallationService,
    skill_installation_workflow: Option<&SkillInstallationWorkflow>,
    notification_tx: Option<&mpsc::UnboundedSender<Value>>,
    commit_may_have_succeeded: bool,
    skill_id: Option<&str>,
) {
    if !commit_may_have_succeeded {
        return;
    }
    // This is an invalidation, not a success claim. The durable store must be
    // re-read because a post-publication fsync failure cannot prove whether the
    // requested mutation became visible.
    notify_skills_changed(
        storage,
        skills_service,
        skill_installation_service,
        skill_installation_workflow,
        notification_tx,
        SkillsChangedReasonDto::CatalogChanged,
        skill_id.map(str::to_string),
    );
}

fn notify_skills_changed(
    storage: &StorageService,
    skills_service: &SkillsService,
    skill_installation_service: &SkillInstallationService,
    skill_installation_workflow: Option<&SkillInstallationWorkflow>,
    notification_tx: Option<&mpsc::UnboundedSender<Value>>,
    reason: SkillsChangedReasonDto,
    skill_id: Option<String>,
) {
    let Some(notification_tx) = notification_tx else {
        return;
    };
    let management_revision = skills_service
        .list()
        .ok()
        .and_then(|catalog| {
            management_response(
                storage,
                &catalog,
                skill_installation_service,
                skill_installation_workflow,
            )
            .ok()
        })
        .map(|response| response.management_revision)
        .unwrap_or_else(|| "unavailable".to_string());
    let notification = SkillsChangedNotification {
        schema_version: SKILL_MANAGEMENT_SCHEMA_VERSION,
        management_revision,
        reason,
        skill_id,
    };
    let _ = notification_tx.send(json!({
        "jsonrpc": "2.0",
        "method": SKILLS_CHANGED_NOTIFICATION_METHOD,
        "params": notification,
    }));
}

fn skill_mutation_response(
    id: JsonRpcId,
    result: Result<SkillInstallationMutation, SkillInstallationServiceError>,
) -> Value {
    match result {
        Ok(mutation) => match mutation_response(&mutation) {
            Ok(response) => response_success(id, response),
            Err(message) => response_error(Some(id), -32603, message),
        },
        Err(error) => match installation_failure(&error) {
            Ok(failure) => {
                let message = failure.to_string();
                serde_json::to_value(error_with_data(
                    Some(id),
                    SKILL_INSTALLATION_ERROR_CODE,
                    message,
                    serde_json::to_value(failure.into_data())
                        .expect("Skill installation error data must serialize"),
                ))
                .expect("JSON-RPC Skill installation error response must serialize")
            }
            Err(mapping_error) => response_error(Some(id), -32603, mapping_error),
        },
    }
}

fn skill_management_error_response(id: JsonRpcId, failure: SkillManagementFailure) -> Value {
    let message = failure.to_string();
    serde_json::to_value(error_with_data(
        Some(id),
        SKILL_MANAGEMENT_ERROR_CODE,
        message,
        serde_json::to_value(failure.into_data())
            .expect("Skill management error data must serialize"),
    ))
    .expect("JSON-RPC Skill management error response must serialize")
}

fn skill_inspection_error_response(id: JsonRpcId, failure: SkillInspectionFailure) -> Value {
    let message = failure.to_string();
    serde_json::to_value(error_with_data(
        Some(id),
        SKILL_INSPECTION_ERROR_CODE,
        message,
        serde_json::to_value(failure.into_data())
            .expect("Skill inspection error data must serialize"),
    ))
    .expect("JSON-RPC Skill inspection error response must serialize")
}

fn skill_source_resolution_error_response(
    id: JsonRpcId,
    failure: SkillSourceResolutionFailure,
) -> Value {
    let message = failure.to_string();
    serde_json::to_value(error_with_data(
        Some(id),
        SKILL_SOURCE_RESOLUTION_ERROR_CODE,
        message,
        serde_json::to_value(failure.into_data())
            .expect("Skill source resolution error data must serialize"),
    ))
    .expect("JSON-RPC Skill source resolution error response must serialize")
}

fn handle_git_request(
    storage: &StorageService,
    git_review_service: &GitReviewService,
    request: JsonRpcRequest,
) -> Value {
    match request.method.as_str() {
        GIT_INSPECT_REPOSITORY_METHOD => {
            let input = match parse_params::<GitRepositoryInspectRequest>(request.params) {
                Ok(input) => input,
                Err(message) => return response_error(Some(request.id), -32602, message),
            };
            let project_path = match resolve_project_path(storage, &input.project_id) {
                Ok(path) => path,
                Err(message) => return response_error(Some(request.id), -32000, message),
            };
            response_success(
                request.id,
                git_review_service.inspect_repository(&input.project_id, &project_path),
            )
        }
        GIT_GET_REVIEW_SUMMARY_METHOD => {
            let input = match parse_params::<GitReviewSummaryRequest>(request.params) {
                Ok(input) => input,
                Err(message) => return response_error(Some(request.id), -32602, message),
            };
            let scope = match GitReviewScope::parse(&input.scope) {
                Ok(scope) => scope,
                Err(message) => return response_error(Some(request.id), -32602, message),
            };
            let project_path = match resolve_project_path(storage, &input.project_id) {
                Ok(path) => path,
                Err(message) => return response_error(Some(request.id), -32000, message),
            };
            match git_review_service.review_summary(&project_path, scope) {
                Ok(summary) => response_success(request.id, summary),
                Err(message) => response_error(Some(request.id), -32000, message),
            }
        }
        GIT_GET_REVIEW_FILE_DIFF_METHOD => {
            let input = match parse_params::<GitReviewFileDiffRequest>(request.params) {
                Ok(input) => input,
                Err(message) => return response_error(Some(request.id), -32602, message),
            };
            match git_review_service.review_file_diff(&input.snapshot_id, &input.file_id) {
                Ok(diff) => response_success(request.id, diff),
                Err(message) => response_error(Some(request.id), -32000, message),
            }
        }
        GIT_GET_REVIEW_FILE_CONTENT_METHOD => {
            let input = match parse_params::<GitReviewFileContentRequest>(request.params) {
                Ok(input) => input,
                Err(message) => return response_error(Some(request.id), -32602, message),
            };
            match git_review_service.review_file_content(&input.snapshot_id, &input.file_id) {
                Ok(content) => response_success(request.id, content),
                Err(message) => response_error(Some(request.id), -32000, message),
            }
        }
        GIT_MUTATE_REVIEW_FILE_METHOD => {
            let input = match parse_params::<GitReviewFileMutationRequest>(request.params) {
                Ok(input) => input,
                Err(message) => return response_error(Some(request.id), -32602, message),
            };
            let action = match GitReviewFileMutationAction::parse(&input.action) {
                Ok(action) => action,
                Err(message) => return response_error(Some(request.id), -32602, message),
            };
            match git_review_service.mutate_review_file(&input.snapshot_id, &input.file_id, action)
            {
                Ok(mutation) => response_success(request.id, mutation),
                Err(message) => response_error(Some(request.id), -32000, message),
            }
        }
        _ => response_error(Some(request.id), -32601, "Method not found"),
    }
}

fn resolve_project_path(storage: &StorageService, project_id: &str) -> Result<PathBuf, String> {
    let project = storage
        .load_projects()?
        .into_iter()
        .find(|project| project.id == project_id)
        .ok_or_else(|| "The selected project no longer exists.".to_string())?;
    let path = project
        .path
        .filter(|path| !path.trim().is_empty())
        .ok_or_else(|| "The selected project does not have a local directory.".to_string())?;
    Ok(PathBuf::from(path))
}

fn handle_core_ping(id: JsonRpcId, params: Option<Value>) -> Value {
    let input =
        parse_params::<CorePingRequest>(params).unwrap_or(CorePingRequest { message: None });
    response_success(
        id,
        CorePingResponse {
            message: "pong",
            echo: input.message,
            server_time_ms: now_ms(),
        },
    )
}

fn handle_agent_start_conversation_turn(
    agent_service: &AgentService,
    notification_tx: agent::CoreServerNotificationSender,
    id: JsonRpcId,
    params: Option<Value>,
) -> Value {
    let input = match parse_params::<AgentConversationTurnInput>(params) {
        Ok(input) => input,
        Err(message) => return response_error(Some(id), -32602, message),
    };

    match agent_service.start_conversation_turn(input, notification_tx) {
        Ok(output) => response_success(id, output),
        Err(error) => agent_service_error_response(id, error),
    }
}

fn handle_agent_cancel_run(
    agent_service: &AgentService,
    id: JsonRpcId,
    params: Option<Value>,
) -> Value {
    let input = match parse_params::<AgentCancelRunRequest>(params) {
        Ok(input) => input,
        Err(message) => return response_error(Some(id), -32602, message),
    };

    let cancelled = agent_service.cancel_run(&input.run_id);
    response_success(
        id,
        AgentCancelRunResponse {
            run_id: input.run_id,
            cancelled,
        },
    )
}

fn handle_agent_approve_action(
    agent_service: &AgentService,
    notification_tx: agent::CoreServerNotificationSender,
    id: JsonRpcId,
    params: Option<Value>,
) -> Value {
    let input = match parse_params::<AgentActionIdRequest>(params) {
        Ok(input) => input,
        Err(message) => return response_error(Some(id), -32602, message),
    };

    match agent_service.approve_action(&input.action_id, notification_tx) {
        Ok(output) => response_success(id, output),
        Err(message) => response_error(Some(id), -32000, message),
    }
}

fn handle_agent_reject_action(
    agent_service: &AgentService,
    notification_tx: agent::CoreServerNotificationSender,
    id: JsonRpcId,
    params: Option<Value>,
) -> Value {
    let input = match parse_params::<AgentRejectActionRequest>(params) {
        Ok(input) => input,
        Err(message) => return response_error(Some(id), -32602, message),
    };

    match agent_service.reject_action(&input.action_id, input.message, notification_tx) {
        Ok(output) => response_success(id, output),
        Err(message) => response_error(Some(id), -32000, message),
    }
}

fn handle_agent_cancel_action(
    agent_service: &AgentService,
    id: JsonRpcId,
    params: Option<Value>,
) -> Value {
    let input = match parse_params::<AgentActionIdRequest>(params) {
        Ok(input) => input,
        Err(message) => return response_error(Some(id), -32602, message),
    };

    match agent_service.cancel_action(&input.action_id) {
        Ok(cancelled) => response_success(id, cancelled),
        Err(message) => response_error(Some(id), -32000, message),
    }
}

fn handle_agent_usage_summary(
    agent_service: &AgentService,
    id: JsonRpcId,
    params: Option<Value>,
) -> Value {
    let input = match parse_params::<AgentUsageSummaryInput>(params) {
        Ok(input) => input,
        Err(message) => return response_error(Some(id), -32602, message),
    };

    match agent_service.get_usage_summary(&input) {
        Ok(summary) => response_success(id, summary),
        Err(message) => response_error(Some(id), -32000, message),
    }
}

fn handle_agent_clear_usage_records(
    agent_service: &AgentService,
    id: JsonRpcId,
    params: Option<Value>,
) -> Value {
    let input = match parse_params::<AgentUsageClearInput>(params) {
        Ok(input) => input,
        Err(message) => return response_error(Some(id), -32602, message),
    };

    match agent_service.clear_usage_records(&input) {
        Ok(output) => response_success(id, output),
        Err(message) => response_error(Some(id), -32000, message),
    }
}

fn parse_params<T>(params: Option<Value>) -> Result<T, String>
where
    T: for<'de> serde::Deserialize<'de>,
{
    serde_json::from_value(params.unwrap_or(Value::Null))
        .map_err(|err| format!("Invalid params: {err}"))
}

fn response_success<T>(id: JsonRpcId, result: T) -> Value
where
    T: Serialize,
{
    serde_json::to_value(success(id, result)).expect("JSON-RPC success response must serialize")
}

fn response_error(id: Option<JsonRpcId>, code: i64, message: impl Into<String>) -> Value {
    serde_json::to_value(error(id, code, message)).expect("JSON-RPC error response must serialize")
}

fn agent_service_error_response(id: JsonRpcId, error: AgentServiceError) -> Value {
    match error.skill_activation() {
        Some(data) => serde_json::to_value(error_with_data(
            Some(id),
            -32000,
            error.message(),
            serde_json::to_value(data).expect("Skill activation error data must serialize"),
        ))
        .expect("JSON-RPC error response must serialize"),
        None => response_error(Some(id), -32000, error.message()),
    }
}

fn storage_response<T>(id: JsonRpcId, result: Result<T, String>) -> Value
where
    T: Serialize,
{
    match result {
        Ok(value) => response_success(id, value),
        Err(message) => response_error(Some(id), -32000, message),
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProjectIdRequest {
    project_id: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ConversationIdRequest {
    conversation_id: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AttachmentIdRequest {
    attachment_id: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct LoadInputAttachmentsRequest {
    attachment_ids: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DeleteChatMessagesRequest {
    conversation_id: String,
    message_ids: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct UpsertChatMessagesRequest {
    conversation_id: String,
    messages: Vec<ChatMessageRecord>,
    position_offset: i64,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SaveChatMessageStateRequest {
    conversation_id: String,
    message: ChatMessageStateRecord,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SaveComposerDraftRequest {
    draft: ComposerDraftRecord,
}

fn now_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

fn database_path() -> PathBuf {
    if let Ok(path) = std::env::var("MYCOPILOT_STORAGE_DB") {
        return PathBuf::from(path);
    }

    if cfg!(target_os = "macos") {
        return home_dir()
            .join("Library")
            .join("Application Support")
            .join("mycopilot-next")
            .join("storage.sqlite");
    }

    if cfg!(target_os = "windows") {
        if let Ok(app_data) = std::env::var("APPDATA") {
            return PathBuf::from(app_data)
                .join("mycopilot-next")
                .join("storage.sqlite");
        }
    }

    std::env::var("XDG_DATA_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| home_dir().join(".local").join("share"))
        .join("mycopilot-next")
        .join("storage.sqlite")
}

fn skill_store_root(database_path: &std::path::Path) -> PathBuf {
    database_path
        .parent()
        .map(|parent| parent.join("skills"))
        .unwrap_or_else(|| PathBuf::from("skills"))
}

fn absolute_path(path: PathBuf) -> io::Result<PathBuf> {
    if path.is_absolute() {
        Ok(path)
    } else {
        std::env::current_dir().map(|current_directory| current_directory.join(path))
    }
}

fn home_dir() -> PathBuf {
    std::env::var("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("."))
}

#[cfg(test)]
mod server_tests {
    use super::*;
    use crate::skills_test_support::write_installed_skill;
    use mycopilot_core::skills::{
        GitHubReference, PreparedSkillAcquisition, PreparedSkillPackage,
        PreparedSkillSourceResolution, PreparedSkillSourceResolutionCandidate,
        ResolvedSkillPackagePreview, ResolvedSkillSource, SkillInstallationAuthority,
        SkillInstallationProvenance, SkillInstallationRefresh, SkillInstallationSourceLocator,
        SkillInstallationSourceResolver, SkillPackageOrigin, SkillSourceCandidateId,
        SkillSourceResolution, SkillSourceResolutionCandidate, SkillSourceResolutionError,
        SkillSourceResolverId, GITHUB_SKILL_ORIGIN_PROVIDER,
    };
    use std::fs;
    use std::sync::{mpsc as std_mpsc, Mutex};
    use tokio::io::AsyncReadExt;

    struct StaticGitHubSourceResolver;

    impl SkillInstallationSourceResolver for StaticGitHubSourceResolver {
        fn id(&self) -> SkillSourceResolverId {
            SkillSourceResolverId::parse("github").unwrap()
        }

        fn supported_hosts(&self) -> Vec<String> {
            vec!["github.com".to_string()]
        }

        fn resolve(
            &self,
            _locator: &SkillInstallationSourceLocator,
        ) -> Result<SkillSourceResolution, SkillSourceResolutionError> {
            let commit = "0123456789abcdef0123456789abcdef01234567";
            SkillSourceResolution::new(
                "https://github.com/example/skills/tree/main/skills/auditor",
                self.id(),
                commit,
                vec![SkillSourceResolutionCandidate::new(
                    "candidate-auditor",
                    ResolvedSkillSource::GitHub {
                        owner: "example".to_string(),
                        repository: "skills".to_string(),
                        tracking_reference: GitHubReference::named("main").unwrap(),
                        resolved_commit: commit.to_string(),
                        subdirectory: Some("skills/auditor".to_string()),
                    },
                    ResolvedSkillPackagePreview::new(
                        1,
                        SkillRevision::parse(format!("sha256:{}", "a".repeat(64))).unwrap(),
                        "auditor",
                        "Audit a repository.",
                        1,
                        128,
                    ),
                )],
            )
        }

        fn resolve_prepared(
            &self,
            _locator: &SkillInstallationSourceLocator,
        ) -> Result<PreparedSkillSourceResolution, SkillSourceResolutionError> {
            let commit = "0123456789abcdef0123456789abcdef01234567";
            let package = PreparedSkillPackage::from_bytes(
                concat!(
                    "---\n",
                    "name: auditor\n",
                    "description: Audit a repository.\n",
                    "---\n",
                    "# Instructions\n",
                    "Audit the repository.\n"
                )
                .as_bytes()
                .to_vec(),
                SkillPackageOrigin::new(
                    GITHUB_SKILL_ORIGIN_PROVIDER,
                    serde_json::json!({
                        "schemaVersion": 1,
                        "owner": "example",
                        "repository": "skills",
                        "requestedReference": { "kind": "named", "value": "main" },
                        "resolvedCommit": commit,
                        "subdirectory": "skills/auditor"
                    })
                    .to_string(),
                )
                .unwrap(),
            )
            .unwrap();
            let provenance = SkillInstallationProvenance::new(
                SkillInstallationAuthority::new(
                    GITHUB_SKILL_ORIGIN_PROVIDER,
                    1,
                    serde_json::json!({
                        "owner": "example",
                        "repository": "skills",
                        "resolvedCommit": commit,
                        "subdirectory": "skills/auditor"
                    })
                    .to_string(),
                )
                .unwrap(),
                Some(
                    SkillInstallationRefresh::new(
                        GITHUB_SKILL_ORIGIN_PROVIDER,
                        1,
                        serde_json::json!({
                            "owner": "example",
                            "repository": "skills",
                            "reference": { "kind": "named", "value": "main" },
                            "subdirectory": "skills/auditor"
                        })
                        .to_string(),
                    )
                    .unwrap(),
                ),
            );
            PreparedSkillSourceResolution::new(
                "https://github.com/example/skills/tree/main/skills/auditor",
                self.id(),
                commit,
                vec![PreparedSkillSourceResolutionCandidate::new(
                    SkillSourceCandidateId::parse("candidate-auditor").unwrap(),
                    ResolvedSkillSource::GitHub {
                        owner: "example".to_string(),
                        repository: "skills".to_string(),
                        tracking_reference: GitHubReference::named("main").unwrap(),
                        resolved_commit: commit.to_string(),
                        subdirectory: Some("skills/auditor".to_string()),
                    },
                    PreparedSkillAcquisition::new(package, provenance),
                )],
            )
        }
    }

    struct BlockingGitHubSourceResolver {
        started: std_mpsc::Sender<()>,
        release: Mutex<std_mpsc::Receiver<()>>,
    }

    impl SkillInstallationSourceResolver for BlockingGitHubSourceResolver {
        fn id(&self) -> SkillSourceResolverId {
            StaticGitHubSourceResolver.id()
        }

        fn supported_hosts(&self) -> Vec<String> {
            StaticGitHubSourceResolver.supported_hosts()
        }

        fn resolve(
            &self,
            locator: &SkillInstallationSourceLocator,
        ) -> Result<SkillSourceResolution, SkillSourceResolutionError> {
            StaticGitHubSourceResolver.resolve(locator)
        }

        fn resolve_prepared(
            &self,
            locator: &SkillInstallationSourceLocator,
        ) -> Result<PreparedSkillSourceResolution, SkillSourceResolutionError> {
            self.started.send(()).unwrap();
            self.release.lock().unwrap().recv().unwrap();
            StaticGitHubSourceResolver.resolve_prepared(locator)
        }
    }

    #[test]
    fn installed_skill_store_is_a_sibling_of_the_effective_database() {
        assert_eq!(
            skill_store_root(std::path::Path::new("profile/storage.sqlite")),
            std::path::Path::new("profile/skills")
        );
    }

    #[test]
    fn relative_database_override_is_absolutized_before_source_registration() {
        let current_directory = std::env::current_dir().unwrap();
        let database_path = absolute_path(PathBuf::from("profile/storage.sqlite")).unwrap();

        assert_eq!(
            database_path,
            current_directory.join("profile/storage.sqlite")
        );
        assert_eq!(
            skill_store_root(&database_path),
            current_directory.join("profile/skills")
        );
    }

    #[test]
    fn agent_skill_failures_preserve_structured_json_rpc_recovery_data() {
        let response = agent_service_error_response(
            JsonRpcId::Number(9),
            AgentServiceError::from(skills_adapter::missing_workspace_failure()),
        );

        assert_eq!(response["error"]["code"], -32000);
        assert_eq!(response["error"]["data"]["type"], "skillActivation");
        assert_eq!(response["error"]["data"]["code"], "invalidSelection");
        assert_eq!(response["error"]["data"]["recovery"], "rejectSelection");
    }

    #[test]
    fn skills_list_resolves_the_project_and_returns_camel_case_catalog() {
        let temp = tempfile::tempdir().unwrap();
        let workspace = temp.path().join("workspace");
        let skill_directory = workspace
            .join(".agents")
            .join("skills")
            .join("repository-evidence-auditor");
        fs::create_dir_all(&skill_directory).unwrap();
        fs::write(
            skill_directory.join("SKILL.md"),
            concat!(
                "---\n",
                "name: repository-evidence-auditor\n",
                "description: Inspect a repository using source evidence.\n",
                "---\n",
                "# Instructions\n"
            ),
        )
        .unwrap();
        let broken_skill_directory = workspace.join(".agents").join("skills").join("broken");
        fs::create_dir(&broken_skill_directory).unwrap();
        fs::write(
            broken_skill_directory.join("SKILL.md"),
            "missing frontmatter",
        )
        .unwrap();
        let storage = Arc::new(StorageService::open(&temp.path().join("storage.sqlite")).unwrap());
        storage
            .save_project(ProjectRecord {
                id: "project-1".to_string(),
                name: "Workspace".to_string(),
                path: Some(workspace.to_string_lossy().into_owned()),
                created_at: 1,
                pinned_at: None,
            })
            .unwrap();
        let installed_store_root = temp.path().join("skills");
        let skills_service = SkillsService::new()
            .with_bundled_source()
            .and_then(|service| service.with_installed_source(&installed_store_root))
            .unwrap();
        let skill_installation_service =
            SkillInstallationService::new(&installed_store_root).unwrap();
        assert!(
            !installed_store_root.exists(),
            "registering the read-only source must not create its store"
        );
        let empty_installed_catalog = skills_service.list().unwrap();
        assert_eq!(empty_installed_catalog.skills().len(), 1);
        assert!(empty_installed_catalog.diagnostics().is_empty());
        assert!(!installed_store_root.exists());
        const INSTALLATION_ID: &str = "0190b0f2-7c50-7cc0-8b25-3bb80f08b334";
        write_installed_skill(
            &installed_store_root,
            INSTALLATION_ID,
            concat!(
                "---\n",
                "name: installed-repository-auditor\n",
                "description: Inspect a repository using an installed Skill.\n",
                "---\n",
                "# Instructions\n",
                "Audit repository evidence.\n"
            ),
        );
        let request = serde_json::from_value::<JsonRpcRequest>(json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": SKILLS_LIST_METHOD,
            "params": { "projectId": "project-1" }
        }))
        .unwrap();

        let response = handle_skills_request(
            &storage,
            &skills_service,
            &skill_installation_service,
            request,
        );

        assert_eq!(response["id"], 1);
        let skills = response["result"]["skills"].as_array().unwrap();
        assert_eq!(skills.len(), 3);
        assert_eq!(response["result"]["schemaVersion"], 4);
        let workspace_skill = skills
            .iter()
            .find(|skill| skill["id"] == "workspace:project-1:repository-evidence-auditor")
            .unwrap();
        let bundled_skill = skills
            .iter()
            .find(|skill| skill["id"] == "bundled:application:repository-evidence-auditor")
            .unwrap();
        let installed_skill = skills
            .iter()
            .find(|skill| skill["id"] == format!("installed:user:{INSTALLATION_ID}"))
            .unwrap();
        assert_eq!(workspace_skill["source"]["kind"], "workspace");
        assert_eq!(workspace_skill["source"]["id"], "workspace:project-1");
        assert_eq!(workspace_skill["trust"], "untrusted");
        assert_eq!(workspace_skill["activationScope"], "run");
        assert_eq!(
            workspace_skill["description"],
            "Inspect a repository using source evidence."
        );
        assert_eq!(
            workspace_skill["location"],
            ".agents/skills/repository-evidence-auditor/SKILL.md"
        );
        assert!(workspace_skill.get("path").is_none());
        assert!(workspace_skill["revision"].is_string());
        assert_eq!(bundled_skill["source"]["kind"], "bundled");
        assert_eq!(bundled_skill["source"]["id"], "bundled:application");
        assert_eq!(bundled_skill["trust"], "application");
        assert_eq!(bundled_skill["activationScope"], "run");
        assert_eq!(
            bundled_skill["location"],
            "repository-evidence-auditor/SKILL.md"
        );
        assert!(bundled_skill["revision"].is_string());
        assert_eq!(installed_skill["source"]["kind"], "installed");
        assert_eq!(installed_skill["source"]["id"], "installed:user");
        assert_eq!(installed_skill["trust"], "untrusted");
        assert_eq!(installed_skill["activationScope"], "run");
        assert!(installed_skill["location"]
            .as_str()
            .is_some_and(
                |location| location.starts_with("packages/v1/") && location.ends_with("/SKILL.md")
            ));
        assert!(installed_skill["revision"].is_string());
        assert!(response["result"]["catalogRevision"].is_string());
        assert_eq!(response["result"]["truncated"], false);
        assert_eq!(
            response["result"]["diagnostics"][0]["code"],
            "missingFrontmatter"
        );
        assert_eq!(response["result"]["diagnostics"][0]["severity"], "error");
        assert!(response["result"]["diagnostics"][0]["message"].is_string());
        assert!(response["result"]["diagnostics"][0]["location"].is_string());
    }

    #[test]
    fn skills_list_without_a_project_returns_only_global_sources() {
        let temp = tempfile::tempdir().unwrap();
        let storage = Arc::new(StorageService::open(&temp.path().join("storage.sqlite")).unwrap());
        let skills_service = SkillsService::new().with_bundled_source().unwrap();
        let skill_installation_service =
            SkillInstallationService::new(temp.path().join("skills")).unwrap();
        let request = serde_json::from_value::<JsonRpcRequest>(json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": SKILLS_LIST_METHOD,
            "params": {}
        }))
        .unwrap();

        let response = handle_skills_request(
            &storage,
            &skills_service,
            &skill_installation_service,
            request,
        );

        assert_eq!(response["result"]["schemaVersion"], 4);
        assert_eq!(response["result"]["skills"].as_array().unwrap().len(), 1);
        assert_eq!(response["result"]["skills"][0]["source"]["kind"], "bundled");
    }

    #[test]
    fn management_enablement_is_cas_protected_and_filters_only_the_picker() {
        let temp = tempfile::tempdir().unwrap();
        let storage = Arc::new(StorageService::open(&temp.path().join("storage.sqlite")).unwrap());
        let skills_service = SkillsService::new().with_bundled_source().unwrap();
        let skill_installation_service =
            SkillInstallationService::new(temp.path().join("skills")).unwrap();
        let list_management = || {
            handle_skills_request(
                &storage,
                &skills_service,
                &skill_installation_service,
                serde_json::from_value::<JsonRpcRequest>(json!({
                    "jsonrpc": "2.0",
                    "id": 1,
                    "method": SKILLS_LIST_MANAGEMENT_METHOD,
                    "params": {}
                }))
                .unwrap(),
            )
        };

        let first = list_management();
        let skill_id = first["result"]["skills"][0]["id"]
            .as_str()
            .unwrap()
            .to_string();
        let first_state = first["result"]["skills"][0]["stateRevision"]
            .as_str()
            .unwrap()
            .to_string();
        assert_eq!(first["result"]["skills"][0]["enabled"], true);
        assert_eq!(
            first["result"]["skills"][0]["actions"]["canUninstall"],
            false
        );

        let disable = handle_skills_request(
            &storage,
            &skills_service,
            &skill_installation_service,
            serde_json::from_value::<JsonRpcRequest>(json!({
                "jsonrpc": "2.0",
                "id": 2,
                "method": SKILLS_SET_ENABLED_METHOD,
                "params": {
                    "skillId": skill_id,
                    "expectedStateRevision": first_state.clone(),
                    "enabled": false
                }
            }))
            .unwrap(),
        );
        assert_eq!(disable["result"]["outcome"], "updated");
        assert_eq!(disable["result"]["enabled"], false);
        let disabled_state = disable["result"]["stateRevision"]
            .as_str()
            .unwrap()
            .to_string();

        let lost_response_retry = handle_skills_request(
            &storage,
            &skills_service,
            &skill_installation_service,
            serde_json::from_value::<JsonRpcRequest>(json!({
                "jsonrpc": "2.0",
                "id": 20,
                "method": SKILLS_SET_ENABLED_METHOD,
                "params": {
                    "skillId": skill_id,
                    "expectedStateRevision": first_state,
                    "enabled": false
                }
            }))
            .unwrap(),
        );
        assert_eq!(lost_response_retry["result"]["outcome"], "alreadyCurrent");
        assert_eq!(
            lost_response_retry["result"]["stateRevision"],
            disabled_state
        );

        let picker = handle_skills_request(
            &storage,
            &skills_service,
            &skill_installation_service,
            serde_json::from_value::<JsonRpcRequest>(json!({
                "jsonrpc": "2.0",
                "id": 3,
                "method": SKILLS_LIST_METHOD,
                "params": {}
            }))
            .unwrap(),
        );
        assert!(picker["result"]["skills"].as_array().unwrap().is_empty());
        let management = list_management();
        assert_eq!(management["result"]["skills"][0]["enabled"], false);

        let idempotent = handle_skills_request(
            &storage,
            &skills_service,
            &skill_installation_service,
            serde_json::from_value::<JsonRpcRequest>(json!({
                "jsonrpc": "2.0",
                "id": 4,
                "method": SKILLS_SET_ENABLED_METHOD,
                "params": {
                    "skillId": skill_id,
                    "expectedStateRevision": disabled_state,
                    "enabled": false
                }
            }))
            .unwrap(),
        );
        assert_eq!(idempotent["result"]["outcome"], "alreadyCurrent");

        let stale = handle_skills_request(
            &storage,
            &skills_service,
            &skill_installation_service,
            serde_json::from_value::<JsonRpcRequest>(json!({
                "jsonrpc": "2.0",
                "id": 5,
                "method": SKILLS_SET_ENABLED_METHOD,
                "params": {
                    "skillId": skill_id,
                    "expectedStateRevision": "stale",
                    "enabled": true
                }
            }))
            .unwrap(),
        );
        assert_eq!(stale["error"]["code"], -32012);
        assert_eq!(stale["error"]["data"]["type"], "skillManagement");
        assert_eq!(stale["error"]["data"]["operation"], "setEnabled");
        assert_eq!(stale["error"]["data"]["code"], "stateConflict");
        assert_eq!(stale["error"]["data"]["recovery"], "refreshManagement");

        let reenabled = handle_skills_request(
            &storage,
            &skills_service,
            &skill_installation_service,
            serde_json::from_value::<JsonRpcRequest>(json!({
                "jsonrpc": "2.0",
                "id": 6,
                "method": SKILLS_SET_ENABLED_METHOD,
                "params": {
                    "skillId": skill_id,
                    "expectedStateRevision": disabled_state,
                    "enabled": true
                }
            }))
            .unwrap(),
        );
        assert_eq!(reenabled["result"]["outcome"], "updated");
        assert_ne!(
            reenabled["result"]["stateRevision"], first_state,
            "monotonic enablement generation must prevent boolean-state ABA"
        );
    }

    #[test]
    fn set_enabled_distinguishes_missing_and_project_scoped_skills() {
        let temp = tempfile::tempdir().unwrap();
        let storage = StorageService::open(&temp.path().join("storage.sqlite")).unwrap();
        let skills_service = SkillsService::new().with_bundled_source().unwrap();
        let skill_installation_service =
            SkillInstallationService::new(temp.path().join("skills")).unwrap();
        let invoke = |skill_id: &str| {
            handle_skills_request(
                &storage,
                &skills_service,
                &skill_installation_service,
                serde_json::from_value::<JsonRpcRequest>(json!({
                    "jsonrpc": "2.0",
                    "id": 1,
                    "method": SKILLS_SET_ENABLED_METHOD,
                    "params": {
                        "skillId": skill_id,
                        "expectedStateRevision": "unknown",
                        "enabled": false
                    }
                }))
                .unwrap(),
            )
        };

        let missing = invoke("installed:user:01234567-89ab-4def-8123-456789abcdef");
        assert_eq!(missing["error"]["code"], SKILL_MANAGEMENT_ERROR_CODE);
        assert_eq!(missing["error"]["data"]["code"], "notFound");
        assert_eq!(missing["error"]["data"]["operation"], "setEnabled");

        let workspace = invoke("workspace:project-1:auditor");
        assert_eq!(workspace["error"]["code"], SKILL_MANAGEMENT_ERROR_CODE);
        assert_eq!(workspace["error"]["data"]["code"], "notManageable");
        assert_eq!(workspace["error"]["data"]["recovery"], "refreshManagement");
    }

    #[test]
    fn skills_list_rejects_an_unknown_project_with_a_server_error() {
        let temp = tempfile::tempdir().unwrap();
        let storage = StorageService::open(&temp.path().join("storage.sqlite")).unwrap();
        let skill_installation_service =
            SkillInstallationService::new(temp.path().join("skills")).unwrap();
        let request = serde_json::from_value::<JsonRpcRequest>(json!({
            "jsonrpc": "2.0",
            "id": "skills-request",
            "method": SKILLS_LIST_METHOD,
            "params": { "projectId": "missing" }
        }))
        .unwrap();

        let response = handle_skills_request(
            &storage,
            &SkillsService::new(),
            &skill_installation_service,
            request,
        );

        assert_eq!(response["id"], "skills-request");
        assert_eq!(response["error"]["code"], -32000);
        assert_eq!(
            response["error"]["message"],
            "The selected project no longer exists."
        );
    }

    #[test]
    fn source_resolution_cancellation_is_routed_and_idempotent() {
        let temp = tempfile::tempdir().unwrap();
        let storage = StorageService::open(&temp.path().join("storage.sqlite")).unwrap();
        let catalog = SkillsService::new();
        let installations = SkillInstallationService::new(temp.path().join("skills")).unwrap();
        let workflow = SkillInstallationWorkflow::new(
            SkillInstallationService::new(temp.path().join("skills")).unwrap(),
        );
        let mut source_resolution =
            SkillSourceResolutionService::with_session_store(workflow.session_store());
        source_resolution
            .register_resolver(Arc::new(StaticGitHubSourceResolver))
            .unwrap();
        let resolution_id = "11111111-1111-4111-8111-111111111111";

        let resolve = parse_skills_request(
            serde_json::from_value::<JsonRpcRequest>(json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": SKILLS_RESOLVE_INSTALLATION_SOURCE_METHOD,
                "params": {
                    "resolutionId": resolution_id,
                    "locator": {
                        "kind": "url",
                        "url": "https://github.com/example/skills"
                    }
                }
            }))
            .unwrap(),
        )
        .unwrap();
        let resolved = handle_parsed_skills_request(
            &storage,
            &catalog,
            &installations,
            Some(&workflow),
            Some(&source_resolution),
            None,
            resolve,
        );
        assert_eq!(resolved["result"]["resolutionId"], resolution_id);

        let cancel = |id, resolution_id: &str| {
            let request = parse_skills_request(
                serde_json::from_value::<JsonRpcRequest>(json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "method": SKILLS_CANCEL_SOURCE_RESOLUTION_METHOD,
                    "params": { "resolutionId": resolution_id }
                }))
                .unwrap(),
            )
            .unwrap();
            handle_parsed_skills_request(
                &storage,
                &catalog,
                &installations,
                Some(&workflow),
                Some(&source_resolution),
                None,
                request,
            )
        };

        let first = cancel(2, resolution_id);
        assert_eq!(first["result"]["outcome"], "cancelled");
        assert_eq!(first["result"]["resolutionId"], resolution_id);
        assert_eq!(
            cancel(3, resolution_id)["result"]["outcome"],
            "alreadyCancelled"
        );
        assert_eq!(
            cancel(4, "99999999-9999-4999-8999-999999999999")["result"]["outcome"],
            "alreadyAbsent"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn queued_source_cancellation_cannot_overtake_the_resolution_it_fences() {
        let temp = tempfile::tempdir().unwrap();
        let storage = Arc::new(StorageService::open(&temp.path().join("storage.sqlite")).unwrap());
        let agent_service = AgentService::new(Arc::clone(&storage));
        let (outbound_tx, mut outbound_rx) = mpsc::unbounded_channel();
        let git_dispatcher = GitDispatcher::new(outbound_tx.clone());
        let skills_dispatcher = SkillsDispatcher::new(outbound_tx.clone());
        let acquisition_dispatcher = SkillsDispatcher::new(outbound_tx.clone());
        let dispatchers = RequestDispatchers {
            git: &git_dispatcher,
            skills: &skills_dispatcher,
            skill_acquisition: &acquisition_dispatcher,
        };
        let workflow = SkillInstallationWorkflow::new(
            SkillInstallationService::new(temp.path().join("skills")).unwrap(),
        );
        let (started_tx, started_rx) = std_mpsc::channel();
        let (release_tx, release_rx) = std_mpsc::channel();
        let mut source_resolution =
            SkillSourceResolutionService::with_session_store(workflow.session_store());
        source_resolution
            .register_resolver(Arc::new(BlockingGitHubSourceResolver {
                started: started_tx,
                release: Mutex::new(release_rx),
            }))
            .unwrap();
        let resolution_id = "11111111-1111-4111-8111-111111111111";
        let input = format!(
            concat!(
                "{{\"jsonrpc\":\"2.0\",\"id\":81,",
                "\"method\":\"skills.resolveInstallationSource\",",
                "\"params\":{{\"resolutionId\":\"{resolution_id}\",",
                "\"locator\":{{\"kind\":\"url\",",
                "\"url\":\"https://github.com/example/skills\"}}}}}}\n",
                "{{\"jsonrpc\":\"2.0\",\"id\":82,",
                "\"method\":\"skills.cancelSourceResolution\",",
                "\"params\":{{\"resolutionId\":\"{resolution_id}\"}}}}\n"
            ),
            resolution_id = resolution_id,
        );

        let shutdown_id = run_request_loop(
            BufReader::new(input.as_bytes()),
            Arc::clone(&storage),
            &agent_service,
            SkillServices {
                catalog: Arc::new(SkillsService::new()),
                installations: Arc::new(
                    SkillInstallationService::new(temp.path().join("skills")).unwrap(),
                ),
                workflow: Arc::new(workflow),
                source_resolution: Arc::new(source_resolution),
            },
            Arc::new(GitReviewService::new()),
            &dispatchers,
            &outbound_tx,
        )
        .await
        .unwrap();
        assert!(shutdown_id.is_none());
        started_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("resolution must enter the acquisition lane");

        assert!(
            tokio::time::timeout(Duration::from_millis(100), outbound_rx.recv())
                .await
                .is_err(),
            "cancellation must wait behind the earlier accepted resolution"
        );
        release_tx.send(()).unwrap();
        let resolved = tokio::time::timeout(Duration::from_secs(2), outbound_rx.recv())
            .await
            .unwrap()
            .unwrap();
        let cancelled = tokio::time::timeout(Duration::from_secs(2), outbound_rx.recv())
            .await
            .unwrap()
            .unwrap();

        assert_eq!(resolved["id"], 81);
        assert_eq!(resolved["result"]["outcome"], "resolved");
        assert_eq!(cancelled["id"], 82);
        assert_eq!(cancelled["result"]["outcome"], "cancelled");

        git_dispatcher.shutdown().await.unwrap();
        skills_dispatcher.shutdown().await.unwrap();
        acquisition_dispatcher.shutdown().await.unwrap();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn request_loop_resolves_and_hands_off_a_candidate_on_one_acquisition_lane() {
        let temp = tempfile::tempdir().unwrap();
        let storage = Arc::new(StorageService::open(&temp.path().join("storage.sqlite")).unwrap());
        let agent_service = AgentService::new(Arc::clone(&storage));
        let (outbound_tx, mut outbound_rx) = mpsc::unbounded_channel();
        let git_dispatcher = GitDispatcher::new(outbound_tx.clone());
        let skills_dispatcher = SkillsDispatcher::new(outbound_tx.clone());
        let acquisition_dispatcher = SkillsDispatcher::new(outbound_tx.clone());
        let dispatchers = RequestDispatchers {
            git: &git_dispatcher,
            skills: &skills_dispatcher,
            skill_acquisition: &acquisition_dispatcher,
        };
        let workflow = SkillInstallationWorkflow::new(
            SkillInstallationService::new(temp.path().join("skills")).unwrap(),
        );
        let mut source_resolution =
            SkillSourceResolutionService::with_session_store(workflow.session_store());
        source_resolution
            .register_resolver(Arc::new(StaticGitHubSourceResolver))
            .unwrap();
        let input = concat!(
            "{\"jsonrpc\":\"2.0\",\"id\":71,",
            "\"method\":\"skills.resolveInstallationSource\",",
            "\"params\":{\"resolutionId\":",
            "\"11111111-1111-4111-8111-111111111111\",",
            "\"locator\":{\"kind\":\"url\",",
            "\"url\":\"https://github.com/example/skills\"}}}\n",
            "{\"jsonrpc\":\"2.0\",\"id\":72,",
            "\"method\":\"skills.inspectInstallation\",",
            "\"params\":{\"preparationId\":",
            "\"33333333-3333-4333-8333-333333333333\",",
            "\"intent\":{\"operation\":\"install\"},",
            "\"source\":{\"kind\":\"resolvedCandidate\",",
            "\"resolutionId\":\"11111111-1111-4111-8111-111111111111\",",
            "\"candidateId\":\"candidate-auditor\"}}}\n"
        );

        let shutdown_id = run_request_loop(
            BufReader::new(input.as_bytes()),
            Arc::clone(&storage),
            &agent_service,
            SkillServices {
                catalog: Arc::new(SkillsService::new()),
                installations: Arc::new(
                    SkillInstallationService::new(temp.path().join("skills")).unwrap(),
                ),
                workflow: Arc::new(workflow),
                source_resolution: Arc::new(source_resolution),
            },
            Arc::new(GitReviewService::new()),
            &dispatchers,
            &outbound_tx,
        )
        .await
        .unwrap();
        let first = tokio::time::timeout(Duration::from_secs(2), outbound_rx.recv())
            .await
            .expect("source resolution must complete")
            .expect("source resolution must produce a response");
        let second = tokio::time::timeout(Duration::from_secs(2), outbound_rx.recv())
            .await
            .expect("candidate inspection must complete")
            .expect("candidate inspection must produce a response");
        let responses = [first, second];
        let response = responses
            .iter()
            .find(|response| response["id"] == 71)
            .expect("source resolution response");
        let preview = responses
            .iter()
            .find(|response| response["id"] == 72)
            .expect("candidate inspection response");

        assert!(shutdown_id.is_none());
        assert_eq!(response["id"], 71);
        assert_eq!(
            response["result"]["resolutionId"],
            "11111111-1111-4111-8111-111111111111"
        );
        assert!(response["result"]["expiresAtUnixMs"].as_u64().unwrap() > 0);
        assert_eq!(response["result"]["outcome"], "resolved");
        assert_eq!(
            response["result"]["candidates"][0]["source"]["reference"]["kind"],
            "named"
        );
        assert_eq!(
            response["result"]["candidates"][0]["source"]["reference"]["value"],
            "main"
        );
        assert_eq!(
            response["result"]["candidates"][0]["source"]["resolvedCommit"],
            "0123456789abcdef0123456789abcdef01234567"
        );
        assert_eq!(
            response["result"]["candidates"][0]["acquisition"],
            json!({
                "kind": "resolvedCandidate",
                "resolutionId": "11111111-1111-4111-8111-111111111111",
                "candidateId": "candidate-auditor"
            })
        );
        assert!(
            preview.get("error").is_none(),
            "candidate inspection failed: {preview}"
        );
        assert_eq!(
            preview["result"]["preparationId"],
            "33333333-3333-4333-8333-333333333333"
        );
        assert_eq!(preview["result"]["package"]["name"], "auditor");

        git_dispatcher.shutdown().await.unwrap();
        skills_dispatcher.shutdown().await.unwrap();
        acquisition_dispatcher.shutdown().await.unwrap();
    }

    #[test]
    fn source_resolution_rejects_invalid_resolution_ids_as_invalid_params() {
        let request = serde_json::from_value::<JsonRpcRequest>(json!({
            "jsonrpc": "2.0",
            "id": 72,
            "method": SKILLS_RESOLVE_INSTALLATION_SOURCE_METHOD,
            "params": {
                "resolutionId": "00000000-0000-0000-0000-000000000000",
                "locator": {
                    "kind": "url",
                    "url": "https://github.com/example/skills"
                }
            }
        }))
        .unwrap();

        let response = match parse_skills_request(request) {
            Ok(_) => panic!("a nil resolution ID must not cross the protocol boundary"),
            Err(response) => response,
        };

        assert_eq!(response["id"], 72);
        assert_eq!(response["error"]["code"], -32602);
        assert!(response["error"]["message"]
            .as_str()
            .unwrap()
            .contains("canonical non-nil lower-case UUID"));
    }

    #[test]
    fn uninstall_rejects_a_malformed_exact_revision_without_legacy_fallback() {
        let request = serde_json::from_value::<JsonRpcRequest>(json!({
            "jsonrpc": "2.0",
            "id": 73,
            "method": SKILLS_UNINSTALL_METHOD,
            "params": {
                "skillId": "installed:user:0190b0f2-7c50-7cc0-8b25-3bb80f08b336",
                "expectedRevision": format!("{SKILL_INSTALLATION_REVISION_PREFIX}malformed")
            }
        }))
        .unwrap();

        let response = match parse_skills_request(request) {
            Ok(_) => panic!("malformed installation revisions must fail closed"),
            Err(response) => response,
        };

        assert_eq!(response["id"], 73);
        assert_eq!(response["error"]["code"], -32602);
        assert!(response["error"]["message"]
            .as_str()
            .unwrap()
            .contains("installation revision"));
    }

    #[test]
    fn source_resolution_rejects_unknown_hosts_with_structured_recovery_data() {
        let temp = tempfile::tempdir().unwrap();
        let storage = StorageService::open(&temp.path().join("storage.sqlite")).unwrap();
        let installations = SkillInstallationService::new(temp.path().join("skills")).unwrap();
        let service = SkillSourceResolutionService::new();
        let request = serde_json::from_value::<JsonRpcRequest>(json!({
            "jsonrpc": "2.0",
            "id": 72,
            "method": SKILLS_RESOLVE_INSTALLATION_SOURCE_METHOD,
            "params": {
                "resolutionId": "22222222-2222-4222-8222-222222222222",
                "locator": {
                    "kind": "url",
                    "url": "https://untrusted.example/private?token=secret"
                }
            }
        }))
        .unwrap();
        let parsed = parse_skills_request(request).unwrap();

        let response = handle_parsed_skills_request(
            &storage,
            &SkillsService::new(),
            &installations,
            None,
            Some(&service),
            None,
            parsed,
        );

        assert_eq!(
            response["error"]["code"],
            SKILL_SOURCE_RESOLUTION_ERROR_CODE
        );
        assert_eq!(response["error"]["data"]["type"], "skillSourceResolution");
        assert_eq!(response["error"]["data"]["phase"], "parse");
        assert_eq!(response["error"]["data"]["code"], "unsupportedHost");
        assert_eq!(
            response["error"]["data"]["recovery"],
            "chooseDifferentSource"
        );
        assert!(!response.to_string().contains("token=secret"));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn request_loop_routes_skills_list_through_the_bounded_dispatcher() {
        let temp = tempfile::tempdir().unwrap();
        let workspace = temp.path().join("workspace");
        let skill_directory = workspace.join(".agents").join("skills").join("auditor");
        fs::create_dir_all(&skill_directory).unwrap();
        fs::write(
            skill_directory.join("SKILL.md"),
            "---\nname: auditor\ndescription: Audit a repository.\n---\n# Instructions\n",
        )
        .unwrap();
        let storage = Arc::new(StorageService::open(&temp.path().join("storage.sqlite")).unwrap());
        storage
            .save_project(ProjectRecord {
                id: "project-1".to_string(),
                name: "Workspace".to_string(),
                path: Some(workspace.to_string_lossy().into_owned()),
                created_at: 1,
                pinned_at: None,
            })
            .unwrap();
        let agent_service = AgentService::new(Arc::clone(&storage));
        let skill_installation_service =
            Arc::new(SkillInstallationService::new(temp.path().join("skills")).unwrap());
        let (outbound_tx, mut outbound_rx) = mpsc::unbounded_channel();
        let git_dispatcher = GitDispatcher::new(outbound_tx.clone());
        let skills_dispatcher = SkillsDispatcher::new(outbound_tx.clone());
        let dispatchers = RequestDispatchers {
            git: &git_dispatcher,
            skills: &skills_dispatcher,
            skill_acquisition: &skills_dispatcher,
        };
        let input = concat!(
            "{\"jsonrpc\":\"2.0\",\"id\":7,\"method\":\"skills.list\",",
            "\"params\":{\"projectId\":\"project-1\"}}\n"
        );

        let shutdown_id = run_request_loop(
            BufReader::new(input.as_bytes()),
            storage,
            &agent_service,
            SkillServices {
                catalog: Arc::new(SkillsService::new()),
                installations: skill_installation_service,
                workflow: Arc::new(SkillInstallationWorkflow::new(
                    SkillInstallationService::new(temp.path().join("skills")).unwrap(),
                )),
                source_resolution: Arc::new(SkillSourceResolutionService::new()),
            },
            Arc::new(GitReviewService::new()),
            &dispatchers,
            &outbound_tx,
        )
        .await
        .unwrap();
        let response = tokio::time::timeout(Duration::from_secs(2), outbound_rx.recv())
            .await
            .expect("skills.list must complete")
            .expect("skills.list must produce a response");

        assert!(shutdown_id.is_none());
        assert_eq!(response["id"], 7);
        assert_eq!(
            response["result"]["skills"][0]["id"],
            "workspace:project-1:auditor"
        );
        git_dispatcher.shutdown().await.unwrap();
        skills_dispatcher.shutdown().await.unwrap();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn changed_enablement_emits_one_invalidation_notification() {
        let temp = tempfile::tempdir().unwrap();
        let storage = Arc::new(StorageService::open(&temp.path().join("storage.sqlite")).unwrap());
        let skills_service = Arc::new(SkillsService::new().with_bundled_source().unwrap());
        let catalog = skills_service.list().unwrap();
        let skill_store = temp.path().join("skills");
        let installation_service = SkillInstallationService::new(&skill_store).unwrap();
        let installation_workflow =
            SkillInstallationWorkflow::new(SkillInstallationService::new(&skill_store).unwrap());
        let management = management_response(
            &storage,
            &catalog,
            &installation_service,
            Some(&installation_workflow),
        )
        .unwrap();
        let item = management.skills.first().unwrap();
        let skill_id = item.id.clone();
        let request_value = json!({
            "jsonrpc": "2.0",
            "id": 42,
            "method": SKILLS_SET_ENABLED_METHOD,
            "params": {
                "skillId": skill_id,
                "expectedStateRevision": item.state_revision,
                "enabled": false
            }
        });
        let input = format!("{request_value}\n");
        let agent_service = AgentService::new(Arc::clone(&storage));
        let installations =
            Arc::new(SkillInstallationService::new(temp.path().join("skills")).unwrap());
        let (outbound_tx, mut outbound_rx) = mpsc::unbounded_channel();
        let git_dispatcher = GitDispatcher::new(outbound_tx.clone());
        let skills_dispatcher = SkillsDispatcher::new(outbound_tx.clone());
        let dispatchers = RequestDispatchers {
            git: &git_dispatcher,
            skills: &skills_dispatcher,
            skill_acquisition: &skills_dispatcher,
        };

        run_request_loop(
            BufReader::new(input.as_bytes()),
            Arc::clone(&storage),
            &agent_service,
            SkillServices {
                catalog: Arc::clone(&skills_service),
                installations,
                workflow: Arc::new(SkillInstallationWorkflow::new(
                    SkillInstallationService::new(temp.path().join("skills")).unwrap(),
                )),
                source_resolution: Arc::new(SkillSourceResolutionService::new()),
            },
            Arc::new(GitReviewService::new()),
            &dispatchers,
            &outbound_tx,
        )
        .await
        .unwrap();

        let first = tokio::time::timeout(Duration::from_secs(2), outbound_rx.recv())
            .await
            .unwrap()
            .unwrap();
        let second = tokio::time::timeout(Duration::from_secs(2), outbound_rx.recv())
            .await
            .unwrap()
            .unwrap();
        let messages = [first, second];
        let notification = messages
            .iter()
            .find(|message| message["method"] == SKILLS_CHANGED_NOTIFICATION_METHOD)
            .expect("a changed Skill must emit an invalidation");
        assert_eq!(notification["params"]["reason"], "enablementChanged");
        assert_eq!(notification["params"]["skillId"], skill_id);
        assert!(notification["params"]["managementRevision"].is_string());
        let response = messages
            .iter()
            .find(|message| message["id"] == 42)
            .expect("the mutation response must also be delivered");
        assert_eq!(response["result"]["outcome"], "updated");

        git_dispatcher.shutdown().await.unwrap();
        skills_dispatcher.shutdown().await.unwrap();
    }

    #[test]
    fn indeterminate_direct_mutation_emits_catalog_invalidation_without_claiming_success() {
        const INSTALLATION_ID: &str = "0190b0f2-7c50-7cc0-8b25-3bb80f08b336";
        let temp = tempfile::tempdir().unwrap();
        let storage = StorageService::open(&temp.path().join("storage.sqlite")).unwrap();
        let catalog = SkillsService::new();
        let installations = SkillInstallationService::new(temp.path().join("skills")).unwrap();
        let workflow = SkillInstallationWorkflow::new(
            SkillInstallationService::new(temp.path().join("skills")).unwrap(),
        );
        let (notification_tx, mut notification_rx) = mpsc::unbounded_channel();
        let installation_id = SkillInstallationId::parse(INSTALLATION_ID).unwrap();
        let skill_id = SkillId::parse(format!("installed:user:{INSTALLATION_ID}")).unwrap();
        let intended_revision = SkillInstallationRevision::parse(format!(
            "skill-installation-sha256-v1:{}",
            "a".repeat(64)
        ))
        .unwrap();
        let error = SkillInstallationServiceError::Installer {
            operation: SkillInstallationOperation::Update,
            installation_id: installation_id.clone(),
            skill_id: skill_id.clone(),
            source: Box::new(
                mycopilot_core::skills::ManagedSkillInstallerError::CommitIndeterminate {
                    operation: mycopilot_core::skills::ManagedSkillMutation::Update,
                    installation_id,
                    intended_revision: Some(intended_revision),
                    reason: "receipt directory acknowledgement was lost".to_string(),
                },
            ),
        };

        let response = skill_mutation_response_with_invalidation(
            &storage,
            &catalog,
            &installations,
            Some(&workflow),
            Some(&notification_tx),
            JsonRpcId::Number(91),
            Err(error),
        );

        assert_eq!(response["error"]["data"]["code"], "commitIndeterminate");
        assert_eq!(response["error"]["data"]["commitMayHaveSucceeded"], true);
        assert!(response.get("result").is_none());
        let notification = notification_rx
            .try_recv()
            .expect("an uncertain direct mutation must invalidate management inventory");
        assert_eq!(notification["method"], SKILLS_CHANGED_NOTIFICATION_METHOD);
        assert_eq!(notification["params"]["reason"], "catalogChanged");
        assert_eq!(notification["params"]["skillId"], skill_id.as_str());
        assert!(notification["params"]["managementRevision"].is_string());
        assert!(notification_rx.try_recv().is_err());
    }

    #[test]
    fn indeterminate_workflow_commit_emits_catalog_invalidation_without_claiming_success() {
        const INSTALLATION_ID: &str = "11111111-1111-4111-8111-111111111111";
        let temp = tempfile::tempdir().unwrap();
        let storage = StorageService::open(&temp.path().join("storage.sqlite")).unwrap();
        let catalog = SkillsService::new();
        let installations = SkillInstallationService::new(temp.path().join("skills")).unwrap();
        let workflow = SkillInstallationWorkflow::new(
            SkillInstallationService::new(temp.path().join("skills")).unwrap(),
        );
        let (notification_tx, mut notification_rx) = mpsc::unbounded_channel();
        let preparation_id =
            mycopilot_core::skills::SkillPreparationId::parse(INSTALLATION_ID).unwrap();
        let installation_id = SkillInstallationId::parse(INSTALLATION_ID).unwrap();
        let skill_id = SkillId::parse(format!("installed:user:{INSTALLATION_ID}")).unwrap();
        let intended_revision = SkillInstallationRevision::parse(format!(
            "skill-installation-sha256-v1:{}",
            "b".repeat(64)
        ))
        .unwrap();
        let workflow_error = mycopilot_core::skills::SkillInstallationWorkflowError::Installation {
            preparation_id,
            source: Box::new(SkillInstallationServiceError::Installer {
                operation: SkillInstallationOperation::Install,
                installation_id: installation_id.clone(),
                skill_id: skill_id.clone(),
                source: Box::new(
                    mycopilot_core::skills::ManagedSkillInstallerError::CommitIndeterminate {
                        operation: mycopilot_core::skills::ManagedSkillMutation::Install,
                        installation_id,
                        intended_revision: Some(intended_revision),
                        reason: "receipt directory acknowledgement was lost".to_string(),
                    },
                ),
            }),
        };
        let failure = workflow_failure(
            mycopilot_protocol_rs::SkillInspectionPhaseDto::Commit,
            &workflow_error,
        );

        let response = skill_workflow_commit_error_response_with_invalidation(
            &storage,
            &catalog,
            &installations,
            Some(&workflow),
            Some(&notification_tx),
            JsonRpcId::Number(92),
            failure,
        );

        assert_eq!(response["error"]["data"]["code"], "commitIndeterminate");
        assert_eq!(response["error"]["data"]["commitMayHaveSucceeded"], true);
        assert!(response.get("result").is_none());
        let notification = notification_rx
            .try_recv()
            .expect("an uncertain workflow commit must invalidate management inventory");
        assert_eq!(notification["method"], SKILLS_CHANGED_NOTIFICATION_METHOD);
        assert_eq!(notification["params"]["reason"], "catalogChanged");
        assert_eq!(notification["params"]["skillId"], skill_id.as_str());
        assert!(notification_rx.try_recv().is_err());
    }

    #[test]
    fn ordinary_direct_mutation_failure_does_not_emit_an_invalidation() {
        const INSTALLATION_ID: &str = "0190b0f2-7c50-7cc0-8b25-3bb80f08b337";
        let temp = tempfile::tempdir().unwrap();
        let storage = StorageService::open(&temp.path().join("storage.sqlite")).unwrap();
        let catalog = SkillsService::new();
        let installations = SkillInstallationService::new(temp.path().join("skills")).unwrap();
        let workflow = SkillInstallationWorkflow::new(
            SkillInstallationService::new(temp.path().join("skills")).unwrap(),
        );
        let (notification_tx, mut notification_rx) = mpsc::unbounded_channel();
        let installation_id = SkillInstallationId::parse(INSTALLATION_ID).unwrap();
        let skill_id = SkillId::parse(format!("installed:user:{INSTALLATION_ID}")).unwrap();
        let error = SkillInstallationServiceError::Installer {
            operation: SkillInstallationOperation::Update,
            installation_id,
            skill_id,
            source: Box::new(mycopilot_core::skills::ManagedSkillInstallerError::Io {
                operation: "publish managed receipt".to_string(),
                reason: "injected pre-publication failure".to_string(),
            }),
        };

        let response = skill_mutation_response_with_invalidation(
            &storage,
            &catalog,
            &installations,
            Some(&workflow),
            Some(&notification_tx),
            JsonRpcId::Number(93),
            Err(error),
        );

        assert_eq!(response["error"]["data"]["code"], "io");
        assert_eq!(response["error"]["data"]["commitMayHaveSucceeded"], false);
        assert!(notification_rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn outbound_writer_finishes_after_draining_with_a_lingering_sender() {
        let (writer_stream, mut reader_stream) = io::duplex(1024);
        let (outbound_tx, outbound_rx) = mpsc::unbounded_channel();
        let lingering_sender = outbound_tx.clone();
        let (finish_tx, finish_rx) = oneshot::channel();
        let writer = tokio::spawn(run_outbound_writer(writer_stream, outbound_rx, finish_rx));
        let first = json!({ "id": 1, "result": "before shutdown" });
        let final_response = json!({ "id": 2, "result": "shutdown" });

        enqueue_outbound(&outbound_tx, first.clone()).unwrap();
        enqueue_outbound(&outbound_tx, final_response.clone()).unwrap();
        finish_tx.send(()).unwrap();
        drop(outbound_tx);

        tokio::time::timeout(Duration::from_secs(1), writer)
            .await
            .expect("writer must not wait for a lingering producer")
            .expect("writer task must join")
            .expect("writer must drain successfully");
        assert!(lingering_sender.send(json!({ "late": true })).is_err());

        let mut output = String::new();
        reader_stream.read_to_string(&mut output).await.unwrap();
        let messages = output
            .lines()
            .map(|line| serde_json::from_str::<Value>(line).unwrap())
            .collect::<Vec<_>>();
        assert_eq!(messages, vec![first, final_response]);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn request_loop_remains_responsive_while_filesystem_workers_are_blocked() {
        let temp = tempfile::tempdir().unwrap();
        let storage = Arc::new(StorageService::open(&temp.path().join("storage.sqlite")).unwrap());
        let agent_service = AgentService::new(Arc::clone(&storage));
        let skills_service = Arc::new(SkillsService::new());
        let skill_installation_service =
            Arc::new(SkillInstallationService::new(temp.path().join("skills")).unwrap());
        let git_review_service = Arc::new(GitReviewService::new());
        let (outbound_tx, mut outbound_rx) = mpsc::unbounded_channel();
        let dispatcher = GitDispatcher::new(outbound_tx.clone());
        let skill_dispatcher = SkillsDispatcher::new(outbound_tx.clone());
        let request_dispatchers = RequestDispatchers {
            git: &dispatcher,
            skills: &skill_dispatcher,
            skill_acquisition: &skill_dispatcher,
        };
        let (started_tx, started_rx) = std_mpsc::channel();
        let mut release_senders = Vec::new();

        for request_id in 1..=3 {
            let (release_tx, release_rx) = std_mpsc::channel();
            release_senders.push(release_tx);
            let started_tx = started_tx.clone();
            dispatcher
                .try_submit(
                    GitJobPriority::Low,
                    JsonRpcId::Number(request_id),
                    move || {
                        started_tx.send(()).unwrap();
                        release_rx.recv().unwrap();
                        serde_json::json!({ "id": request_id })
                    },
                )
                .unwrap();
        }
        for _ in 0..3 {
            started_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        }
        let (skill_started_tx, skill_started_rx) = std_mpsc::channel();
        let (skill_release_tx, skill_release_rx) = std_mpsc::channel();
        skill_dispatcher
            .try_submit(JsonRpcId::Number(4), move || {
                skill_started_tx.send(()).unwrap();
                skill_release_rx.recv().unwrap();
                serde_json::json!({ "id": 4 })
            })
            .unwrap();
        skill_started_rx
            .recv_timeout(Duration::from_secs(2))
            .unwrap();

        let input = concat!(
            "{\"jsonrpc\":\"2.0\",\"id\":99,\"method\":\"core.ping\",",
            "\"params\":{\"message\":\"responsive\"}}\n",
            "{\"jsonrpc\":\"2.0\",\"id\":100,\"method\":\"core.shutdown\"}\n"
        );
        let shutdown_id = run_request_loop(
            BufReader::new(input.as_bytes()),
            storage,
            &agent_service,
            SkillServices {
                catalog: skills_service,
                installations: skill_installation_service,
                workflow: Arc::new(SkillInstallationWorkflow::new(
                    SkillInstallationService::new(temp.path().join("skills")).unwrap(),
                )),
                source_resolution: Arc::new(SkillSourceResolutionService::new()),
            },
            git_review_service,
            &request_dispatchers,
            &outbound_tx,
        )
        .await
        .unwrap();

        assert!(matches!(shutdown_id, Some(JsonRpcId::Number(100))));
        let ping = outbound_rx.try_recv().unwrap();
        assert_eq!(ping["id"], 99);
        assert_eq!(ping["result"]["echo"], "responsive");

        for release in release_senders {
            release.send(()).unwrap();
        }
        skill_release_tx.send(()).unwrap();
        dispatcher.shutdown().await.unwrap();
        skill_dispatcher.shutdown().await.unwrap();
    }
}
