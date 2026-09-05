//! Central policy for projecting run-scoped tool data into durable conversation history.
//!
//! Runtime observations, approval checkpoints, and presentation events deliberately do not use
//! these limits. A tool executes once; this module only derives the bounded, provider-neutral view
//! used by audit/search. Replayable model context is persisted separately and is required for
//! every current Assistant turn.

use serde_json::{json, Map, Value};

pub(crate) const BINARY_OMITTED_MARKER: &str = "[binary/base64 omitted]";
const TRUNCATED_SUFFIX: &str = "\n...[truncated for conversation history]";
const EARLIER_OUTPUT_OMITTED_PREFIX: &str = "[earlier output omitted]\n";
const BINARY_OMITTED_FROM_HISTORY_KEY: &str = "binaryomittedfromhistory";

/// Every durable trace limit lives here so projection behavior can be reviewed and tested as one
/// contract instead of accumulating tool-local magic numbers.
pub(crate) struct DurableTraceProjectionLimits;

impl DurableTraceProjectionLimits {
    pub(crate) const ASSISTANT_NARRATION_CHARS: usize = 8_000;
    pub(crate) const USER_GUIDANCE_CHARS: usize = 12_000;
    pub(crate) const TERMINAL_ERROR_CHARS: usize = 2_000;
    pub(crate) const TOOL_ERROR_CHARS: usize = 2_000;
    pub(crate) const PATH_CHARS: usize = 2_048;
    pub(crate) const URL_CHARS: usize = 4_096;
    pub(crate) const QUERY_CHARS: usize = 2_000;
    pub(crate) const TITLE_CHARS: usize = 512;
    pub(crate) const SUMMARY_CHARS: usize = 4_000;
    pub(crate) const READ_SUMMARY_CHARS: usize = 1_200;
    pub(crate) const COMMAND_CHARS: usize = 2_000;
    pub(crate) const COMMAND_OUTPUT_TAIL_CHARS: usize = 3_000;
    pub(crate) const GENERIC_STRING_CHARS: usize = 2_000;
    pub(crate) const GENERIC_ARRAY_ITEMS: usize = 32;
    pub(crate) const GENERIC_OBJECT_FIELDS: usize = 64;
    pub(crate) const GENERIC_DEPTH: usize = 8;
    pub(crate) const WEB_SEARCH_RESULTS: usize = 10;
    pub(crate) const WEB_FAILED_RESULTS: usize = 10;
}

#[derive(Debug)]
pub(crate) struct ProjectedValue {
    pub(crate) value: Value,
    pub(crate) truncated: bool,
}

impl ProjectedValue {
    fn new(value: Value, truncated: bool) -> Self {
        Self { value, truncated }
    }
}

pub(crate) fn project_narration(value: &str) -> (String, bool) {
    sanitize_and_bound_text(
        value,
        DurableTraceProjectionLimits::ASSISTANT_NARRATION_CHARS,
        false,
    )
}

pub(crate) fn project_user_guidance(value: &str) -> (String, bool) {
    sanitize_and_bound_text(
        value,
        DurableTraceProjectionLimits::USER_GUIDANCE_CHARS,
        false,
    )
}

pub(crate) fn project_terminal_error(value: &str) -> (String, bool) {
    sanitize_and_bound_text(
        value,
        DurableTraceProjectionLimits::TERMINAL_ERROR_CHARS,
        false,
    )
}

pub(crate) fn project_attachment_text(value: &str) -> (String, bool) {
    sanitize_and_bound_text(value, DurableTraceProjectionLimits::PATH_CHARS, false)
}

/// Binary-only sanitizer used by current-turn model/checkpoint projections. It intentionally does
/// not apply durable history length limits.
pub(crate) fn sanitize_runtime_text(value: &str) -> (String, bool) {
    if value.trim_start().to_ascii_lowercase().starts_with("data:")
        && value.to_ascii_lowercase().contains(";base64,")
    {
        return (BINARY_OMITTED_MARKER.to_string(), true);
    }
    (value.to_string(), false)
}

/// Binary-only sanitizer used by current-turn model/checkpoint projections. It intentionally does
/// not apply durable history length limits.
pub(crate) fn sanitize_runtime_value(value: &Value) -> (Value, bool) {
    sanitize_runtime_value_inner(value)
}

/// Human answers are bounded text supplied by the trusted resume port. Their exact display facts
/// must survive runtime, checkpoint and history projection; unrelated tool sanitization is intact.
pub(crate) fn sanitize_runtime_tool_result(tool: &str, value: &Value) -> (Value, bool) {
    if tool == "request_user_input"
        && crate::human_interaction::HumanInteractionResponseDisplay::valid_value(value)
    {
        return (value.clone(), false);
    }
    sanitize_runtime_value(value)
}

pub(crate) fn project_tool_call(tool: &str, operation: &Value) -> ProjectedValue {
    let projected = match tool {
        "web_fetch" => project_selected_object(
            operation,
            &[
                ("url", DurableTraceProjectionLimits::URL_CHARS),
                ("query", DurableTraceProjectionLimits::QUERY_CHARS),
                (
                    "chunksPerSource",
                    DurableTraceProjectionLimits::GENERIC_STRING_CHARS,
                ),
                (
                    "extractDepth",
                    DurableTraceProjectionLimits::GENERIC_STRING_CHARS,
                ),
                ("format", DurableTraceProjectionLimits::GENERIC_STRING_CHARS),
                (
                    "maxChars",
                    DurableTraceProjectionLimits::GENERIC_STRING_CHARS,
                ),
            ],
        ),
        "web_search" => project_selected_object(
            operation,
            &[
                ("query", DurableTraceProjectionLimits::QUERY_CHARS),
                (
                    "maxResults",
                    DurableTraceProjectionLimits::GENERIC_STRING_CHARS,
                ),
                (
                    "searchDepth",
                    DurableTraceProjectionLimits::GENERIC_STRING_CHARS,
                ),
                ("topic", DurableTraceProjectionLimits::GENERIC_STRING_CHARS),
                (
                    "timeRange",
                    DurableTraceProjectionLimits::GENERIC_STRING_CHARS,
                ),
                (
                    "includeDomains",
                    DurableTraceProjectionLimits::GENERIC_STRING_CHARS,
                ),
                (
                    "excludeDomains",
                    DurableTraceProjectionLimits::GENERIC_STRING_CHARS,
                ),
            ],
        ),
        "apply_patch" => project_apply_patch_call(operation),
        "run_command" => {
            // This operation is also verified against the frozen approval action during recovery.
            // Its schema already caps command/reason/paths; retain all fields exactly so durable
            // history cannot weaken that proof. Result streams are still bounded below.
            sanitize_runtime_value(operation)
        }
        tool if is_manually_audited_operation(tool) => {
            // Same rationale as run_command: these schemas are bounded and the exact semantic
            // operation is part of the authoritative settlement proof.
            sanitize_runtime_value(operation)
        }
        tool if is_read_tool(tool) => project_read_call(operation),
        _ => project_generic_value(operation),
    };
    ProjectedValue::new(projected.0, projected.1)
}

pub(crate) fn project_tool_result(
    tool: &str,
    operation: Option<&Value>,
    observation: &Value,
    error: Option<&str>,
) -> (ProjectedValue, Option<String>, bool) {
    let projected = match tool {
        "request_user_input" => project_human_interaction_response(observation),
        "web_fetch" => project_web_fetch_result(observation),
        "web_search" => project_web_search_result(observation),
        "apply_patch" => project_apply_patch_result(operation, observation),
        "run_command" => project_run_command_result(observation),
        "command_session" => project_command_session_result(observation),
        "conversation_history" => project_conversation_history_result(observation),
        "skills_read_resource" => project_skill_resource_result(observation),
        tool if is_read_tool(tool) => project_read_result(observation),
        _ => project_generic_value(observation),
    };
    let (error, error_truncated) = error
        .map(|error| {
            sanitize_and_bound_text(error, DurableTraceProjectionLimits::TOOL_ERROR_CHARS, false)
        })
        .map(|(error, truncated)| (Some(error), truncated))
        .unwrap_or((None, false));
    (
        ProjectedValue::new(projected.0, projected.1),
        error,
        error_truncated,
    )
}

