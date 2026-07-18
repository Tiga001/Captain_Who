mod agent;
mod agent_support;
mod git_dispatcher;
mod skills_adapter;
mod skills_dispatcher;
#[cfg(test)]
mod skills_test_support;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use agent::{AgentConversationTurnInput, AgentService, AgentServiceError};
use git_dispatcher::{GitDispatcher, GitJobPriority};
use mycopilot_core::git_review::{GitReviewFileMutationAction, GitReviewScope, GitReviewService};
use mycopilot_core::skills::SkillsService;
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
    GitReviewSummaryRequest, JsonRpcId, JsonRpcRequest, SkillsListRequest,
    AGENT_APPROVE_ACTION_METHOD, AGENT_CANCEL_ACTION_METHOD, AGENT_CANCEL_RUN_METHOD,
    AGENT_CLEAR_USAGE_RECORDS_METHOD, AGENT_GET_CONTEXT_COMPACTION_AUDIT_METHOD,
    AGENT_GET_CONTEXT_WINDOW_SNAPSHOT_METHOD, AGENT_GET_FILE_WRITE_DIFF_METHOD,
    AGENT_GET_USAGE_SUMMARY_METHOD, AGENT_LIST_PENDING_ACTIONS_METHOD,
    AGENT_READ_FILE_DRAFT_METHOD, AGENT_REJECT_ACTION_METHOD, AGENT_START_CONVERSATION_TURN_METHOD,
    CORE_PING_METHOD, CORE_SHUTDOWN_METHOD, GIT_GET_REVIEW_FILE_CONTENT_METHOD,
    GIT_GET_REVIEW_FILE_DIFF_METHOD, GIT_GET_REVIEW_SUMMARY_METHOD, GIT_INSPECT_REPOSITORY_METHOD,
    GIT_MUTATE_REVIEW_FILE_METHOD, SEARCH_SEARCH_CHATS_METHOD, SKILLS_LIST_METHOD,
    STORAGE_DELETE_CHAT_MESSAGES_METHOD, STORAGE_DELETE_CONVERSATION_METHOD,
    STORAGE_DELETE_PROJECT_METHOD, STORAGE_FORK_CONVERSATION_METHOD,
    STORAGE_LOAD_AGENT_PROMPT_PREFERENCES_METHOD, STORAGE_LOAD_ATTACHMENT_IMAGE_METHOD,
    STORAGE_LOAD_COMPOSER_DRAFTS_METHOD, STORAGE_LOAD_CONVERSATIONS_METHOD,
    STORAGE_LOAD_INPUT_ATTACHMENTS_METHOD, STORAGE_LOAD_MODEL_SETTINGS_METHOD,
    STORAGE_LOAD_PROJECTS_METHOD, STORAGE_LOAD_UI_PREFERENCES_METHOD,
    STORAGE_SAVE_AGENT_PROMPT_PREFERENCES_METHOD, STORAGE_SAVE_CHAT_MESSAGE_STATE_METHOD,
    STORAGE_SAVE_COMPOSER_DRAFT_METHOD, STORAGE_SAVE_CONVERSATION_META_METHOD,
    STORAGE_SAVE_MODEL_SETTINGS_METHOD, STORAGE_SAVE_PROJECT_METHOD,
    STORAGE_SAVE_UI_PREFERENCES_METHOD, STORAGE_UPSERT_CHAT_MESSAGES_METHOD,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use skills_adapter::catalog_response;
