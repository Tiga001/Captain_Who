// Rust core server.
mod agent;
mod agent_support;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use agent::{AgentConversationTurnInput, AgentService};
use mycopilot_core::storage::models::{
    AgentPromptPreferencesRecord, ChatConversationMetaRecord, ChatConversationRecord,
    ChatMessageRecord, ChatMessageStateRecord, ComposerDraftRecord, ModelSettingsRecord,
    ProjectRecord, UiPreferencesRecord,
};
use mycopilot_core::storage::service::StorageService;
use mycopilot_core::{AgentUsageClearInput, AgentUsageSummaryInput};
use mycopilot_protocol_rs::{
    error, success, AgentActionIdRequest, AgentCancelRunRequest, AgentCancelRunResponse,
    AgentRejectActionRequest, AgentStartRunRequest, AgentStartRunResponse, AppVersionResponse,
    CorePingRequest, CorePingResponse, JsonRpcId, JsonRpcRequest, AGENT_APPROVE_ACTION_METHOD,
    AGENT_CANCEL_ACTION_METHOD, AGENT_CANCEL_RUN_METHOD, AGENT_CLEAR_USAGE_RECORDS_METHOD,
    AGENT_GET_USAGE_SUMMARY_METHOD, AGENT_LIST_PENDING_ACTIONS_METHOD, AGENT_REJECT_ACTION_METHOD,
    AGENT_START_CONVERSATION_TURN_METHOD, AGENT_START_RUN_METHOD, APP_GET_VERSION_METHOD,
    CORE_PING_METHOD, STORAGE_DELETE_CHAT_MESSAGES_METHOD, STORAGE_DELETE_COMPOSER_DRAFT_METHOD,
    STORAGE_DELETE_CONVERSATION_METHOD, STORAGE_DELETE_PROJECT_METHOD,
    STORAGE_LOAD_AGENT_PROMPT_PREFERENCES_METHOD, STORAGE_LOAD_APP_DATA_METHOD,
    STORAGE_LOAD_ATTACHMENT_IMAGE_METHOD, STORAGE_LOAD_COMPOSER_DRAFTS_METHOD,
    STORAGE_LOAD_CONVERSATIONS_METHOD, STORAGE_LOAD_INPUT_ATTACHMENTS_METHOD,
    STORAGE_LOAD_MODEL_SETTINGS_METHOD, STORAGE_LOAD_PROJECTS_METHOD,
    STORAGE_LOAD_UI_PREFERENCES_METHOD, STORAGE_REVEAL_PROJECT_FILE_METHOD,
    STORAGE_SAVE_AGENT_PROMPT_PREFERENCES_METHOD, STORAGE_SAVE_CHAT_MESSAGE_STATE_METHOD,
    STORAGE_SAVE_COMPOSER_DRAFT_METHOD, STORAGE_SAVE_CONVERSATION_META_METHOD,
    STORAGE_SAVE_CONVERSATION_METHOD, STORAGE_SAVE_MODEL_SETTINGS_METHOD,
    STORAGE_SAVE_PROJECT_METHOD, STORAGE_SAVE_UI_PREFERENCES_METHOD,
    STORAGE_SELECT_PROFILE_AVATAR_METHOD, STORAGE_SELECT_PROJECT_DIRECTORY_METHOD,
    STORAGE_SHOW_PROJECT_IN_FOLDER_METHOD, STORAGE_UPSERT_CHAT_MESSAGES_METHOD,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::io::{self, AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::mpsc;
use uuid::Uuid;

#[tokio::main]
async fn main() -> io::Result<()> {
    let storage = Arc::new(StorageService::open(&database_path()).map_err(|error| {
        io::Error::new(
            io::ErrorKind::Other,
            format!("failed to initialize storage: {error}"),
        )
    })?);
    let agent_service = AgentService::new(storage.clone());
    let stdin = BufReader::new(io::stdin());
    let mut lines = stdin.lines();
    let mut stdout = io::stdout();
    let (notification_tx, mut notification_rx) = mpsc::unbounded_channel::<Value>();

    loop {
        tokio::select! {
            line = lines.next_line() => {
                let Some(line) = line? else {
                    break;
                };
                if line.trim().is_empty() {
                    continue;
                }

                let response = match serde_json::from_str::<JsonRpcRequest>(&line) {
                    Ok(request) => handle_request(
                        &storage,
                        &agent_service,
                        notification_tx.clone(),
                        request,
                    ),
                    Err(err) => serde_json::to_value(error(None, -32700, format!("Parse error: {err}")))
                        .expect("JSON-RPC parse error response must serialize"),
                };

                write_json_line(&mut stdout, response).await?;
            }
            notification = notification_rx.recv() => {
                let Some(notification) = notification else {
                    break;
                };
                write_json_line(&mut stdout, notification).await?;
            }
        }
    }

    Ok(())
}

async fn write_json_line(stdout: &mut io::Stdout, message: Value) -> io::Result<()> {
    stdout.write_all(message.to_string().as_bytes()).await?;
    stdout.write_all(b"\n").await?;
    stdout.flush().await
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
        APP_GET_VERSION_METHOD => response_success(
            request.id,
            AppVersionResponse {
                name: env!("CARGO_PKG_NAME"),
                version: env!("CARGO_PKG_VERSION"),
            },
        ),
        AGENT_START_RUN_METHOD => handle_agent_start_run(request.id, request.params),
        AGENT_START_CONVERSATION_TURN_METHOD => handle_agent_start_conversation_turn(
            agent_service,
            notification_tx,
            request.id,
            request.params,
        ),
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
        STORAGE_LOAD_APP_DATA_METHOD => storage_response(request.id, storage.load_app_data()),
        STORAGE_LOAD_MODEL_SETTINGS_METHOD => {
            storage_response(request.id, storage.load_model_settings())
        }
        STORAGE_SAVE_MODEL_SETTINGS_METHOD => {
            let settings = match parse_params::<ModelSettingsRecord>(request.params) {
                Ok(settings) => settings,
                Err(message) => return response_error(Some(request.id), -32602, message),
            };
            storage_response(
                request.id,
                storage.save_model_settings(settings).map(|_| json!(null)),
            )
        }
        STORAGE_LOAD_AGENT_PROMPT_PREFERENCES_METHOD => {
            storage_response(request.id, storage.load_agent_prompt_preferences())
        }
        STORAGE_SAVE_AGENT_PROMPT_PREFERENCES_METHOD => {
            let preferences = match parse_params::<AgentPromptPreferencesRecord>(request.params) {
                Ok(preferences) => preferences,
                Err(message) => return response_error(Some(request.id), -32602, message),
            };
            storage_response(
                request.id,
                storage.save_agent_prompt_preferences(preferences),
            )
        }
        STORAGE_LOAD_PROJECTS_METHOD => storage_response(request.id, storage.load_projects()),
        STORAGE_SELECT_PROJECT_DIRECTORY_METHOD => response_success(request.id, Value::Null),
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
                storage
                    .delete_project(&input.project_id)
                    .map(|_| json!(null)),
            )
        }
        STORAGE_SHOW_PROJECT_IN_FOLDER_METHOD => response_success(request.id, Value::Null),
        STORAGE_REVEAL_PROJECT_FILE_METHOD => response_success(request.id, Value::Null),
        STORAGE_LOAD_CONVERSATIONS_METHOD => {
            storage_response(request.id, storage.load_conversations())
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
        STORAGE_SAVE_CONVERSATION_METHOD => {
            let conversation = match parse_params::<ChatConversationRecord>(request.params) {
                Ok(conversation) => conversation,
                Err(message) => return response_error(Some(request.id), -32602, message),
            };
            storage_response(request.id, storage.save_conversation(conversation))
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
                storage
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
                storage
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
        STORAGE_DELETE_COMPOSER_DRAFT_METHOD => {
            let input = match parse_params::<ScopeIdRequest>(request.params) {
                Ok(input) => input,
                Err(message) => return response_error(Some(request.id), -32602, message),
            };
            storage_response(
                request.id,
                storage
                    .delete_composer_draft(&input.scope_id)
                    .map(|_| json!(null)),
            )
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
        STORAGE_SELECT_PROFILE_AVATAR_METHOD => response_success(request.id, Value::Null),
        _ => response_error(Some(request.id), -32601, "Method not found"),
    }
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

fn handle_agent_start_run(id: JsonRpcId, params: Option<Value>) -> Value {
    if let Err(message) = parse_params::<AgentStartRunRequest>(params) {
        return response_error(Some(id), -32602, message);
    }

    response_success(
        id,
        AgentStartRunResponse {
            run_id: Uuid::new_v4().to_string(),
            status: "not_implemented",
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
        Err(message) => response_error(Some(id), -32000, message),
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
struct ScopeIdRequest {
    scope_id: String,
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

fn home_dir() -> PathBuf {
    std::env::var("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("."))
}