fn project_human_interaction_response(value: &Value) -> (Value, bool) {
    if crate::human_interaction::HumanInteractionResponseDisplay::valid_value(value) {
        (value.clone(), false)
    } else {
        project_generic_value(value)
    }
}

pub(crate) fn project_conversation_history_result(value: &Value) -> (Value, bool) {
    let Some(input) = value.as_object() else {
        return project_generic_value(value);
    };
    let mut metadata = input.clone();
    let returned_chars = metadata.remove("content").and_then(|content| {
        content
            .as_str()
            .map(|content| content.chars().count() as u64)
    });
    if let Some(returned_chars) = returned_chars {
        metadata.insert("returnedChars".to_string(), json!(returned_chars));
        metadata.insert(
            "contentOmittedFromConversationTrace".to_string(),
            json!(true),
        );
    }
    let mut historical_payload_omitted = returned_chars.is_some();
    for (key, count_key) in [
        ("turns", "returnedTurns"),
        ("results", "returnedTurns"),
        ("timeline", "returnedRecords"),
    ] {
        if let Some(items) = metadata
            .remove(key)
            .and_then(|value| value.as_array().cloned())
        {
            metadata
                .entry(count_key.to_string())
                .or_insert_with(|| json!(items.len()));
            historical_payload_omitted |= !items.is_empty();
        }
    }
    if metadata.remove("turn").is_some() {
        historical_payload_omitted = true;
    }
    if let Some(hits) = metadata.get_mut("hits").and_then(Value::as_array_mut) {
        for hit in hits {
            if let Some(hit) = hit.as_object_mut() {
                historical_payload_omitted |= hit.remove("preview").is_some();
                historical_payload_omitted |= hit.remove("snippet").is_some();
            }
        }
    }
    if let Some(records) = metadata.get_mut("records").and_then(Value::as_array_mut) {
        let returned_records = records.len() as u64;
        metadata.remove("records");
        metadata
            .entry("returnedRecords".to_string())
            .or_insert_with(|| json!(returned_records));
        historical_payload_omitted |= returned_records > 0;
    }
    if historical_payload_omitted {
        metadata.insert(
            "historicalPayloadOmittedFromConversationTrace".to_string(),
            json!(true),
        );
    }
    let (value, projected_truncated) = project_generic_value(&Value::Object(metadata));
    (value, historical_payload_omitted || projected_truncated)
}

pub(crate) fn is_repeat_failure_eligible(tool: &str) -> bool {
    matches!(
        tool,
        "web_fetch" | "web_search" | "search_code" | "search_files"
    ) || is_read_tool(tool)
}

fn is_manually_audited_operation(tool: &str) -> bool {
    matches!(
        tool,
        "office_document"
            | "office_spreadsheet"
            | "office_presentation"
            | "skills_materialize_resource"
            | "skills_run_script"
            | "skills_preflight_script"
    )
}

fn is_read_tool(tool: &str) -> bool {
    tool.starts_with("read_") || tool == "skills_read_resource"
}

fn project_apply_patch_call(value: &Value) -> (Value, bool) {
    let Some(input) = crate::tools::apply_patch_request(value) else {
        return (json!({ "request": {} }), true);
    };
    if !crate::tools::apply_patch_wire_is_valid(value)
        && !is_host_validated_apply_patch_projection(input)
    {
        return (json!({ "request": {} }), true);
    }
    let mut output = Map::new();
    // This marker is Host-owned: the strict model-visible apply_patch Wire rejects it. Keeping it
    // in the durable projection makes the projection idempotent when an approval continuation
    // appends a new ToolCall to an already-projected Trace prefix.
    output.insert("validatedRequest".into(), Value::Bool(true));
    let mut truncated = false;
    for (key, limit) in [
        ("action", DurableTraceProjectionLimits::TITLE_CHARS),
        (
            "operation",
            DurableTraceProjectionLimits::GENERIC_STRING_CHARS,
        ),
        ("filePath", DurableTraceProjectionLimits::PATH_CHARS),
        (
            "transactionId",
            DurableTraceProjectionLimits::GENERIC_STRING_CHARS,
        ),
        (
            "expectedDraftRevision",
            DurableTraceProjectionLimits::GENERIC_STRING_CHARS,
        ),
        ("index", DurableTraceProjectionLimits::GENERIC_STRING_CHARS),
        ("strategy", DurableTraceProjectionLimits::TITLE_CHARS),
        ("summary", DurableTraceProjectionLimits::SUMMARY_CHARS),
        (
            "contentBytes",
            DurableTraceProjectionLimits::GENERIC_STRING_CHARS,
        ),
        (
            "editCount",
            DurableTraceProjectionLimits::GENERIC_STRING_CHARS,
        ),
        (
            "contentDigest",
            DurableTraceProjectionLimits::GENERIC_STRING_CHARS,
        ),
        (
            "editsDigest",
            DurableTraceProjectionLimits::GENERIC_STRING_CHARS,
        ),
    ] {
        copy_bounded_field(input, &mut output, key, limit, &mut truncated);
    }

    let (representation, additions, deletions) = if let Some(content) =
        input.get("content").and_then(Value::as_str)
    {
        output.insert(
            "contentBytes".into(),
            json!(u64::try_from(content.len()).unwrap_or(u64::MAX)),
        );
        output.insert(
            "contentDigest".into(),
            json!(crate::file_change::content_digest(content.as_bytes())),
        );
        ("content", line_count(content), 0)
    } else if let Some(edits) = input.get("edits").and_then(Value::as_array) {
        let (additions, deletions) = structured_edit_stats(edits);
        output.insert("editCount".into(), json!(edits.len()));
        if let Ok(digest) = crate::file_change::proposal_digest(&Value::Array(edits.clone())) {
            output.insert("editsDigest".into(), json!(digest));
        }
        ("edits", additions, deletions)
    } else if let Some(representation) = input.get("changeRepresentation").and_then(Value::as_str) {
        (
            representation,
            input.get("additions").and_then(Value::as_u64).unwrap_or(0),
            input.get("deletions").and_then(Value::as_u64).unwrap_or(0),
        )
    } else if input.get("editCount").and_then(Value::as_u64).is_some() {
        ("edits", 0, 0)
    } else {
        ("metadata_only", 0, 0)
    };
    output.insert("changeRepresentation".into(), json!(representation));
    output.insert("additions".into(), json!(additions));
    output.insert("deletions".into(), json!(deletions));
    truncated |= representation != "metadata_only";
    (json!({ "request": Value::Object(output) }), truncated)
}

fn is_host_validated_apply_patch_projection(input: &Map<String, Value>) -> bool {
    input.get("validatedRequest").and_then(Value::as_bool) == Some(true)
        && input.keys().all(|key| {
            matches!(
                key.as_str(),
                "validatedRequest"
                    | "action"
                    | "operation"
                    | "filePath"
                    | "strategy"
                    | "transactionId"
                    | "index"
                    | "expectedDraftRevision"
                    | "contentBytes"
                    | "contentDigest"
                    | "editCount"
                    | "editsDigest"
                    | "changeRepresentation"
                    | "additions"
                    | "deletions"
                    | "summary"
            )
        })
}

