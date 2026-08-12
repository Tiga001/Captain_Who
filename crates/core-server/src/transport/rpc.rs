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
        None => match error.data() {
            Some(data) => serde_json::to_value(error_with_data(
                Some(id),
                -32000,
                error.message(),
                data.clone(),
            ))
            .expect("Agent service error data must serialize"),
            None => response_error(Some(id), -32000, error.message()),
        },
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

pub(crate) fn model_settings_save_response<T>(
    id: JsonRpcId,
    result: Result<T, mycopilot_core::storage::models::ModelSettingsSaveError>,
) -> Value
where
    T: Serialize,
{
    use mycopilot_core::storage::models::ModelSettingsSaveError;
    use mycopilot_protocol_rs::StorageModelSettingsValidationErrorData;

    match result {
        Ok(value) => response_success(id, value),
        Err(ModelSettingsSaveError::DuplicateModelId { model_id }) => {
            let data = StorageModelSettingsValidationErrorData::duplicate_model_id(model_id);
            serde_json::to_value(error_with_data(
                Some(id),
                -32000,
                "Model settings validation failed.",
                serde_json::to_value(data)
                    .expect("model settings validation error data must serialize"),
            ))
            .expect("model settings validation JSON-RPC error response must serialize")
        }
        Err(ModelSettingsSaveError::Other(_)) => {
            response_error(Some(id), -32000, "Model settings could not be saved.")
        }
    }
}

pub(crate) fn conversation_fork_response<T>(
    id: JsonRpcId,
    result: Result<T, mycopilot_core::storage::conversation_fork_repository::ConversationForkError>,
) -> Value
where
    T: Serialize,
{
    use mycopilot_core::storage::conversation_fork_repository::{
        ConversationForkError, CONVERSATION_FORK_ACTIVE_COMMAND_ERROR_CODE,
        CONVERSATION_FORK_ACTIVE_COMMAND_MESSAGE, CONVERSATION_FORK_ERROR_TYPE,
    };

    match result {
        Ok(value) => response_success(id, value),
        Err(ConversationForkError::ActiveCommandSession {
            conversation_id,
            active_session_count,
        }) => serde_json::to_value(error_with_data(
            Some(id),
            -32000,
            CONVERSATION_FORK_ACTIVE_COMMAND_MESSAGE,
            json!({
                "type": CONVERSATION_FORK_ERROR_TYPE,
                "code": CONVERSATION_FORK_ACTIVE_COMMAND_ERROR_CODE,
                "conversationId": conversation_id,
                "activeSessionCount": active_session_count,
            }),
        ))
        .expect("conversation fork JSON-RPC error response must serialize"),
        Err(error) => response_error(Some(id), -32000, error.message()),
    }
}

pub(crate) fn now_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}
