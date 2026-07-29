use super::*;

pub(crate) fn parse_params<T>(params: Option<Value>) -> Result<T, String>
where
    T: for<'de> serde::Deserialize<'de>,
{
    serde_json::from_value(params.unwrap_or(Value::Null))
        .map_err(|err| format!("Invalid params: {err}"))
}

pub(crate) fn response_success<T>(id: JsonRpcId, result: T) -> Value
where
    T: Serialize,
{
    serde_json::to_value(success(id, result)).expect("JSON-RPC success response must serialize")
}

pub(crate) fn response_error(
    id: Option<JsonRpcId>,
    code: i64,
    message: impl Into<String>,
) -> Value {
    serde_json::to_value(error(id, code, message)).expect("JSON-RPC error response must serialize")
}

pub(crate) fn agent_service_error_response(id: JsonRpcId, error: AgentServiceError) -> Value {
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

pub(crate) fn storage_response<T>(id: JsonRpcId, result: Result<T, String>) -> Value
where
    T: Serialize,
{
    match result {
        Ok(value) => response_success(id, value),
        Err(message) => response_error(Some(id), -32000, message),
    }
}

pub(crate) fn now_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}