use skills_dispatcher::SkillsDispatcher;
use tokio::io::{self, AsyncBufRead, AsyncBufReadExt, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::sync::{mpsc, oneshot};

#[tokio::main]
async fn main() -> io::Result<()> {
    let database_path = absolute_path(database_path())?;
    let skill_store_root = skill_store_root(&database_path);
    let storage = Arc::new(
        StorageService::open(&database_path)
            .map_err(|error| io::Error::other(format!("failed to initialize storage: {error}")))?,
    );
    let git_review_service = Arc::new(GitReviewService::new());
    let skills_service = Arc::new(
        SkillsService::new()
            .with_bundled_source()
            .and_then(|service| service.with_installed_source(skill_store_root))
            .map_err(|error| io::Error::other(format!("failed to initialize Skills: {error}")))?,
    );
    let agent_service =
        AgentService::new(storage.clone()).with_skills_service(Arc::clone(&skills_service));
    let (outbound_tx, outbound_rx) = mpsc::unbounded_channel::<Value>();
    let (finish_outbound_tx, finish_outbound_rx) = oneshot::channel();
    let writer = tokio::spawn(run_outbound_writer(
        io::stdout(),
        outbound_rx,
        finish_outbound_rx,
    ));
    let git_dispatcher = GitDispatcher::new(outbound_tx.clone());
    let skill_dispatcher = SkillsDispatcher::new(outbound_tx.clone());
    let request_dispatchers = RequestDispatchers {
        git: &git_dispatcher,
        skills: &skill_dispatcher,
    };

    let input_result = run_request_loop(
        BufReader::new(io::stdin()),
        storage,
        &agent_service,
        skills_service,
        git_review_service,
        &request_dispatchers,
        &outbound_tx,
    )
    .await;

    // Admission has stopped. Settle accepted filesystem jobs while active agents are cancelled in
    // parallel; queued jobs receive cancellation errors and running jobs get a bounded grace
    // period. The outbound writer remains live for every final response and notification.
    let (git_dispatcher_result, skill_dispatcher_result, (cancelled_runs, timed_out)) = tokio::join!(
        git_dispatcher.shutdown(),
        skill_dispatcher.shutdown(),
        agent_service.shutdown_active_runs(Duration::from_secs(2))
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
    if let Some(error) = outbound_error {
        return Err(error);
    }
    writer_result
}

struct RequestDispatchers<'a> {
    git: &'a GitDispatcher,
    skills: &'a SkillsDispatcher,
}

async fn run_request_loop<R>(
    input: R,
    storage: Arc<StorageService>,
    agent_service: &AgentService,
    skills_service: Arc<SkillsService>,
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
            if request.method == SKILLS_LIST_METHOD {
                let request_id = request.id.clone();
                let request_storage = Arc::clone(&storage);
                let request_service = Arc::clone(&skills_service);
                if let Err(error) = dispatchers.skills.try_submit(request_id.clone(), move || {
                    handle_skills_request(&request_storage, &request_service, request)
                }) {
                    enqueue_outbound(
                        outbound,
                        response_error(Some(request_id), error.code(), error.message()),
                    )?;
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

fn handle_skills_request(
    storage: &StorageService,
    skills_service: &SkillsService,
    request: JsonRpcRequest,
) -> Value {
    if request.jsonrpc != "2.0" {
        return response_error(Some(request.id), -32600, "Invalid JSON-RPC version");
    }
    if request.method != SKILLS_LIST_METHOD {
        return response_error(Some(request.id), -32601, "Method not found");
    }

    let input = match parse_params::<SkillsListRequest>(request.params) {
        Ok(input) => input,
        Err(message) => return response_error(Some(request.id), -32602, message),
    };
    let result = resolve_project_path(storage, &input.project_id).and_then(|workspace| {
        skills_service
            .list_with_workspace(&input.project_id, &workspace)
            .map_err(|error| error.to_string())
    });
    match result.and_then(|catalog| catalog_response(&catalog)) {
        Ok(catalog) => response_success(request.id, catalog),
        Err(message) => response_error(Some(request.id), -32000, message),
    }
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
    use std::fs;
    use std::sync::mpsc as std_mpsc;
    use tokio::io::AsyncReadExt;

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

        let response = handle_skills_request(&storage, &skills_service, request);

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
    fn skills_list_rejects_missing_project_id_as_invalid_params() {
        let temp = tempfile::tempdir().unwrap();
        let storage = Arc::new(StorageService::open(&temp.path().join("storage.sqlite")).unwrap());
        let skills_service = SkillsService::new();
        let request = serde_json::from_value::<JsonRpcRequest>(json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": SKILLS_LIST_METHOD,
            "params": {}
        }))
        .unwrap();

        let response = handle_skills_request(&storage, &skills_service, request);

        assert_eq!(response["error"]["code"], -32602);
    }

    #[test]
    fn skills_list_rejects_an_unknown_project_with_a_server_error() {
        let temp = tempfile::tempdir().unwrap();
        let storage = StorageService::open(&temp.path().join("storage.sqlite")).unwrap();
        let request = serde_json::from_value::<JsonRpcRequest>(json!({
            "jsonrpc": "2.0",
            "id": "skills-request",
            "method": SKILLS_LIST_METHOD,
            "params": { "projectId": "missing" }
        }))
        .unwrap();

        let response = handle_skills_request(&storage, &SkillsService::new(), request);

        assert_eq!(response["id"], "skills-request");
        assert_eq!(response["error"]["code"], -32000);
        assert_eq!(
            response["error"]["message"],
            "The selected project no longer exists."
        );
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
        let (outbound_tx, mut outbound_rx) = mpsc::unbounded_channel();
        let git_dispatcher = GitDispatcher::new(outbound_tx.clone());
        let skills_dispatcher = SkillsDispatcher::new(outbound_tx.clone());
        let dispatchers = RequestDispatchers {
            git: &git_dispatcher,
            skills: &skills_dispatcher,
        };
        let input = concat!(
            "{\"jsonrpc\":\"2.0\",\"id\":7,\"method\":\"skills.list\",",
            "\"params\":{\"projectId\":\"project-1\"}}\n"
        );

        let shutdown_id = run_request_loop(
            BufReader::new(input.as_bytes()),
            storage,
            &agent_service,
            Arc::new(SkillsService::new()),
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
        let git_review_service = Arc::new(GitReviewService::new());
        let (outbound_tx, mut outbound_rx) = mpsc::unbounded_channel();
        let dispatcher = GitDispatcher::new(outbound_tx.clone());
        let skill_dispatcher = SkillsDispatcher::new(outbound_tx.clone());
        let request_dispatchers = RequestDispatchers {
            git: &dispatcher,
            skills: &skill_dispatcher,
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
            skills_service,
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