fn project_read_call(value: &Value) -> (Value, bool) {
    project_selected_object(
        value,
        &[
            ("path", DurableTraceProjectionLimits::PATH_CHARS),
            ("filePath", DurableTraceProjectionLimits::PATH_CHARS),
            ("resourceUri", DurableTraceProjectionLimits::PATH_CHARS),
            (
                "startLine",
                DurableTraceProjectionLimits::GENERIC_STRING_CHARS,
            ),
            (
                "startByte",
                DurableTraceProjectionLimits::GENERIC_STRING_CHARS,
            ),
            (
                "maxLines",
                DurableTraceProjectionLimits::GENERIC_STRING_CHARS,
            ),
            (
                "maxChars",
                DurableTraceProjectionLimits::GENERIC_STRING_CHARS,
            ),
            ("range", DurableTraceProjectionLimits::GENERIC_STRING_CHARS),
            ("sheet", DurableTraceProjectionLimits::TITLE_CHARS),
        ],
    )
}

fn project_web_fetch_result(value: &Value) -> (Value, bool) {
    let Some(input) = value.as_object() else {
        return project_generic_value(value);
    };
    let mut output = Map::new();
    let mut truncated = false;
    for (key, limit) in [
        ("url", DurableTraceProjectionLimits::URL_CHARS),
        ("requestedUrl", DurableTraceProjectionLimits::URL_CHARS),
        ("title", DurableTraceProjectionLimits::TITLE_CHARS),
        ("provider", DurableTraceProjectionLimits::TITLE_CHARS),
        ("source", DurableTraceProjectionLimits::URL_CHARS),
        ("status", DurableTraceProjectionLimits::TITLE_CHARS),
        ("code", DurableTraceProjectionLimits::TITLE_CHARS),
        ("errorCode", DurableTraceProjectionLimits::TITLE_CHARS),
        ("message", DurableTraceProjectionLimits::TOOL_ERROR_CHARS),
        ("recovery", DurableTraceProjectionLimits::TITLE_CHARS),
        ("format", DurableTraceProjectionLimits::TITLE_CHARS),
        ("extractDepth", DurableTraceProjectionLimits::TITLE_CHARS),
        (
            "responseTime",
            DurableTraceProjectionLimits::GENERIC_STRING_CHARS,
        ),
        (
            "sourceStopReason",
            DurableTraceProjectionLimits::TITLE_CHARS,
        ),
    ] {
        copy_bounded_field(input, &mut output, key, limit, &mut truncated);
    }
    for key in [
        "contentCoverage",
        "imagesCoverage",
        "failedResultsCoverage",
        "truncatedAtSource",
        "omittedBytes",
    ] {
        copy_bounded_field(
            input,
            &mut output,
            key,
            DurableTraceProjectionLimits::GENERIC_STRING_CHARS,
            &mut truncated,
        );
    }
    if let Some(content) = input
        .get("summary")
        .or_else(|| input.get("content"))
        .or_else(|| input.get("text"))
        .and_then(Value::as_str)
    {
        let (summary, summary_truncated) =
            sanitize_and_bound_text(content, DurableTraceProjectionLimits::SUMMARY_CHARS, false);
        output.insert("summary".into(), json!(summary));
        output.insert(
            "bodyTruncatedInHistory".into(),
            json!(
                summary_truncated
                    || input.get("summary").is_none()
                    || input
                        .get("bodyTruncatedInHistory")
                        .and_then(Value::as_bool)
                        .unwrap_or(false)
            ),
        );
        truncated |= summary_truncated
            || input.get("summary").is_none()
            || input
                .get("bodyTruncatedInHistory")
                .and_then(Value::as_bool)
                .unwrap_or(false);
    }
    if let Some(failed) = input.get("failedResults").and_then(Value::as_array) {
        let selected = failed
            .iter()
            .take(DurableTraceProjectionLimits::WEB_FAILED_RESULTS)
            .map(project_generic_value)
            .map(|(value, item_truncated)| {
                truncated |= item_truncated;
                value
            })
            .collect::<Vec<_>>();
        truncated |= failed.len() > selected.len();
        output.insert("failedResults".into(), Value::Array(selected));
    }
    let source_truncated = input
        .get("truncated")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    output.insert("truncated".into(), json!(source_truncated || truncated));
    (Value::Object(output), truncated)
}

fn project_web_search_result(value: &Value) -> (Value, bool) {
    let Some(input) = value.as_object() else {
        return project_generic_value(value);
    };
    let mut output = Map::new();
    let mut truncated = false;
    for (key, limit) in [
        ("query", DurableTraceProjectionLimits::QUERY_CHARS),
        ("provider", DurableTraceProjectionLimits::TITLE_CHARS),
        ("status", DurableTraceProjectionLimits::TITLE_CHARS),
        ("code", DurableTraceProjectionLimits::TITLE_CHARS),
        ("errorCode", DurableTraceProjectionLimits::TITLE_CHARS),
        ("message", DurableTraceProjectionLimits::TOOL_ERROR_CHARS),
        ("recovery", DurableTraceProjectionLimits::TITLE_CHARS),
        ("answer", DurableTraceProjectionLimits::SUMMARY_CHARS),
        ("contentKind", DurableTraceProjectionLimits::TITLE_CHARS),
        ("fullContentTool", DurableTraceProjectionLimits::TITLE_CHARS),
        (
            "sourceCompleteness",
            DurableTraceProjectionLimits::TITLE_CHARS,
        ),
        (
            "requestedMaxResults",
            DurableTraceProjectionLimits::GENERIC_STRING_CHARS,
        ),
        ("cursor", DurableTraceProjectionLimits::GENERIC_STRING_CHARS),
        ("next", DurableTraceProjectionLimits::GENERIC_STRING_CHARS),
        (
            "nextCursor",
            DurableTraceProjectionLimits::GENERIC_STRING_CHARS,
        ),
        (
            "continueWith",
            DurableTraceProjectionLimits::GENERIC_STRING_CHARS,
        ),
        (
            "truncatedAtSource",
            DurableTraceProjectionLimits::GENERIC_STRING_CHARS,
        ),
        (
            "omittedBytes",
            DurableTraceProjectionLimits::GENERIC_STRING_CHARS,
        ),
        (
            "sourceStopReason",
            DurableTraceProjectionLimits::TITLE_CHARS,
        ),
        (
            "responseTime",
            DurableTraceProjectionLimits::GENERIC_STRING_CHARS,
        ),
    ] {
        copy_bounded_field(input, &mut output, key, limit, &mut truncated);
    }
    if let Some(results) = input.get("results").and_then(Value::as_array) {
        let selected = results
            .iter()
            .take(DurableTraceProjectionLimits::WEB_SEARCH_RESULTS)
            .map(project_web_search_entry)
            .map(|(entry, entry_truncated)| {
                truncated |= entry_truncated;
                entry
            })
            .collect::<Vec<_>>();
        truncated |= selected.len() < results.len();
        output.insert(
            "resultCoverage".into(),
            json!({
                "total": results.len(),
                "returned": selected.len(),
                "omitted": results.len().saturating_sub(selected.len())
            }),
        );
        output.insert("results".into(), Value::Array(selected));
    }
    let source_truncated = input
        .get("truncatedAtSource")
        .or_else(|| input.get("truncated"))
        .and_then(Value::as_bool);
    if source_truncated.is_some() || truncated {
        output.insert(
            "truncated".into(),
            json!(source_truncated.unwrap_or(false) || truncated),
        );
    }
    (Value::Object(output), truncated)
}

fn project_web_search_entry(value: &Value) -> (Value, bool) {
    project_selected_object(
        value,
        &[
            ("title", DurableTraceProjectionLimits::TITLE_CHARS),
            ("url", DurableTraceProjectionLimits::URL_CHARS),
            ("source", DurableTraceProjectionLimits::URL_CHARS),
            ("content", DurableTraceProjectionLimits::READ_SUMMARY_CHARS),
            ("publishedDate", DurableTraceProjectionLimits::TITLE_CHARS),
            ("score", DurableTraceProjectionLimits::GENERIC_STRING_CHARS),
        ],
    )
}

