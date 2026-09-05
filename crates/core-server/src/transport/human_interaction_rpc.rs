use super::*;
use crate::application::human_interaction::HumanInteractionService;
use mycopilot_core::human_interaction::{HumanInteractionError, HUMAN_INTERACTION_MAX_INPUT_BYTES};
use mycopilot_protocol_rs::*;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct EmptyInput {}

pub(crate) fn is_human_interaction_method(method: &str) -> bool {
    matches!(
        method,
        HUMAN_INTERACTION_GET_SETTINGS_METHOD
            | HUMAN_INTERACTION_UPDATE_SETTINGS_METHOD
            | HUMAN_INTERACTION_LIST_REQUESTS_METHOD
            | HUMAN_INTERACTION_SUBMIT_METHOD
            | HUMAN_INTERACTION_IGNORE_METHOD
    )
}

pub(crate) fn handle_human_interaction_request(
    storage: &StorageService,
    agent: &AgentService,
    notifications: agent::CoreServerNotificationSender,
    request: JsonRpcRequest,
) -> Value {
    let service = HumanInteractionService::new(storage, agent);
    let id = request.id;
    match request.method.as_str() {
        HUMAN_INTERACTION_GET_SETTINGS_METHOD => {
            parse_and_run(id, request.params, |_: EmptyInput| service.get_settings())
        }
        HUMAN_INTERACTION_UPDATE_SETTINGS_METHOD => parse_and_run(id, request.params, |input| {
            service.update_settings(input, &notifications)
        }),
        HUMAN_INTERACTION_LIST_REQUESTS_METHOD => {
            parse_and_run(id, request.params, |input| service.list(input))
        }
        HUMAN_INTERACTION_SUBMIT_METHOD => parse_and_run(id, request.params, |input| {
            service.submit(input, &notifications)
        }),
        HUMAN_INTERACTION_IGNORE_METHOD => parse_and_run(id, request.params, |input| {
            service.ignore(input, &notifications)
        }),
        _ => response_error(Some(id), -32601, "Method not found"),
    }
}

fn parse_and_run<I, O>(
    id: JsonRpcId,
    params: Option<Value>,
    operation: impl FnOnce(I) -> Result<O, HumanInteractionError>,
) -> Value
where
    I: for<'de> Deserialize<'de>,
    O: Serialize,
{
    if params.as_ref().is_some_and(|params| {
        serde_json::to_vec(params).map_or(true, |bytes| {
            bytes.len() > HUMAN_INTERACTION_MAX_INPUT_BYTES + 4096
        })
    }) {
        return response_error(Some(id), -32602, "Human interaction request is too large");
    }
    let input = match parse_params::<I>(params) {
        Ok(input) => input,
        Err(_) => return response_error(Some(id), -32602, "Invalid human interaction parameters"),
    };
    match operation(input) {
        Ok(output) => response_success(id, output),
        Err(error) => serde_json::to_value(error_with_data(
            Some(id),
            -32047,
            error.message,
            serde_json::json!({"type":"human_interaction_error", "code":error.code}),
        ))
        .expect("human interaction errors serialize"),
    }
}
