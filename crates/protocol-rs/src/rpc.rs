use serde::{Deserialize, Serialize};
use serde_json::Value;

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