fn project_apply_patch_result(_operation: Option<&Value>, value: &Value) -> (Value, bool) {
    let Some(input) = value.as_object() else {
        return project_generic_value(value);
    };
    let mut output = Map::new();
    let mut truncated = false;
    for (key, limit) in [
        (
            "transactionId",
            DurableTraceProjectionLimits::GENERIC_STRING_CHARS,
        ),
        ("status", DurableTraceProjectionLimits::TITLE_CHARS),
        ("outcome", DurableTraceProjectionLimits::TITLE_CHARS),
        (
            "operation",
            DurableTraceProjectionLimits::GENERIC_STRING_CHARS,
        ),
        ("strategy", DurableTraceProjectionLimits::TITLE_CHARS),
        ("updateStrategy", DurableTraceProjectionLimits::TITLE_CHARS),
        ("filePath", DurableTraceProjectionLimits::PATH_CHARS),
        (
            "revision",
            DurableTraceProjectionLimits::GENERIC_STRING_CHARS,
        ),
        ("errorCode", DurableTraceProjectionLimits::TITLE_CHARS),
        ("error", DurableTraceProjectionLimits::TOOL_ERROR_CHARS),
        ("message", DurableTraceProjectionLimits::SUMMARY_CHARS),
    ] {
        copy_bounded_field(input, &mut output, key, limit, &mut truncated);
    }
    for key in [
        "schemaVersion",
        "additions",
        "deletions",
        "lineCount",
        "byteCount",
        "draftRevision",
        "nextIndex",
        "mutationCount",
    ] {
        if let Some(value) = input.get(key).filter(|value| value.is_u64()) {
            output.insert(key.to_string(), value.clone());
        }
    }
    (Value::Object(output), truncated)
}

fn project_run_command_result(value: &Value) -> (Value, bool) {
    let Some(input) = value.as_object() else {
        return project_generic_value(value);
    };
    if input.get("status").and_then(Value::as_str) == Some("running") {
        return project_running_command_result(input);
    }
    let mut output = Map::new();
    let mut truncated = false;
    for (key, limit) in [
        ("status", DurableTraceProjectionLimits::TITLE_CHARS),
        ("command", DurableTraceProjectionLimits::COMMAND_CHARS),
        ("cwd", DurableTraceProjectionLimits::PATH_CHARS),
        (
            "exitCode",
            DurableTraceProjectionLimits::GENERIC_STRING_CHARS,
        ),
        (
            "timedOut",
            DurableTraceProjectionLimits::GENERIC_STRING_CHARS,
        ),
        (
            "cancelled",
            DurableTraceProjectionLimits::GENERIC_STRING_CHARS,
        ),
        (
            "durationMs",
            DurableTraceProjectionLimits::GENERIC_STRING_CHARS,
        ),
    ] {
        copy_bounded_field(input, &mut output, key, limit, &mut truncated);
    }
    for (source_key, target_key, source_truncated_key) in [
        ("stdout", "stdoutTail", "stdoutTruncated"),
        ("stderr", "stderrTail", "stderrTruncated"),
    ] {
        if let Some(stream) = input
            .get(source_key)
            .or_else(|| input.get(target_key))
            .and_then(Value::as_str)
        {
            let (tail, tail_truncated) = sanitize_and_bound_text(
                stream,
                DurableTraceProjectionLimits::COMMAND_OUTPUT_TAIL_CHARS,
                true,
            );
            output.insert(target_key.into(), json!(tail));
            let source_truncated = input
                .get(source_truncated_key)
                .and_then(Value::as_bool)
                .unwrap_or(false);
            output.insert(
                source_truncated_key.into(),
                json!(source_truncated || tail_truncated),
            );
            truncated |= tail_truncated;
        }
    }
    for key in [
        "error",
        "policyEvaluation",
        "artifactObservation",
        "inputFiles",
        "runtime",
    ] {
        if let Some(value) = input.get(key) {
            let (value, value_truncated) = project_generic_value(value);
            output.insert(key.into(), value);
            truncated |= value_truncated;
        }
    }
    (Value::Object(output), truncated)
}

fn project_running_command_result(input: &Map<String, Value>) -> (Value, bool) {
    let mut output = Map::new();
    let mut truncated = false;
    for (key, limit) in [
        ("status", DurableTraceProjectionLimits::TITLE_CHARS),
        ("sessionId", DurableTraceProjectionLimits::TITLE_CHARS),
        (
            "startedAt",
            DurableTraceProjectionLimits::GENERIC_STRING_CHARS,
        ),
        (
            "latestSequence",
            DurableTraceProjectionLimits::GENERIC_STRING_CHARS,
        ),
    ] {
        copy_bounded_field(input, &mut output, key, limit, &mut truncated);
    }

    let source_output_truncated = input
        .get("outputTruncated")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let mut output_projection_truncated = false;
    if let Some(value) = input.get("output").and_then(Value::as_str) {
        let (value, value_truncated) = sanitize_and_bound_text(
            value,
            DurableTraceProjectionLimits::COMMAND_OUTPUT_TAIL_CHARS,
            true,
        );
        output.insert("output".into(), Value::String(value));
        output_projection_truncated = value_truncated;
        truncated |= value_truncated;
    }
    if input.contains_key("outputTruncated") || output_projection_truncated {
        output.insert(
            "outputTruncated".into(),
            Value::Bool(source_output_truncated || output_projection_truncated),
        );
    }

    (Value::Object(output), truncated)
}

fn project_command_session_result(value: &Value) -> (Value, bool) {
    let Some(input) = value.as_object() else {
        // A valid command_session result is always an object. Never fall back to the generic
        // projection here: malformed provider data could otherwise persist command output as an
        // opaque string or array.
        return (Value::Object(Map::new()), !value.is_null());
    };

    let mut output = Map::new();
    let mut truncated = false;
    for (key, limit) in [
        ("sessionId", DurableTraceProjectionLimits::TITLE_CHARS),
        ("status", DurableTraceProjectionLimits::TITLE_CHARS),
        (
            "exitCode",
            DurableTraceProjectionLimits::GENERIC_STRING_CHARS,
        ),
        (
            "latestSequence",
            DurableTraceProjectionLimits::GENERIC_STRING_CHARS,
        ),
        (
            "outputTruncated",
            DurableTraceProjectionLimits::GENERIC_STRING_CHARS,
        ),
    ] {
        copy_bounded_field(input, &mut output, key, limit, &mut truncated);
    }

    if let Some(read) = input.get("read") {
        if let Some(read) = read.as_object() {
            let mut projected_read = Map::new();
            for (key, limit) in [
                (
                    "requestedAfterSequence",
                    DurableTraceProjectionLimits::GENERIC_STRING_CHARS,
                ),
                (
                    "firstSequence",
                    DurableTraceProjectionLimits::GENERIC_STRING_CHARS,
                ),
                (
                    "throughSequence",
                    DurableTraceProjectionLimits::GENERIC_STRING_CHARS,
                ),
                (
                    "truncatedBefore",
                    DurableTraceProjectionLimits::GENERIC_STRING_CHARS,
                ),
                (
                    "outputBytes",
                    DurableTraceProjectionLimits::GENERIC_STRING_CHARS,
                ),
                ("outputHash", DurableTraceProjectionLimits::TITLE_CHARS),
            ] {
                copy_bounded_field(read, &mut projected_read, key, limit, &mut truncated);
            }
            output.insert("read".into(), Value::Object(projected_read));
        } else {
            truncated = true;
        }
    }
    for (key, limit) in [
        ("historyOpen", DurableTraceProjectionLimits::PATH_CHARS),
        ("continueWith", DurableTraceProjectionLimits::PATH_CHARS),
    ] {
        copy_bounded_field(input, &mut output, key, limit, &mut truncated);
    }

    // Poll output is delivered to the current model turn, while terminal settlement archives the
    // Session transcript exactly once. Durable conversation Trace intentionally retains only the
    // delivery receipt above so repeated polls cannot duplicate the body linearly.
    truncated |= input
        .get("output")
        .and_then(Value::as_str)
        .is_some_and(|output| !output.is_empty());

    (Value::Object(output), truncated)
}

