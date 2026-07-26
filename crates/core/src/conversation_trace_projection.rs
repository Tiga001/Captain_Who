//! Central policy for projecting run-scoped tool data into durable conversation history.
//!
//! Runtime observations, approval checkpoints, and presentation events deliberately do not use
//! these limits. A tool executes once; this module only derives the bounded, provider-neutral view
//! that later conversation turns may receive.

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
    pub(crate) const FILE_PATHS: usize = 32;
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
        "write_file" => project_write_file_call(operation),
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
        "web_fetch" => project_web_fetch_result(observation),
        "web_search" => project_web_search_result(observation),
        "write_file" => project_write_file_result(observation),
        "apply_patch" => project_apply_patch_result(operation, observation),
        "run_command" => project_run_command_result(observation),
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

fn project_write_file_call(value: &Value) -> (Value, bool) {
    let Some(input) = value.as_object() else {
        return project_generic_value(value);
    };
    let mut output = Map::new();
    let mut truncated = false;
    for (key, limit) in [
        ("phase", DurableTraceProjectionLimits::GENERIC_STRING_CHARS),
        (
            "draftId",
            DurableTraceProjectionLimits::GENERIC_STRING_CHARS,
        ),
        ("filePath", DurableTraceProjectionLimits::PATH_CHARS),
        ("mode", DurableTraceProjectionLimits::GENERIC_STRING_CHARS),
        ("index", DurableTraceProjectionLimits::GENERIC_STRING_CHARS),
        ("summary", DurableTraceProjectionLimits::SUMMARY_CHARS),
    ] {
        copy_bounded_field(input, &mut output, key, limit, &mut truncated);
    }
    if input
        .get("content")
        .and_then(Value::as_str)
        .is_some_and(|content| content == "[write_file chunk omitted from conversation history]")
    {
        if let Some(content_bytes) = input.get("contentBytes") {
            output.insert("contentBytes".into(), content_bytes.clone());
        }
        if let Some(content_lines) = input.get("contentLines") {
            output.insert("contentLines".into(), content_lines.clone());
        }
        output.insert(
            "content".into(),
            json!("[write_file chunk omitted from conversation history]"),
        );
        truncated = true;
    } else if let Some(content) = input.get("content").and_then(Value::as_str) {
        output.insert("contentBytes".into(), json!(content.len()));
        output.insert("contentLines".into(), json!(line_count(content)));
        output.insert(
            "content".into(),
            json!("[write_file chunk omitted from conversation history]"),
        );
        truncated = true;
    } else if let Some(content_bytes) = input.get("contentBytes") {
        output.insert("contentBytes".into(), content_bytes.clone());
        output.insert(
            "content".into(),
            json!("[write_file chunk omitted from conversation history]"),
        );
        truncated = true;
    }
    if let Some(edits) = input.get("edits").and_then(Value::as_array) {
        output.insert("editCount".into(), json!(edits.len()));
        let (additions, deletions) = structured_edit_stats(edits);
        output.insert("additions".into(), json!(additions));
        output.insert("deletions".into(), json!(deletions));
        truncated = true;
    }
    (Value::Object(output), truncated)
}

fn project_apply_patch_call(value: &Value) -> (Value, bool) {
    let Some(input) = value.as_object() else {
        return project_generic_value(value);
    };
    let mut output = Map::new();
    let mut truncated = false;
    for (key, limit) in [
        (
            "operation",
            DurableTraceProjectionLimits::GENERIC_STRING_CHARS,
        ),
        ("filePath", DurableTraceProjectionLimits::PATH_CHARS),
        (
            "baseRevision",
            DurableTraceProjectionLimits::GENERIC_STRING_CHARS,
        ),
        (
            "expectedRevision",
            DurableTraceProjectionLimits::GENERIC_STRING_CHARS,
        ),
        ("summary", DurableTraceProjectionLimits::SUMMARY_CHARS),
    ] {
        copy_bounded_field(input, &mut output, key, limit, &mut truncated);
    }

    let (representation, additions, deletions) = if let Some(patch) =
        input.get("patch").and_then(Value::as_str)
    {
        let (additions, deletions) = unified_diff_stats(patch);
        ("patch", additions, deletions)
    } else if let Some(content) = input.get("content").and_then(Value::as_str) {
        ("content", line_count(content), 0)
    } else if let Some(edits) = input.get("edits").and_then(Value::as_array) {
        let (additions, deletions) = structured_edit_stats(edits);
        output.insert("editCount".into(), json!(edits.len()));
        ("edits", additions, deletions)
    } else if let Some(representation) = input.get("changeRepresentation").and_then(Value::as_str) {
        (
            representation,
            input.get("additions").and_then(Value::as_u64).unwrap_or(0),
            input.get("deletions").and_then(Value::as_u64).unwrap_or(0),
        )
    } else {
        ("metadata_only", 0, 0)
    };
    output.insert("changeRepresentation".into(), json!(representation));
    output.insert("additions".into(), json!(additions));
    output.insert("deletions".into(), json!(deletions));
    truncated |= representation != "metadata_only";
    (Value::Object(output), truncated)
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
    ] {
        copy_bounded_field(input, &mut output, key, limit, &mut truncated);
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
        output.insert("results".into(), Value::Array(selected));
    }
    let source_truncated = input
        .get("truncated")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    output.insert("truncated".into(), json!(source_truncated || truncated));
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

