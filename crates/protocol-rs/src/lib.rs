use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const CORE_PING_METHOD: &str = "core.ping";
pub const CORE_SHUTDOWN_METHOD: &str = "core.shutdown";
pub const AGENT_CANCEL_RUN_METHOD: &str = "agent.cancelRun";
pub const AGENT_START_CONVERSATION_TURN_METHOD: &str = "agent.startConversationTurn";
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