fn project_read_result(value: &Value) -> (Value, bool) {
    let Some(input) = value.as_object() else {
        return project_generic_value(value);
    };
    let mut output = Map::new();
    let mut truncated = false;
    for (key, limit) in [
        ("path", DurableTraceProjectionLimits::PATH_CHARS),
        ("resourceUri", DurableTraceProjectionLimits::PATH_CHARS),
        ("status", DurableTraceProjectionLimits::TITLE_CHARS),
        ("code", DurableTraceProjectionLimits::TITLE_CHARS),
        ("errorCode", DurableTraceProjectionLimits::TITLE_CHARS),
        ("message", DurableTraceProjectionLimits::TOOL_ERROR_CHARS),
        ("error", DurableTraceProjectionLimits::TOOL_ERROR_CHARS),
        ("recovery", DurableTraceProjectionLimits::TITLE_CHARS),
        ("format", DurableTraceProjectionLimits::TITLE_CHARS),
        (
            "revision",
            DurableTraceProjectionLimits::GENERIC_STRING_CHARS,
        ),
        ("mimeType", DurableTraceProjectionLimits::TITLE_CHARS),
        (
            "sizeBytes",
            DurableTraceProjectionLimits::GENERIC_STRING_CHARS,
        ),
        (
            "startLine",
            DurableTraceProjectionLimits::GENERIC_STRING_CHARS,
        ),
        (
            "startColumn",
            DurableTraceProjectionLimits::GENERIC_STRING_CHARS,
        ),
        (
            "startByte",
            DurableTraceProjectionLimits::GENERIC_STRING_CHARS,
        ),
        (
            "endLine",
            DurableTraceProjectionLimits::GENERIC_STRING_CHARS,
        ),
        (
            "endColumn",
            DurableTraceProjectionLimits::GENERIC_STRING_CHARS,
        ),
        (
            "endByteExclusive",
            DurableTraceProjectionLimits::GENERIC_STRING_CHARS,
        ),
        (
            "totalLines",
            DurableTraceProjectionLimits::GENERIC_STRING_CHARS,
        ),
        (
            "totalBytes",
            DurableTraceProjectionLimits::GENERIC_STRING_CHARS,
        ),
        (
            "returnedBytes",
            DurableTraceProjectionLimits::GENERIC_STRING_CHARS,
        ),
        (
            "nextStartByte",
            DurableTraceProjectionLimits::GENERIC_STRING_CHARS,
        ),
        (
            "nextStartLine",
            DurableTraceProjectionLimits::GENERIC_STRING_CHARS,
        ),
        (
            "nextStartColumn",
            DurableTraceProjectionLimits::GENERIC_STRING_CHARS,
        ),
        (
            "pageCount",
            DurableTraceProjectionLimits::GENERIC_STRING_CHARS,
        ),
        (
            "sheetCount",
            DurableTraceProjectionLimits::GENERIC_STRING_CHARS,
        ),
        (
            "slideCount",
            DurableTraceProjectionLimits::GENERIC_STRING_CHARS,
        ),
        (
            "partCount",
            DurableTraceProjectionLimits::GENERIC_STRING_CHARS,
        ),
        ("extractor", DurableTraceProjectionLimits::TITLE_CHARS),
        ("truncatedReason", DurableTraceProjectionLimits::TITLE_CHARS),
    ] {
        copy_bounded_field(input, &mut output, key, limit, &mut truncated);
    }
    if let Some(content) = input
        .get("summary")
        .or_else(|| input.get("content"))
        .or_else(|| input.get("text"))
        .and_then(Value::as_str)
    {
        let (summary, summary_truncated) = sanitize_and_bound_text(
            content,
            DurableTraceProjectionLimits::READ_SUMMARY_CHARS,
            false,
        );
        output.insert("summary".into(), json!(summary));
        output.insert(
            "contentTruncatedInHistory".into(),
            json!(
                summary_truncated
                    || input.get("summary").is_none()
                    || input
                        .get("contentTruncatedInHistory")
                        .and_then(Value::as_bool)
                        .unwrap_or(false)
            ),
        );
        truncated |= summary_truncated
            || input.get("summary").is_none()
            || input
                .get("contentTruncatedInHistory")
                .and_then(Value::as_bool)
                .unwrap_or(false);
    }
    let source_truncated = input
        .get("truncated")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    output.insert("truncated".into(), json!(source_truncated || truncated));
    (Value::Object(output), truncated)
}

fn project_skill_resource_result(value: &Value) -> (Value, bool) {
    let Some(input) = value.as_object() else {
        return project_generic_value(value);
    };
    let (mut projected, mut truncated) = project_selected_map(
        input,
        &[
            ("uri", DurableTraceProjectionLimits::PATH_CHARS),
            ("resourceUri", DurableTraceProjectionLimits::PATH_CHARS),
            (
                "startByte",
                DurableTraceProjectionLimits::GENERIC_STRING_CHARS,
            ),
            (
                "endByteExclusive",
                DurableTraceProjectionLimits::GENERIC_STRING_CHARS,
            ),
            (
                "totalBytes",
                DurableTraceProjectionLimits::GENERIC_STRING_CHARS,
            ),
            (
                "returnedBytes",
                DurableTraceProjectionLimits::GENERIC_STRING_CHARS,
            ),
            (
                "nextStartByte",
                DurableTraceProjectionLimits::GENERIC_STRING_CHARS,
            ),
            ("status", DurableTraceProjectionLimits::TITLE_CHARS),
            ("code", DurableTraceProjectionLimits::TITLE_CHARS),
            ("errorCode", DurableTraceProjectionLimits::TITLE_CHARS),
            ("message", DurableTraceProjectionLimits::TOOL_ERROR_CHARS),
            ("error", DurableTraceProjectionLimits::TOOL_ERROR_CHARS),
            ("recovery", DurableTraceProjectionLimits::TITLE_CHARS),
            (
                "contentOmittedFromHistory",
                DurableTraceProjectionLimits::GENERIC_STRING_CHARS,
            ),
        ],
    );
    if input.contains_key("content") || input.contains_key("text") {
        if let Some(output) = projected.as_object_mut() {
            output.insert("contentOmittedFromHistory".to_string(), Value::Bool(true));
            truncated = true;
        }
    }
    (projected, truncated)
}

fn project_selected_object(value: &Value, fields: &[(&str, usize)]) -> (Value, bool) {
    let Some(input) = value.as_object() else {
        return project_generic_value(value);
    };
    project_selected_map(input, fields)
}

fn project_selected_map(input: &Map<String, Value>, fields: &[(&str, usize)]) -> (Value, bool) {
    let mut output = Map::new();
    let mut truncated = false;
    for (key, limit) in fields {
        copy_bounded_field(input, &mut output, key, *limit, &mut truncated);
    }
    (Value::Object(output), truncated)
}

fn copy_bounded_field(
    input: &Map<String, Value>,
    output: &mut Map<String, Value>,
    key: &str,
    string_limit: usize,
    truncated: &mut bool,
) {
    let Some(value) = input.get(key) else {
        return;
    };
    let (value, value_truncated) = bound_value(value, string_limit, 0);
    output.insert(key.to_string(), value);
    *truncated |= value_truncated;
}

fn project_generic_value(value: &Value) -> (Value, bool) {
    bound_value(value, DurableTraceProjectionLimits::GENERIC_STRING_CHARS, 0)
}

