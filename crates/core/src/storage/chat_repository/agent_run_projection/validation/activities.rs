use super::*;
fn current_web_source_is_safe(source: &serde_json::Map<String, serde_json::Value>) -> bool {
    exact_required_optional_keys(
        source,
        &["id", "title", "url", "displayUrl", "domain"],
        &["faviconUrl", "snippet", "score", "publishedDate"],
    ) && bounded_string(&source["id"], 1_024, false)
        && bounded_string(&source["title"], 128 * 1_024, true)
        && bounded_string(&source["url"], 16 * 1_024, false)
        && bounded_string(&source["displayUrl"], 16 * 1_024, true)
        && bounded_string(&source["domain"], 4_096, false)
        && optional_bounded_string(source, "faviconUrl", 16 * 1_024, true)
        && optional_bounded_string(source, "snippet", 128 * 1_024, true)
        && source.get("score").is_none_or(serde_json::Value::is_number)
        && optional_bounded_string(source, "publishedDate", 4_096, true)
}

pub(super) fn current_web_activity_is_safe(
    activity: &serde_json::Map<String, serde_json::Value>,
) -> bool {
    exact_required_optional_keys(
        activity,
        &[
            "callId",
            "query",
            "provider",
            "status",
            "sources",
            "updatedAt",
        ],
        &[
            "kind",
            "answer",
            "summaryQuality",
            "error",
            "responseTime",
            "truncated",
        ],
    ) && bounded_string(&activity["callId"], 1_024, false)
        && bounded_string(&activity["query"], 128 * 1_024, true)
        && bounded_string(&activity["provider"], 1_024, false)
        && matches!(
            activity["status"].as_str(),
            Some("running" | "completed" | "failed" | "cancelled")
        )
        && record_array_is_safe(&activity["sources"], current_web_source_is_safe)
        && safe_integer(&activity["updatedAt"])
        && activity
            .get("kind")
            .is_none_or(|value| matches!(value.as_str(), Some("search" | "fetch")))
        && optional_bounded_string(activity, "answer", 4 * 1_024 * 1_024, true)
        && activity
            .get("summaryQuality")
            .is_none_or(|value| matches!(value.as_str(), Some("good" | "low")))
        && optional_bounded_string(activity, "error", 128 * 1_024, true)
        && activity
            .get("responseTime")
            .is_none_or(|value| value.is_null() || value.is_number() || value.is_string())
        && activity
            .get("truncated")
            .is_none_or(serde_json::Value::is_boolean)
}

pub(super) fn current_read_activity_is_safe(
    activity: &serde_json::Map<String, serde_json::Value>,
) -> bool {
    exact_required_optional_keys(
        activity,
        &[
            "callId",
            "tool",
            "kind",
            "status",
            "path",
            "fileName",
            "updatedAt",
        ],
        &[
            "extension",
            "mimeType",
            "thumbnailDataUrl",
            "fullDataUrl",
            "error",
        ],
    ) && bounded_string(&activity["callId"], 1_024, false)
        && bounded_string(&activity["tool"], 1_024, false)
        && matches!(
            activity["kind"].as_str(),
            Some("file" | "image" | "word" | "presentation" | "spreadsheet")
        )
        && matches!(
            activity["status"].as_str(),
            Some("running" | "completed" | "failed" | "cancelled")
        )
        && bounded_string(&activity["path"], 16 * 1_024, false)
        && bounded_string(&activity["fileName"], 4_096, false)
        && safe_integer(&activity["updatedAt"])
        && optional_bounded_string(activity, "extension", 1_024, true)
        && optional_bounded_string(activity, "mimeType", 1_024, true)
        && optional_bounded_string(activity, "thumbnailDataUrl", 32 * 1_024 * 1_024, true)
        && optional_bounded_string(activity, "fullDataUrl", 32 * 1_024 * 1_024, true)
        && optional_bounded_string(activity, "error", 128 * 1_024, true)
}