fn project_write_file_result(value: &Value) -> (Value, bool) {
    let source = value
        .get("draft")
        .and_then(Value::as_object)
        .or_else(|| value.as_object());
    let Some(input) = source else {
        return project_generic_value(value);
    };
    project_selected_map(
        input,
        &[
            ("status", DurableTraceProjectionLimits::TITLE_CHARS),
            (
                "draftId",
                DurableTraceProjectionLimits::GENERIC_STRING_CHARS,
            ),
            ("mode", DurableTraceProjectionLimits::TITLE_CHARS),
            ("filePath", DurableTraceProjectionLimits::PATH_CHARS),
            (
                "additions",
                DurableTraceProjectionLimits::GENERIC_STRING_CHARS,
            ),
            (
                "deletions",
                DurableTraceProjectionLimits::GENERIC_STRING_CHARS,
            ),
            (
                "lineCount",
                DurableTraceProjectionLimits::GENERIC_STRING_CHARS,
            ),
            (
                "byteCount",
                DurableTraceProjectionLimits::GENERIC_STRING_CHARS,
            ),
            (
                "revision",
                DurableTraceProjectionLimits::GENERIC_STRING_CHARS,
            ),
            ("error", DurableTraceProjectionLimits::TOOL_ERROR_CHARS),
            ("message", DurableTraceProjectionLimits::SUMMARY_CHARS),
        ],
    )
}

fn project_apply_patch_result(operation: Option<&Value>, value: &Value) -> (Value, bool) {
    let Some(input) = value.as_object() else {
        return project_generic_value(value);
    };
    let mut output = Map::new();
    let mut truncated = false;
    for (key, limit) in [
        ("status", DurableTraceProjectionLimits::TITLE_CHARS),
        (
            "operation",
            DurableTraceProjectionLimits::GENERIC_STRING_CHARS,
        ),
        ("filePath", DurableTraceProjectionLimits::PATH_CHARS),
        ("error", DurableTraceProjectionLimits::TOOL_ERROR_CHARS),
        ("message", DurableTraceProjectionLimits::SUMMARY_CHARS),
        (
            "gitDiffError",
            DurableTraceProjectionLimits::TOOL_ERROR_CHARS,
        ),
    ] {
        copy_bounded_field(input, &mut output, key, limit, &mut truncated);
    }
    if let Some(paths) = input.get("appliedFilePaths").and_then(Value::as_array) {
        let (paths, paths_truncated) = bounded_string_array(
            paths,
            DurableTraceProjectionLimits::FILE_PATHS,
            DurableTraceProjectionLimits::PATH_CHARS,
        );
        output.insert("appliedFilePaths".into(), Value::Array(paths));
        truncated |= paths_truncated;
    }
    let patch = input
        .get("gitDiff")
        .and_then(|value| value.get("patch"))
        .and_then(Value::as_str)
        .or_else(|| {
            operation
                .and_then(|value| value.get("patch"))
                .and_then(Value::as_str)
        });
    let (additions, deletions) = patch.map(unified_diff_stats).unwrap_or_else(|| {
        let operation_stats = operation.map(operation_stats).unwrap_or((0, 0));
        if operation_stats == (0, 0) {
            (
                input.get("additions").and_then(Value::as_u64).unwrap_or(0),
                input.get("deletions").and_then(Value::as_u64).unwrap_or(0),
            )
        } else {
            operation_stats
        }
    });
    output.insert("additions".into(), json!(additions));
    output.insert("deletions".into(), json!(deletions));
    if input.get("gitDiff").is_some()
        || input
            .get("patchOmittedFromHistory")
            .and_then(Value::as_bool)
            .unwrap_or(false)
    {
        output.insert("patchOmittedFromHistory".into(), json!(true));
        truncated = true;
    }
    (Value::Object(output), truncated)
}

fn project_run_command_result(value: &Value) -> (Value, bool) {
    let Some(input) = value.as_object() else {
        return project_generic_value(value);
    };
    let mut output = Map::new();
    let mut truncated = false;
    for (key, limit) in [
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

fn unified_diff_stats(patch: &str) -> (u64, u64) {
    patch.lines().fold((0, 0), |(additions, deletions), line| {
        if line.starts_with("+++") || line.starts_with("---") {
            (additions, deletions)
        } else if line.starts_with('+') {
            (additions.saturating_add(1), deletions)
        } else if line.starts_with('-') {
            (additions, deletions.saturating_add(1))
        } else {
            (additions, deletions)
        }
    })
}

fn operation_stats(operation: &Value) -> (u64, u64) {
    if let Some(patch) = operation.get("patch").and_then(Value::as_str) {
        unified_diff_stats(patch)
    } else if let Some(content) = operation.get("content").and_then(Value::as_str) {
        (line_count(content), 0)
    } else if let Some(edits) = operation.get("edits").and_then(Value::as_array) {
        structured_edit_stats(edits)
    } else {
        (0, 0)
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

fn bounded_string_array(input: &[Value], max_items: usize, max_chars: usize) -> (Vec<Value>, bool) {
    let mut truncated = input.len() > max_items;
    let output = input
        .iter()
        .take(max_items)
        .filter_map(Value::as_str)
        .map(|value| {
            let (value, item_truncated) = sanitize_and_bound_text(value, max_chars, false);
            truncated |= item_truncated;
            json!(value)
        })
        .collect();
    (output, truncated)
}

#[cfg(test)]
mod tests {
    use super::*;

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