fn bound_value(value: &Value, string_limit: usize, depth: usize) -> (Value, bool) {
    if depth >= DurableTraceProjectionLimits::GENERIC_DEPTH {
        return (
            json!("[nested value omitted from conversation history]"),
            true,
        );
    }
    match value {
        Value::Object(input) => {
            let mut output = Map::new();
            let mut truncated = input.len() > DurableTraceProjectionLimits::GENERIC_OBJECT_FIELDS;
            for (key, value) in input
                .iter()
                .take(DurableTraceProjectionLimits::GENERIC_OBJECT_FIELDS)
            {
                if is_hidden_reasoning_key(key) {
                    truncated = true;
                    continue;
                }
                let canonical_key = canonical_key(key);
                if is_binary_key(&canonical_key) {
                    output.insert(key.clone(), json!(BINARY_OMITTED_MARKER));
                    truncated = true;
                    continue;
                }
                if canonical_key == BINARY_OMITTED_FROM_HISTORY_KEY && value.as_bool() == Some(true)
                {
                    output.insert(key.clone(), value.clone());
                    truncated = true;
                    continue;
                }
                let (value, value_truncated) = bound_value(value, string_limit, depth + 1);
                output.insert(key.clone(), value);
                truncated |= value_truncated;
            }
            (Value::Object(output), truncated)
        }
        Value::Array(input) => {
            let mut truncated = input.len() > DurableTraceProjectionLimits::GENERIC_ARRAY_ITEMS;
            let output = input
                .iter()
                .take(DurableTraceProjectionLimits::GENERIC_ARRAY_ITEMS)
                .map(|value| {
                    let (value, value_truncated) = bound_value(value, string_limit, depth + 1);
                    truncated |= value_truncated;
                    value
                })
                .collect();
            (Value::Array(output), truncated)
        }
        Value::String(value) => {
            let (value, truncated) = sanitize_and_bound_text(value, string_limit, false);
            (Value::String(value), truncated)
        }
        Value::Null | Value::Bool(_) | Value::Number(_) => (value.clone(), false),
    }
}

fn sanitize_runtime_value_inner(value: &Value) -> (Value, bool) {
    match value {
        Value::Object(input) => {
            let mut output = Map::new();
            let mut redacted = false;
            for (key, value) in input {
                let canonical_key = canonical_key(key);
                if is_runtime_binary_key(&canonical_key) {
                    output.insert(key.clone(), json!(BINARY_OMITTED_MARKER));
                    redacted = true;
                } else if canonical_key == BINARY_OMITTED_FROM_HISTORY_KEY
                    && value.as_bool() == Some(true)
                {
                    output.insert(key.clone(), value.clone());
                    redacted = true;
                } else {
                    let (value, item_redacted) = sanitize_runtime_value_inner(value);
                    output.insert(key.clone(), value);
                    redacted |= item_redacted;
                }
            }
            (Value::Object(output), redacted)
        }
        Value::Array(input) => {
            let mut redacted = false;
            let output = input
                .iter()
                .map(|value| {
                    let (value, item_redacted) = sanitize_runtime_value_inner(value);
                    redacted |= item_redacted;
                    value
                })
                .collect();
            (Value::Array(output), redacted)
        }
        Value::String(value) => {
            let (value, redacted) = sanitize_runtime_text(value);
            (Value::String(value), redacted)
        }
        Value::Null | Value::Bool(_) | Value::Number(_) => (value.clone(), false),
    }
}

fn sanitize_and_bound_text(value: &str, max_chars: usize, tail: bool) -> (String, bool) {
    let (value, binary_redacted) = redact_binary_text(value);
    let (value, truncated) = truncate_text(&value, max_chars, tail);
    (value, binary_redacted || truncated)
}

fn truncate_text(value: &str, max_chars: usize, tail: bool) -> (String, bool) {
    let char_count = value.chars().count();
    if char_count <= max_chars {
        return (value.to_string(), false);
    }
    if tail {
        let tail = value
            .chars()
            .skip(char_count.saturating_sub(max_chars))
            .collect::<String>();
        return (format!("{EARLIER_OUTPUT_OMITTED_PREFIX}{tail}"), true);
    }
    let prefix = value.chars().take(max_chars).collect::<String>();
    (format!("{prefix}{TRUNCATED_SUFFIX}"), true)
}

fn redact_binary_text(value: &str) -> (String, bool) {
    if value == BINARY_OMITTED_MARKER {
        return (value.to_string(), false);
    }
    let mut output = String::with_capacity(value.len().min(4_096));
    let mut cursor = 0;
    let lower = value.to_ascii_lowercase();
    let mut redacted = false;
    while let Some(relative_start) = lower[cursor..].find("data:") {
        let start = cursor + relative_start;
        let mut header_end = (start + 512).min(lower.len());
        while header_end > start && !lower.is_char_boundary(header_end) {
            header_end -= 1;
        }
        let Some(relative_marker) = lower[start..header_end].find(";base64,") else {
            output.push_str(&value[cursor..start + "data:".len()]);
            cursor = start + "data:".len();
            continue;
        };
        let payload_start = start + relative_marker + ";base64,".len();
        let mut payload_end = payload_start;
        for byte in value.as_bytes()[payload_start..].iter().copied() {
            if byte.is_ascii_alphanumeric() || matches!(byte, b'+' | b'/' | b'=' | b'-' | b'_') {
                payload_end += 1;
            } else {
                break;
            }
        }
        output.push_str(&value[cursor..start]);
        output.push_str(BINARY_OMITTED_MARKER);
        cursor = payload_end.max(payload_start);
        redacted = true;
    }
    output.push_str(&value[cursor..]);

    let trimmed = output.trim();
    if !redacted && looks_like_standalone_base64(trimmed) {
        return (BINARY_OMITTED_MARKER.to_string(), true);
    }
    (output, redacted)
}

fn looks_like_standalone_base64(value: &str) -> bool {
    if value.len() < 32 || !value.len().is_multiple_of(4) {
        return false;
    }
    let valid = value
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'+' | b'/' | b'='));
    valid && (value.contains(['+', '/', '=']) || value.len() >= 256)
}

fn is_binary_key(canonical: &str) -> bool {
    canonical.contains("base64")
        || canonical.contains("dataurl")
        || canonical.contains("binarybytes")
        || canonical.contains("imagebytes")
}

fn is_runtime_binary_key(canonical: &str) -> bool {
    canonical.contains("base64") || canonical.contains("dataurl")
}

fn is_hidden_reasoning_key(key: &str) -> bool {
    matches!(
        canonical_key(key).as_str(),
        "reasoning" | "hiddenthinking" | "chainofthought" | "internalreasoning"
    )
}