fn current_timeline_attachment_is_safe(
    attachment: &serde_json::Map<String, serde_json::Value>,
) -> bool {
    exact_required_optional_keys(
        attachment,
        &["id", "kind", "name", "sizeBytes"],
        &[
            "mimeType",
            "encoding",
            "data",
            "previewData",
            "previewMimeType",
            "createdAt",
        ],
    ) && bounded_string(&attachment["id"], 1_024, false)
        && matches!(attachment["kind"].as_str(), Some("file" | "image"))
        && bounded_string(&attachment["name"], 4_096, false)
        && safe_integer(&attachment["sizeBytes"])
        && attachment
            .get("mimeType")
            .is_none_or(|value| value.is_null() || bounded_string(value, 1_024, true))
        && attachment
            .get("encoding")
            .is_none_or(|value| matches!(value.as_str(), Some("utf8" | "base64")))
        && optional_bounded_string(attachment, "data", 32 * 1_024 * 1_024, true)
        && attachment
            .get("previewData")
            .is_none_or(|value| value.is_null() || bounded_string(value, 32 * 1_024 * 1_024, true))
        && attachment
            .get("previewMimeType")
            .is_none_or(|value| value.is_null() || bounded_string(value, 1_024, true))
        && attachment.get("createdAt").is_none_or(safe_integer)
}

pub(super) fn current_timeline_item_is_safe(
    item: &serde_json::Map<String, serde_json::Value>,
) -> bool {
    if !bounded_string(
        item.get("id").unwrap_or(&serde_json::Value::Null),
        1_024,
        false,
    ) {
        return false;
    }
    match item.get("type").and_then(serde_json::Value::as_str) {
        Some("message") => {
            exact_required_optional_keys(
                item,
                &["id", "type", "content"],
                &["streamId", "traceSequence"],
            ) && bounded_string(&item["content"], 4 * 1_024 * 1_024, true)
                && optional_bounded_string(item, "streamId", 1_024, false)
                && optional_safe_integer(item, "traceSequence")
        }
        Some("tool_call") => {
            exact_required_optional_keys(
                item,
                &["id", "type", "callId"],
                &["identity", "traceSequence"],
            ) && bounded_string(&item["callId"], 1_024, false)
                && optional_safe_integer(item, "traceSequence")
                && item.get("identity").is_none_or(|identity| {
                    serde_json::from_value::<crate::AgentToolIdentity>(identity.clone()).is_ok()
                })
        }
        Some("mcp_tool_call") => {
            exact_required_optional_keys(item, &["id", "type", "invocationId"], &["traceSequence"])
                && bounded_string(&item["invocationId"], 1_024, false)
                && optional_safe_integer(item, "traceSequence")
        }
        Some("context_compaction") => {
            exact_required_optional_keys(
                item,
                &["id", "type", "operationId", "status"],
                &["traceSequence"],
            ) && bounded_string(&item["operationId"], 1_024, false)
                && matches!(
                    item["status"].as_str(),
                    Some("running" | "applied" | "skipped" | "failed" | "cancelled")
                )
                && optional_safe_integer(item, "traceSequence")
        }
        Some("error") => {
            exact_required_optional_keys(item, &["id", "type", "message"], &["traceSequence"])
                && bounded_string(&item["message"], 128 * 1_024, true)
                && optional_safe_integer(item, "traceSequence")
        }
        Some("user_guidance") => {
            exact_required_optional_keys(
                item,
                &[
                    "id",
                    "type",
                    "clientMessageId",
                    "content",
                    "attachments",
                    "status",
                    "createdAt",
                ],
                &[
                    "guidanceId",
                    "rejectionCode",
                    "error",
                    "recoverable",
                    "sequence",
                    "traceSequence",
                ],
            ) && bounded_string(&item["clientMessageId"], 1_024, false)
                && bounded_string(&item["content"], 4 * 1_024 * 1_024, true)
                && record_array_is_safe(&item["attachments"], current_timeline_attachment_is_safe)
                && matches!(
                    item["status"].as_str(),
                    Some("submitting" | "queued" | "applied" | "rejected")
                )
                && safe_integer(&item["createdAt"])
                && optional_bounded_string(item, "guidanceId", 1_024, false)
                && optional_bounded_string(item, "rejectionCode", 1_024, false)
                && optional_bounded_string(item, "error", 128 * 1_024, true)
                && item
                    .get("recoverable")
                    .is_none_or(serde_json::Value::is_boolean)
                && item.get("sequence").is_none_or(safe_integer)
                && optional_safe_integer(item, "traceSequence")
        }
        _ => false,
    }
}

