use super::*;
use crate::application::notification::{NotificationService, NotificationServiceError};
use mycopilot_protocol_rs::*;

pub(crate) fn is_notification_request_method(method: &str) -> bool {
    matches!(
        method,
        NOTIFICATION_BATCHES_CLAIM_METHOD
            | NOTIFICATION_BATCH_VALIDATE_METHOD
            | NOTIFICATION_BATCH_ACKNOWLEDGE_METHOD
            | NOTIFICATION_BATCH_RELEASE_METHOD
            | NOTIFICATION_BATCH_SUPPRESS_METHOD
            | NOTIFICATION_LIST_METHOD
            | NOTIFICATION_SUMMARY_METHOD
            | NOTIFICATION_MARK_SEEN_METHOD
            | NOTIFICATION_SETTINGS_GET_METHOD
            | NOTIFICATION_SETTINGS_UPDATE_METHOD
    )
}

pub(crate) fn handle_notification_request(
    storage: &StorageService,
    request: JsonRpcRequest,
) -> Value {
    let id = request.id;
    let service = NotificationService::new(storage);
    match request.method.as_str() {
        NOTIFICATION_BATCHES_CLAIM_METHOD => {
            parse_and_run(id, request.params, |input| service.claim(input))
        }
        NOTIFICATION_BATCH_VALIDATE_METHOD => {
            parse_and_run(id, request.params, |input| service.validate(input))
        }
        NOTIFICATION_BATCH_ACKNOWLEDGE_METHOD => {
            parse_and_run(id, request.params, |input| service.acknowledge(input))
        }
        NOTIFICATION_BATCH_RELEASE_METHOD => {
            parse_and_run(id, request.params, |input| service.release(input))
        }
        NOTIFICATION_BATCH_SUPPRESS_METHOD => {
            parse_and_run(id, request.params, |input| service.suppress(input))
        }
        NOTIFICATION_LIST_METHOD => parse_and_run(id, request.params, |input| service.list(input)),
        NOTIFICATION_SUMMARY_METHOD => {
            parse_and_run(id, request.params, |input| service.summary(input))
        }
        NOTIFICATION_MARK_SEEN_METHOD => {
            parse_and_run(id, request.params, |input| service.mark_seen(input))
        }
        NOTIFICATION_SETTINGS_GET_METHOD => {
            parse_and_run(id, request.params, |input| service.get_settings(input))
        }
        NOTIFICATION_SETTINGS_UPDATE_METHOD => {
            parse_and_run(id, request.params, |input| service.update_settings(input))
        }
        _ => response_error(Some(id), -32601, "Method not found"),
    }
}

fn parse_and_run<I, O>(
    id: JsonRpcId,
    params: Option<Value>,
    operation: impl FnOnce(I) -> Result<O, NotificationServiceError>,
) -> Value
where
    I: for<'de> Deserialize<'de>,
    O: Serialize,
{
    let input = match parse_params::<I>(params) {
        Ok(value) => value,
        Err(message) => return response_error(Some(id), -32602, message),
    };
    match operation(input) {
        Ok(output) => response_success(id, output),
        Err(error) => serde_json::to_value(error_with_data(
            Some(id),
            -32046,
            &error.message,
            serde_json::json!({"type":"notification_error","code":error.code,"field":error.field}),
        ))
        .expect("notification JSON-RPC error response must serialize"),
    }
}

const _: fn(NotificationBatchesClaimInputDto) = |_| {};
const _: fn(NotificationBatchValidateInputDto) = |_| {};
const _: fn(NotificationBatchAcknowledgeInputDto) = |_| {};
const _: fn(NotificationBatchReleaseInputDto) = |_| {};
const _: fn(NotificationBatchSuppressInputDto) = |_| {};
const _: fn(NotificationListInputDto) = |_| {};
const _: fn(NotificationSummaryInputDto) = |_| {};
const _: fn(NotificationMarkSeenInputDto) = |_| {};
const _: fn(NotificationSettingsGetInputDto) = |_| {};
const _: fn(NotificationSettingsUpdateInputDto) = |_| {};