fn canonical_key(key: &str) -> String {
    key.chars()
        .filter(|character| character.is_ascii_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

fn line_count(value: &str) -> u64 {
    if value.is_empty() {
        0
    } else {
        value.lines().count() as u64
    }
}

fn structured_edit_stats(edits: &[Value]) -> (u64, u64) {
    edits.iter().fold((0, 0), |(additions, deletions), edit| {
        let kind = edit.get("kind").and_then(Value::as_str).unwrap_or_default();
        let inserted = match kind {
            "replace" => edit.get("newText"),
            _ => edit.get("text"),
        }
        .and_then(Value::as_str)
        .map(line_count)
        .unwrap_or(0);
        let deleted = (kind == "replace")
            .then(|| edit.get("oldText").and_then(Value::as_str).map(line_count))
            .flatten()
            .unwrap_or(0);
        (
            additions.saturating_add(inserted),
            deletions.saturating_add(deleted),
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn human_display(answers: Vec<Value>) -> Value {
        json!({"type":"human_interaction_response", "schemaVersion":1,
            "requestId":"r".repeat(256), "responseId":"s".repeat(256), "answers":answers})
    }

    #[test]
    fn human_interaction_history_preserves_whole_large_batch_and_bound_response_identity() {
        let mut answers = (0..40)
            .map(|index| {
                json!({
                    "questionId":format!("question-{index}"), "question":"问题".repeat(600),
                    "kind":"text", "answer":"答案".repeat(700),
                })
            })
            .collect::<Vec<_>>();
        answers.push(json!({"questionId":"option", "question":"Option?", "kind":"option", "optionId":"choice", "answer":"Choice label"}));
        answers.push(
            json!({"questionId":"skip", "question":"Skip?", "kind":"skipped", "answer":"已跳过"}),
        );
        let display = human_display(answers);
        let (projected, _, _) = project_tool_result("request_user_input", None, &display, None);
        assert_eq!(projected.value, display);
        assert!(!projected.truncated);
        assert_eq!(projected.value["answers"].as_array().unwrap().len(), 42);
        assert_eq!(projected.value["requestId"].as_str().unwrap().len(), 256);
        let (again, _, _) = project_tool_result("request_user_input", None, &projected.value, None);
        assert_eq!(again.value, display);
        assert!(!again.truncated);
        let (other_tool, _, _) = project_tool_result("unknown_tool", None, &display, None);
        assert!(other_tool.truncated);
        assert_eq!(
            other_tool.value["answers"].as_array().unwrap().len(),
            DurableTraceProjectionLimits::GENERIC_ARRAY_ITEMS
        );
    }

    #[test]
    fn human_interaction_history_invalid_shapes_and_byte_overflow_use_generic_projection() {
        let valid = human_display(vec![
            json!({"questionId":"q", "question":"Question", "kind":"text", "answer":"answer"}),
        ]);
        let mut invalid = Vec::new();
        let mut extra = valid.clone();
        extra["answers"][0]["optionId"] = json!("unexpected");
        invalid.push(extra);
        let mut duplicate = valid.clone();
        duplicate["answers"] = json!([valid["answers"][0].clone(), valid["answers"][0].clone()]);
        invalid.push(duplicate);
        let mut skipped = valid.clone();
        skipped["answers"][0]["kind"] = json!("skipped");
        invalid.push(skipped);
        let mut id = valid.clone();
        id["requestId"] = json!("x".repeat(257));
        invalid.push(id);
        let mut title = valid.clone();
        title["answers"][0]["question"] = json!("q".repeat(8193));
        invalid.push(title);
        let mut answer = valid.clone();
        answer["answers"][0]["answer"] = json!("a".repeat(32769));
        invalid.push(answer);
        invalid.push(human_display((0..9).map(|i| json!({"questionId":format!("q-{i}"), "question":"Q", "kind":"text", "answer":"a".repeat(32700)})).collect()));
        invalid.push(human_display((0..40).map(|i| json!({"questionId":format!("q-{i}"), "question":"q".repeat(7000), "kind":"skipped", "answer":"已跳过"})).collect()));
        invalid.push(human_display((0..300).map(|i| json!({"questionId":format!("q-{i}"), "question":"q".repeat(4000), "kind":"skipped", "answer":"已跳过"})).collect()));
        for value in invalid {
            assert!(
                !crate::human_interaction::HumanInteractionResponseDisplay::valid_value(&value)
            );
            assert_eq!(
                project_human_interaction_response(&value),
                project_generic_value(&value)
            );
        }
    }

    #[test]
    fn human_interaction_runtime_canonical_projection_keeps_submitted_text_exact() {
        let display = human_display(vec![
            json!({"questionId":"q", "question":"A textual data URL?", "kind":"text", "answer":"data:text/plain;base64,SGVsbG8="}),
        ]);
        let result = crate::AgentToolResult {
            call_id: "call".into(),
            tool: "request_user_input".into(),
            ok: true,
            result: Some(display.clone()),
            error: None,
            exact_archive_file: None,
        };
        assert_eq!(
            crate::conversation_trace::canonical_tool_result_for_context(&result).result,
            Some(display.clone())
        );
        assert_eq!(
            sanitize_runtime_tool_result("request_user_input", &display),
            (display.clone(), false)
        );
        assert!(sanitize_runtime_tool_result("mcp_tool", &display).1);
    }

    #[test]
    fn apply_patch_call_projection_keeps_only_current_direct_metadata() {
        let (projected, _) = project_apply_patch_call(&json!({
            "request": {
                "validatedRequest": true,
                "action": "apply",
                "operation": "update",
                "filePath": "src/lib.rs",
                "editCount": 2
            }
        }));

        let request = &projected["request"];
        assert_eq!(request["action"], "apply");
        assert_eq!(request["operation"], "update");
        assert_eq!(request["filePath"], "src/lib.rs");
        assert_eq!(request["changeRepresentation"], "edits");
        assert_eq!(request["editCount"], 2);
        assert!(request.get("observationId").is_none());
    }

    #[test]
    fn apply_patch_staged_call_projection_omits_body_and_keeps_bounded_identity() {
        let private_content = "私密正文\nsecond line\n";
        let (projected, truncated) = project_apply_patch_call(&json!({
            "request": {
                "action": "append",
                "transactionId": "file-change-1",
                "index": 4,
                "expectedDraftRevision": 4,
                "content": private_content,
            }
        }));

        assert!(truncated);
        let request = &projected["request"];
        assert_eq!(request["action"], "append");
        assert_eq!(request["transactionId"], "file-change-1");
        assert_eq!(request["index"], 4);
        assert_eq!(request["expectedDraftRevision"], 4);
        assert_eq!(request["contentBytes"], private_content.len());
        assert_eq!(
            request["contentDigest"],
            crate::file_change::content_digest(private_content.as_bytes())
        );
        assert!(request.get("content").is_none());
        assert!(!serde_json::to_string(&projected)
            .unwrap()
            .contains(private_content));
    }

    #[test]
    fn apply_patch_trace_projection_is_body_free_and_idempotent() {
        let cases = [
            json!({
                "request": {
                    "action": "apply",
                    "operation": "create",
                    "filePath": "created.txt",
                    "content": "DIRECT_TRACE_CANARY\nsecond line\n",
                    "summary": "create fixture"
                }
            }),
            json!({
                "request": {
                    "action": "apply",
                    "operation": "update",
                    "filePath": "updated.txt",
                    "observationId": "fobs_private",
                    "edits": [{
                        "kind": "replace",
                        "oldText": "OLD_TRACE_CANARY",
                        "newText": "NEW_TRACE_CANARY"
                    }]
                }
            }),
            json!({
                "request": {
                    "action": "commit",
                    "transactionId": "file-change-staged-v1:fixture",
                    "expectedDraftRevision": 3,
                    "summary": "commit fixture"
                }
            }),
        ];

        for raw in cases {
            let first = project_tool_call("apply_patch", &raw);
            let second = project_tool_call("apply_patch", &first.value);
            assert_eq!(second.value, first.value);
            assert_eq!(second.truncated, first.truncated);
            assert_eq!(first.value["request"]["validatedRequest"], true);
            let serialized = first.value.to_string();
            assert!(!serialized.contains("fobs_private"));
            assert!(!serialized.contains("DIRECT_TRACE_CANARY"));
            assert!(!serialized.contains("OLD_TRACE_CANARY"));
            assert!(!serialized.contains("NEW_TRACE_CANARY"));
        }
    }

    #[test]
    fn malformed_apply_patch_calls_cannot_leak_bodies_into_durable_trace() {
        for malformed in [
            json!({
                "action": "apply",
                "content": "PRIVATE_FLAT_TRACE_CANARY"
            }),
            json!({
                "request": {
                    "action": "apply",
                    "operation": "delete",
                    "filePath": "PRIVATE_PATH_TRACE_CANARY",
                    "body": "PRIVATE_NESTED_TRACE_CANARY"
                }
            }),
        ] {
            let projected = project_tool_call("apply_patch", &malformed);
            let serialized = projected.value.to_string();
            assert!(!serialized.contains("PRIVATE_FLAT_TRACE_CANARY"));
            assert!(!serialized.contains("PRIVATE_NESTED_TRACE_CANARY"));
            assert!(!serialized.contains("PRIVATE_PATH_TRACE_CANARY"));
        }
    }

    #[test]
    fn embedded_data_urls_and_binary_named_fields_are_redacted_recursively() {
        let input = json!({
            "note": "before data:image/png;base64,U0VDUkVU after",
            "nested": {
                "arbitraryDataUrlPayload": "not-even-a-data-url",
                "safe": "keep"
            }
        });
        let (projected, truncated) = project_generic_value(&input);
        assert!(truncated);
        assert_eq!(projected["note"], "before [binary/base64 omitted] after");
        assert_eq!(
            projected["nested"]["arbitraryDataUrlPayload"],
            BINARY_OMITTED_MARKER
        );
        assert_eq!(projected["nested"]["safe"], "keep");
        let serialized = serde_json::to_string(&projected).unwrap();
        assert!(!serialized.contains("U0VDUkVU"));
    }

    #[test]
    fn command_output_uses_a_bounded_tail() {
        let result = json!({
            "status": "exited",
            "command": "cargo test",
            "cwd": ".",
            "exitCode": 1,
            "stdout": format!("old-marker{}", "x".repeat(DurableTraceProjectionLimits::COMMAND_OUTPUT_TAIL_CHARS + 50)),
            "stderr": "important failure at the end",
            "stdoutTruncated": false,
            "stderrTruncated": false
        });
        let (projected, truncated) = project_run_command_result(&result);
        assert!(truncated);
        assert_eq!(projected["status"], "exited");
        assert!(!projected["stdoutTail"]
            .as_str()
            .unwrap()
            .contains("old-marker"));
        assert!(projected["stdoutTail"]
            .as_str()
            .unwrap()
            .starts_with(EARLIER_OUTPUT_OMITTED_PREFIX));
        assert_eq!(projected["stderrTail"], "important failure at the end");
    }

    #[test]
    fn running_command_receipt_keeps_session_continuity_with_bounded_output() {
        let result = json!({
            "status": "running",
            "sessionId": "cmd_0123456789abcdef0123456789abcdef",
            "output": format!(
                "old-marker{}important-tail",
                "x".repeat(DurableTraceProjectionLimits::COMMAND_OUTPUT_TAIL_CHARS + 50)
            ),
            "startedAt": 1_725_000_000_000_i64,
            "latestSequence": 42,
            "outputTruncated": false,
            "hostPrivateField": "must not survive",
        });

        let (projected, truncated) = project_run_command_result(&result);

        assert!(truncated);
        assert_eq!(projected["status"], "running");
        assert_eq!(
            projected["sessionId"],
            "cmd_0123456789abcdef0123456789abcdef"
        );
        assert_eq!(projected["startedAt"], 1_725_000_000_000_i64);
        assert_eq!(projected["latestSequence"], 42);
        assert_eq!(projected["outputTruncated"], true);
        assert!(!projected["output"].as_str().unwrap().contains("old-marker"));
        assert!(projected["output"]
            .as_str()
            .unwrap()
            .starts_with(EARLIER_OUTPUT_OMITTED_PREFIX));
        assert!(projected["output"]
            .as_str()
            .unwrap()
            .ends_with("important-tail"));
        assert!(projected.get("hostPrivateField").is_none());
    }

    #[test]
    fn command_session_projection_keeps_only_the_opaque_terminal_recovery_route() {
        let open = format!("hist_v1_{}", "a".repeat(64));
        let result = json!({
            "sessionId": "cmd_0123456789abcdef0123456789abcdef",
            "status": "exited",
            "output": "bounded transcript body",
            "latestSequence": 42,
            "outputTruncated": true,
            "historyOpen": open,
            "continueWith": {
                "tool": "conversation_history",
                "args": { "open": open }
            },
            "read": {
                "requestedAfterSequence": 0,
                "firstSequence": 1,
                "throughSequence": 42,
                "truncatedBefore": false,
                "outputBytes": 23,
                "outputHash": "hash"
            }
        });

        let (projected, truncated) = project_command_session_result(&result);

        assert!(truncated, "the transcript body is intentionally omitted");
        assert!(projected.get("output").is_none());
        assert_eq!(projected["historyOpen"], open);
        assert_eq!(projected["continueWith"]["tool"], "conversation_history");
        assert_eq!(projected["continueWith"]["args"]["open"], open);
    }

    #[test]
    fn running_command_receipt_preserves_source_capture_truncation() {
        let result = json!({
            "status": "running",
            "sessionId": "cmd_0123456789abcdef0123456789abcdef",
            "output": "initial output",
            "startedAt": 1_725_000_000_000_i64,
            "latestSequence": 7,
            "outputTruncated": true,
        });

        let (projected, truncated) = project_run_command_result(&result);

        assert!(!truncated);
        assert_eq!(projected["output"], "initial output");
        assert_eq!(projected["outputTruncated"], true);
    }

    #[test]
    fn web_fetch_trace_keeps_completeness_metadata_while_bounding_the_body() {
        let result = json!({
            "url": "https://example.com/large",
            "content": "正文".repeat(10_000),
            "contentCoverage": {
                "unit": "bytes",
                "total": 60_000,
                "returned": 60_000,
                "omitted": 0
            },
            "imagesCoverage": {
                "unit": "items",
                "total": 42,
                "returned": 30,
                "omitted": 12
            },
            "failedResultsCoverage": {
                "unit": "items",
                "total": 0,
                "returned": 0,
                "omitted": 0
            },
            "truncated": true,
            "truncatedAtSource": false
        });

        let (projected, trace_projection_truncated) = project_web_fetch_result(&result);

        assert!(trace_projection_truncated);
        assert!(
            projected["summary"].as_str().unwrap().chars().count()
                <= DurableTraceProjectionLimits::SUMMARY_CHARS + 100
        );
        assert_eq!(
            projected["contentCoverage"], result["contentCoverage"],
            "durable audit must retain exact capture coverage"
        );
        assert_eq!(projected["imagesCoverage"], result["imagesCoverage"]);
        assert_eq!(
            projected["failedResultsCoverage"],
            result["failedResultsCoverage"]
        );
        assert_eq!(projected["truncatedAtSource"], false);
    }

    #[test]
    fn read_directory_failure_keeps_its_classification_and_path_in_durable_trace() {
        let result = json!({
            "code": "path_is_directory",
            "errorCode": "read_file.path_is_directory",
            "path": "crates/mcp-client/src",
            "message": "read_file 只能读取普通文本文件。",
            "continueWith": {
                "tool": "workspace_map",
                "args": {
                    "focusPath": "crates/mcp-client/src",
                    "maxDepth": 3
                }
            }
        });

        let (projected, truncated) = project_read_result(&result);

        assert!(!truncated);
        assert_eq!(projected["code"], "path_is_directory");
        assert_eq!(projected["errorCode"], "read_file.path_is_directory");
        assert_eq!(projected["path"], "crates/mcp-client/src");
        assert_eq!(projected["message"], "read_file 只能读取普通文本文件。");
        assert!(projected.get("continueWith").is_none());
    }

    #[test]
    fn skill_resource_body_is_checkpoint_only_and_never_enters_durable_projection() {
        let result = json!({
            "uri": "skill://package/example/revision/references/guide.md",
            "startByte": 0,
            "endByteExclusive": 26,
            "totalBytes": 26,
            "returnedBytes": 26,
            "content": "private run-scoped content"
        });

        let (projected, error, error_truncated) =
            project_tool_result("skills_read_resource", None, &result, None);
        assert!(projected.truncated);
        assert!(!error_truncated);
        assert!(error.is_none());
        assert_eq!(
            projected.value["uri"],
            "skill://package/example/revision/references/guide.md"
        );
        assert_eq!(projected.value["contentOmittedFromHistory"], true);
        assert!(projected.value.get("content").is_none());
        assert!(!serde_json::to_string(&projected.value)
            .unwrap()
            .contains("private run-scoped content"));
    }
}