pub(super) fn current_message_stream_checkpoints_are_safe(value: &serde_json::Value) -> bool {
    value.as_object().is_some_and(|checkpoints| {
        checkpoints.len() <= MAX_CURRENT_RUN_ITEMS
            && checkpoints.values().all(|checkpoint| {
                checkpoint.as_object().is_some_and(|checkpoint| {
                    exact_keys(checkpoint, &["previousContent"])
                        && bounded_string(&checkpoint["previousContent"], 4 * 1_024 * 1_024, true)
                })
            })
    })
}

pub(super) fn current_interruption_is_safe(value: &serde_json::Value) -> bool {
    let Some(interruption) = value.as_object() else {
        return false;
    };
    exact_keys(interruption, &["reason"])
        && matches!(
            interruption["reason"].as_str(),
            Some(
                "service_connection_failed"
                    | "service_unavailable"
                    | "authentication_failed"
                    | "quota_exhausted"
                    | "context_limit_exceeded"
                    | "request_rejected"
                    | "response_invalid"
                    | "output_limit_reached"
                    | "empty_response"
                    | "stream_interrupted"
                    | "request_failed"
                    | "admission_unconfirmed"
            )
        )
}

pub(super) fn current_mcp_invocation_is_safe(value: &serde_json::Value) -> bool {
    const REQUIRED: &[&str] = &[
        "actionId",
        "invocationId",
        "callId",
        "serverId",
        "serverDisplayName",
        "rawToolName",
        "modelToolName",
        "external",
        "state",
        "dispatchCertainty",
        "outputTruncated",
    ];
    const OPTIONAL: &[&str] = &[
        "scope",
        "displayReason",
        "outcome",
        "isError",
        "errorCode",
        "rejectionReason",
        "durationMs",
    ];
    let Some(invocation) = value.as_object() else {
        return false;
    };
    if !REQUIRED.iter().all(|field| invocation.contains_key(*field))
        || !invocation
            .keys()
            .all(|key| REQUIRED.contains(&key.as_str()) || OPTIONAL.contains(&key.as_str()))
    {
        return false;
    }
    for field in [
        "actionId",
        "invocationId",
        "callId",
        "serverId",
        "serverDisplayName",
        "rawToolName",
        "modelToolName",
        "state",
        "dispatchCertainty",
    ] {
        if invocation
            .get(field)
            .and_then(serde_json::Value::as_str)
            .is_none_or(|value| value.is_empty())
        {
            return false;
        }
    }
    if invocation.get("external") != Some(&serde_json::Value::Bool(true))
        || !invocation
            .get("outputTruncated")
            .is_some_and(serde_json::Value::is_boolean)
    {
        return false;
    }
    if invocation.get("scope").is_some_and(|scope| {
        let Some(scope) = scope.as_object() else {
            return true;
        };
        serde_json::from_value::<AgentMcpServerScope>(serde_json::Value::Object(scope.clone()))
            .is_err()
            || !match scope["type"].as_str() {
                Some("builtin" | "user" | "managed") => exact_keys(scope, &["type"]),
                Some("project") => {
                    exact_keys(scope, &["type", "projectId"])
                        && bounded_string(&scope["projectId"], 1_024, false)
                }
                Some("plugin") => {
                    exact_keys(scope, &["type", "pluginId"])
                        && bounded_string(&scope["pluginId"], 1_024, false)
                }
                _ => false,
            }
    }) {
        return false;
    }
    for field in ["displayReason", "outcome", "errorCode", "rejectionReason"] {
        if invocation
            .get(field)
            .is_some_and(|value| !value.is_string())
        {
            return false;
        }
    }
    if invocation
        .get("isError")
        .is_some_and(|value| !value.is_boolean())
        || invocation
            .get("durationMs")
            .is_some_and(|value| !safe_integer(value))
        || !bounded_string(&invocation["callId"], 1_024, false)
        || !bounded_string(&invocation["serverDisplayName"], 1_024, false)
        || !bounded_string(&invocation["rawToolName"], 1_024, false)
        || !bounded_string(&invocation["modelToolName"], 1_024, false)
        || !optional_bounded_string(invocation, "displayReason", 512, false)
        || !optional_bounded_string(invocation, "errorCode", 1_024, false)
        || !optional_bounded_string(invocation, "rejectionReason", 512, false)
    {
        return false;
    }
    let Some(action_id) = invocation["actionId"].as_str() else {
        return false;
    };
    let Some(invocation_id) = invocation["invocationId"].as_str() else {
        return false;
    };
    let Some(server_id) = invocation["serverId"].as_str() else {
        return false;
    };
    if !current_uuid_is_safe(action_id)
        || !current_uuid_is_safe(invocation_id)
        || !current_uuid_is_safe(server_id)
        || action_id == invocation_id
    {
        return false;
    }
    let state = invocation["state"].as_str();
    let dispatch = invocation["dispatchCertainty"].as_str();
    let outcome = invocation
        .get("outcome")
        .and_then(serde_json::Value::as_str);
    let is_error = invocation
        .get("isError")
        .and_then(serde_json::Value::as_bool);
    let has_error_code = invocation.contains_key("errorCode");
    let has_duration = invocation.contains_key("durationMs");
    let truncated = invocation["outputTruncated"].as_bool().unwrap_or(false);
    match state {
        Some("pending_approval" | "approved") => {
            dispatch == Some("definitely_not_dispatched")
                && outcome.is_none()
                && is_error.is_none()
                && !has_error_code
                && !has_duration
                && !truncated
        }
        Some("dispatching" | "running") => {
            dispatch == Some("possibly_dispatched")
                && outcome.is_none()
                && is_error.is_none()
                && !has_error_code
                && !has_duration
                && !truncated
        }
        Some("completed") => {
            dispatch == Some("response_received")
                && has_duration
                && ((outcome == Some("succeeded") && is_error == Some(false) && !has_error_code)
                    || (outcome == Some("tool_error") && is_error == Some(true) && has_error_code))
        }
        Some("failed") => {
            has_duration
                && has_error_code
                && is_error == Some(true)
                && matches!(
                    (outcome, dispatch),
                    (Some("output_too_large"), Some("response_received"))
                        | (Some("timed_out"), Some("definitely_not_dispatched"))
                        | (Some("transport_error"), Some("definitely_not_dispatched"))
                        | (Some("transport_error"), Some("response_received"))
                )
                && (dispatch != Some("definitely_not_dispatched") || !truncated)
        }
        Some("cancelled") => {
            dispatch == Some("definitely_not_dispatched")
                && outcome == Some("cancelled")
                && is_error.is_none()
                && has_error_code
                && !truncated
        }
        Some("rejected") => {
            dispatch == Some("definitely_not_dispatched")
                && outcome == Some("rejected")
                && is_error.is_none()
                && has_error_code
                && !truncated
        }
        Some("expired") => {
            dispatch == Some("definitely_not_dispatched")
                && outcome == Some("expired")
                && is_error.is_none()
                && has_error_code
                && !truncated
        }
        Some("payload_unavailable") => {
            dispatch == Some("definitely_not_dispatched")
                && outcome == Some("payload_unavailable")
                && is_error == Some(true)
                && has_error_code
                && !truncated
        }
        Some("policy_denied") => {
            dispatch == Some("definitely_not_dispatched")
                && outcome == Some("policy_denied")
                && is_error.is_none()
                && has_error_code
                && !truncated
        }
        Some("outcome_unknown") => {
            dispatch == Some("possibly_dispatched")
                && outcome == Some("outcome_unknown")
                && is_error.is_none()
                && has_error_code
                && !truncated
        }
        _ => false,
    }
}

