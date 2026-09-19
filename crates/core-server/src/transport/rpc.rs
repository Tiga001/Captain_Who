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
        Err(ModelSettingsSaveError::DuplicateDisplayName { display_name }) => {
            let data =
                StorageModelSettingsValidationErrorData::duplicate_display_name(display_name);
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
        Err(ModelSettingsSaveError::InvalidContextCapacity {
            model_id,
            display_name,
            context_window_tokens,
            reserved_output_tokens,
            safety_margin_tokens,
            minimum_context_window_tokens,
        }) => {
            let data = StorageModelSettingsValidationErrorData::invalid_context_capacity(
                model_id,
                display_name,
                context_window_tokens,
                reserved_output_tokens,
                safety_margin_tokens,
                minimum_context_window_tokens,
            );
            serde_json::to_value(error_with_data(
                Some(id),
                -32000,
                "Model settings validation failed.",
                serde_json::to_value(data)
                    .expect("model settings validation error data must serialize"),
            ))
            .expect("model settings validation JSON-RPC error response must serialize")
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_context_capacity_save_failure_has_safe_structured_error_data() {
        let response = model_settings_save_response::<Value>(
            JsonRpcId::Number(7),
            Err(
                mycopilot_core::storage::models::ModelSettingsSaveError::InvalidContextCapacity {
                    model_id: "model-a".into(),
                    display_name: "DeepSeek Max".into(),
                    context_window_tokens: 128_000,
                    reserved_output_tokens: 131_072,
                    safety_margin_tokens: 6_400,
                    minimum_context_window_tokens: 137_972,
                },
            ),
        );
        assert_eq!(
            response["error"]["data"],
            json!({
                "kind": "model_settings_validation", "code": "invalid_context_capacity_configuration",
                "modelId": "model-a", "displayName": "DeepSeek Max", "contextWindowTokens": 128_000,
                "reservedOutputTokens": 131_072, "safetyMarginTokens": 6_400, "minimumContextWindowTokens": 137_972
            })
        );
        assert_eq!(
            response["error"]["message"],
            "Model settings validation failed."
        );
        assert!(response.get("result").is_none());
    }
}