pub(super) fn current_collaboration_timeline_activity_is_safe(
    activity: &serde_json::Map<String, serde_json::Value>,
) -> bool {
    exact_keys(
        activity,
        &[
            "activityId",
            "agentId",
            "occurredAt",
            "parentAgentId",
            "parentConversationId",
            "anchorMessageId",
            "traceBoundarySequence",
            "runId",
            "semantic",
            "sequence",
            "taskNameSnapshot",
            "turnId",
        ],
    ) && bounded_string(&activity["activityId"], 2_048, false)
        && bounded_string(&activity["agentId"], 256, false)
        && safe_integer(&activity["occurredAt"])
        && bounded_string(&activity["parentAgentId"], 256, false)
        && bounded_string(&activity["parentConversationId"], 256, false)
        && bounded_string(&activity["anchorMessageId"], 2_048, false)
        && safe_integer(&activity["traceBoundarySequence"])
        && nullable_bounded_string(&activity["runId"], 2_048, false)
        && matches!(
            activity["semantic"].as_str(),
            Some(
                "started" | "updated" | "waiting_approval" | "completed" | "failed" | "interrupted"
            )
        )
        && safe_integer(&activity["sequence"])
        && bounded_string(&activity["taskNameSnapshot"], 256, false)
        && nullable_bounded_string(&activity["turnId"], 2_048, false)
}
