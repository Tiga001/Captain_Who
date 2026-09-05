use crate::AgentMcpServerScope;
use std::collections::HashSet;

mod activities;
mod artifacts;
mod skills_office;

use activities::{
    current_collaboration_timeline_activity_is_safe, current_interruption_is_safe,
    current_mcp_invocation_is_safe, current_message_stream_checkpoints_are_safe,
    current_read_activity_is_safe, current_timeline_item_is_safe, current_web_activity_is_safe,
};
use artifacts::{current_command_sessions_are_safe, current_file_change_snapshot_is_safe};
use skills_office::{
    current_activated_skill_is_safe, current_office_operation_is_safe,
    current_skill_installation_is_safe, current_skill_materialization_is_safe,
    current_skill_script_is_safe, current_skill_selection_is_safe, current_todo_is_safe,
};

const CURRENT_AGENT_RUN_KEYS: &[&str] = &[
    "runId",
    "status",
    "startedAt",
    "firstResponseAt",
    "lastResponseAt",
    "completedAt",
    "toolDefinitions",
    "toolSetRevision",
    "todo",
    "toolCalls",
    "toolResults",
    "webSearchActivities",
    "readActivities",
    "approvals",
    "skillInstallations",
    "fileChangeProposals",
    "fileChanges",
    "commandSessions",
    "mcpInvocations",
    "collaborationTimelineActivities",
    "messageStreamCheckpoints",
    "timeline",
    "state",
    "interruption",
    "error",
    "usage",
    "finishReason",
    "activatedSkills",
    "skillActivationRevision",
    "explicitSkillSelections",
];

fn exact_keys(object: &serde_json::Map<String, serde_json::Value>, allowed: &[&str]) -> bool {
    object.len() == allowed.len() && object.keys().all(|key| allowed.contains(&key.as_str()))
}

fn only_allowed_keys(
    object: &serde_json::Map<String, serde_json::Value>,
    allowed: &[&str],
) -> bool {
    object.keys().all(|key| allowed.contains(&key.as_str()))
}

const MAX_CURRENT_RUN_ITEMS: usize = 10_000;
pub(in crate::storage::chat_repository) const MAX_JS_SAFE_INTEGER: u64 = 9_007_199_254_740_991;

fn exact_required_optional_keys(
    object: &serde_json::Map<String, serde_json::Value>,
    required: &[&str],
    optional: &[&str],
) -> bool {
    required.iter().all(|key| object.contains_key(*key))
        && object
            .keys()
            .all(|key| required.contains(&key.as_str()) || optional.contains(&key.as_str()))
}

fn safe_integer(value: &serde_json::Value) -> bool {
    value
        .as_u64()
        .is_some_and(|value| value <= MAX_JS_SAFE_INTEGER)
}

fn bounded_string(value: &serde_json::Value, maximum: usize, allow_empty: bool) -> bool {
    value
        .as_str()
        .is_some_and(|value| (allow_empty || !value.is_empty()) && value.chars().count() <= maximum)
}

fn optional_bounded_string(
    object: &serde_json::Map<String, serde_json::Value>,
    key: &str,
    maximum: usize,
    allow_empty: bool,
) -> bool {
    object
        .get(key)
        .is_none_or(|value| bounded_string(value, maximum, allow_empty))
}

fn nullable_bounded_string(value: &serde_json::Value, maximum: usize, allow_empty: bool) -> bool {
    value.is_null() || bounded_string(value, maximum, allow_empty)
}

fn optional_safe_integer(object: &serde_json::Map<String, serde_json::Value>, key: &str) -> bool {
    object.get(key).is_none_or(safe_integer)
}

fn current_string_array_is_safe(value: &serde_json::Value, maximum: usize) -> bool {
    value.as_array().is_some_and(|values| {
        values.len() <= MAX_CURRENT_RUN_ITEMS
            && values
                .iter()
                .all(|value| bounded_string(value, maximum, true))
    })
}

fn record_array_is_safe(
    value: &serde_json::Value,
    validate: impl Fn(&serde_json::Map<String, serde_json::Value>) -> bool,
) -> bool {
    value.as_array().is_some_and(|values| {
        values.len() <= MAX_CURRENT_RUN_ITEMS
            && values
                .iter()
                .all(|value| value.as_object().is_some_and(&validate))
    })
}

fn current_tool_definition_is_safe(
    definition: &serde_json::Map<String, serde_json::Value>,
) -> bool {
    exact_required_optional_keys(
        definition,
        &[
            "name",
            "description",
            "inputSchema",
            "safety",
            "requiresWorkspace",
            "requiresApproval",
            "approvalMode",
        ],
        &[],
    ) && bounded_string(&definition["name"], 1_024, false)
        && bounded_string(&definition["description"], 128 * 1_024, true)
        && matches!(
            definition["safety"].as_str(),
            Some("read_only" | "requires_approval" | "destructive")
        )
        && definition["requiresWorkspace"].is_boolean()
        && definition["requiresApproval"].is_boolean()
        && matches!(
            definition["approvalMode"].as_str(),
            Some("never" | "always" | "dynamic")
        )
}

pub(in crate::storage::chat_repository) fn current_uuid_is_safe(value: &str) -> bool {
    let Ok(parsed) = uuid::Uuid::parse_str(value) else {
        return false;
    };
    parsed.hyphenated().to_string().eq_ignore_ascii_case(value)
        && matches!(parsed.get_version_num(), 1..=5)
        && parsed.get_variant() == uuid::Variant::RFC4122
}

fn current_tool_call_is_safe(call: &serde_json::Map<String, serde_json::Value>) -> bool {
    exact_keys(call, &["id", "tool", "args", "approvalStatus", "reason"])
        && bounded_string(&call["id"], 1_024, false)
        && bounded_string(&call["tool"], 1_024, false)
        && matches!(
            call["approvalStatus"].as_str(),
            Some("not_required" | "required" | "approved" | "rejected")
        )
        && (call["reason"].is_null() || bounded_string(&call["reason"], 4_096, true))
        && serde_json::from_value::<crate::AgentToolCall>(serde_json::Value::Object(call.clone()))
            .is_ok()
}

fn current_tool_result_is_safe(result: &serde_json::Map<String, serde_json::Value>) -> bool {
    exact_required_optional_keys(result, &["callId", "tool", "ok"], &["result", "error"])
        && bounded_string(&result["callId"], 1_024, false)
        && bounded_string(&result["tool"], 1_024, false)
        && result["ok"].is_boolean()
        && optional_bounded_string(result, "error", 128 * 1_024, true)
}

fn current_approval_status_is_safe(value: &serde_json::Value) -> bool {
    matches!(
        value.as_str(),
        Some("not_required" | "required" | "approved" | "rejected")
    )
}

fn current_command_observation_is_safe(value: &serde_json::Value) -> bool {
    if value.is_null() {
        return true;
    }
    let Some(observation) = value.as_object() else {
        return false;
    };
    exact_required_optional_keys(
        observation,
        &["kinds"],
        &["expectedOutputs", "additionalRoots"],
    ) && observation["kinds"].as_array().is_some_and(|kinds| {
        kinds.len() <= MAX_CURRENT_RUN_ITEMS && kinds.iter().all(|kind| kind == "office")
    }) && ["expectedOutputs", "additionalRoots"].iter().all(|field| {
        observation.get(*field).is_none_or(|values| {
            values.as_array().is_some_and(|values| {
                values.len() <= MAX_CURRENT_RUN_ITEMS
                    && values
                        .iter()
                        .all(|value| bounded_string(value, 16 * 1_024, true))
            })
        })
    })
}

fn current_command_approval_is_safe(value: &serde_json::Value) -> bool {
    let Some(command) = value.as_object() else {
        return false;
    };
    exact_keys(
        command,
        &[
            "id",
            "command",
            "cwd",
            "timeoutMs",
            "approvalStatus",
            "riskLevel",
            "reason",
            "observe",
        ],
    ) && bounded_string(&command["id"], 1_024, false)
        && bounded_string(&command["command"], 4 * 1_024 * 1_024, true)
        && (command["cwd"].is_null() || bounded_string(&command["cwd"], 16 * 1_024, true))
        && (command["timeoutMs"].is_null() || safe_integer(&command["timeoutMs"]))
        && current_approval_status_is_safe(&command["approvalStatus"])
        && (command["riskLevel"].is_null()
            || matches!(
                command["riskLevel"].as_str(),
                Some("read_only" | "writes_workspace" | "network" | "destructive" | "unknown")
            ))
        && (command["reason"].is_null() || bounded_string(&command["reason"], 16 * 1_024, true))
        && current_command_observation_is_safe(&command["observe"])
}

pub(in crate::storage::chat_repository) fn current_persisted_approval_is_safe(
    approval: &serde_json::Map<String, serde_json::Value>,
) -> bool {
    match approval.get("type").and_then(serde_json::Value::as_str) {
        Some("tool_call") => {
            exact_keys(approval, &["type", "call"])
                && approval["call"]
                    .as_object()
                    .is_some_and(current_tool_call_is_safe)
        }
        Some("file_change") => {
            exact_keys(approval, &["type", "fileChange"])
                && approval["fileChange"]
                    .as_object()
                    .is_some_and(current_file_change_proposal_is_safe)
        }
        Some("command") => {
            exact_keys(approval, &["type", "command"])
                && current_command_approval_is_safe(&approval["command"])
        }
        Some("skill_materialization") => {
            exact_keys(approval, &["type", "materialization"])
                && approval["materialization"]
                    .as_object()
                    .is_some_and(current_skill_materialization_is_safe)
        }
        Some("skill_script") => {
            exact_keys(approval, &["type", "script"])
                && approval["script"]
                    .as_object()
                    .is_some_and(current_skill_script_is_safe)
        }
        Some("mcp_tool_call") => false,
        Some("office_operation") => {
            exact_keys(approval, &["type", "officeOperation"])
                && approval["officeOperation"]
                    .as_object()
                    .is_some_and(current_office_operation_is_safe)
        }
        Some("skill_installation") => {
            if !exact_keys(approval, &["type", "installation"]) {
                return false;
            }
            let synthetic = serde_json::Map::from_iter([
                ("action".to_string(), approval["installation"].clone()),
                (
                    "status".to_string(),
                    serde_json::Value::String("waiting_for_approval".to_string()),
                ),
            ]);
            current_skill_installation_is_safe(&synthetic)
        }
        Some(_) | None => false,
    }
}

fn current_file_change_proposal_is_safe(
    proposal: &serde_json::Map<String, serde_json::Value>,
) -> bool {
    exact_keys(
        proposal,
        &[
            "schemaVersion",
            "id",
            "transactionId",
            "operation",
            "updateStrategy",
            "filePath",
            "inlineDiff",
            "baseRevision",
            "summary",
            "additions",
            "deletions",
            "lineCount",
            "byteCount",
            "approvalStatus",
        ],
    ) && proposal["schemaVersion"] == crate::AGENT_FILE_CHANGE_PROTOCOL_SCHEMA_VERSION
        && bounded_string(&proposal["id"], 1_024, false)
        && bounded_string(&proposal["transactionId"], 1_024, false)
        && current_file_change_operation_fields_are_safe(proposal)
        && bounded_string(&proposal["filePath"], 16 * 1_024, false)
        && current_file_change_inline_diff_is_safe(&proposal["inlineDiff"])
        && nullable_bounded_string(&proposal["baseRevision"], 2_048, false)
        && nullable_bounded_string(&proposal["summary"], 16 * 1_024, true)
        && ["additions", "deletions", "lineCount", "byteCount"]
            .iter()
            .all(|field| safe_integer(&proposal[*field]))
        && (proposal["operation"] != "delete"
            || (proposal["lineCount"] == 0 && proposal["byteCount"] == 0))
        && matches!(
            proposal["approvalStatus"].as_str(),
            Some("required" | "approved")
        )
}

fn current_file_change_inline_diff_is_safe(value: &serde_json::Value) -> bool {
    value.is_null()
        || value.as_object().is_some_and(|diff| {
            exact_keys(diff, &["patch", "truncated"])
                && bounded_string(&diff["patch"], 4 * 1_024 * 1_024, true)
                && diff["truncated"] == false
        })
}

fn current_file_change_operation_fields_are_safe(
    value: &serde_json::Map<String, serde_json::Value>,
) -> bool {
    let operation = value["operation"].as_str();
    let strategy = value["updateStrategy"].as_str();
    let inline_diff_is_null = value["inlineDiff"].is_null();
    let base_revision_is_null = value["baseRevision"].is_null();
    let strategy_is_safe =
        value["updateStrategy"].is_null() || matches!(strategy, Some("modify" | "rewrite"));
    if !strategy_is_safe || !matches!(operation, Some("create" | "update" | "delete")) {
        return false;
    }
    match operation {
        Some("create") => strategy.is_none() && base_revision_is_null,
        Some("update") => !base_revision_is_null && (inline_diff_is_null == strategy.is_some()),
        Some("delete") => strategy.is_none() && !base_revision_is_null && !inline_diff_is_null,
        _ => false,
    }
}

pub(crate) fn current_agent_run_projection_is_safe(
    run: &serde_json::Map<String, serde_json::Value>,
    expected_run_id: &str,
) -> bool {
    current_agent_run_projection_is_safe_with_trace_policy(run, expected_run_id, true)
}

/// Validates the complete current Renderer projection while allowing only Tool identity
/// coherence to be deferred to an authoritative durable Trace. The caller must discard and
/// rebuild every Trace-owned Timeline item before returning the projection.
pub(crate) fn current_agent_run_projection_is_safe_for_trace_rebuild(
    run: &serde_json::Map<String, serde_json::Value>,
    expected_run_id: &str,
) -> bool {
    current_agent_run_projection_is_safe_with_trace_policy(run, expected_run_id, false)
}

fn current_agent_run_projection_is_safe_with_trace_policy(
    run: &serde_json::Map<String, serde_json::Value>,
    expected_run_id: &str,
    require_timeline_tool_identity_coherence: bool,
) -> bool {
    const REQUIRED: &[&str] = &[
        "runId",
        "status",
        "startedAt",
        "toolDefinitions",
        "toolCalls",
        "toolResults",
        "webSearchActivities",
        "readActivities",
        "approvals",
        "fileChangeProposals",
        "fileChanges",
        "mcpInvocations",
        "messageStreamCheckpoints",
        "timeline",
    ];
    if !only_allowed_keys(run, CURRENT_AGENT_RUN_KEYS)
        || !REQUIRED.iter().all(|field| run.contains_key(*field))
        || !run.get("runId").is_some_and(|value| {
            value.is_null()
                || value
                    .as_str()
                    .is_some_and(|run_id| run_id == expected_run_id)
        })
        || !matches!(
            run["status"].as_str(),
            Some(
                "starting"
                    | "idle"
                    | "queued"
                    | "running"
                    | "waiting_for_approval"
                    | "waiting_for_user_input"
                    | "completed"
                    | "failed"
                    | "cancelled"
            )
        )
        || !safe_integer(&run["startedAt"])
        || !run.get("firstResponseAt").is_none_or(safe_integer)
        || !run.get("lastResponseAt").is_none_or(safe_integer)
        || !run.get("completedAt").is_none_or(safe_integer)
    {
        return false;
    }
    let terminal = matches!(
        run["status"].as_str(),
        Some("completed" | "failed" | "cancelled")
    );
    if terminal != run.contains_key("completedAt")
        || !record_array_is_safe(&run["toolDefinitions"], current_tool_definition_is_safe)
        || !record_array_is_safe(&run["toolCalls"], current_tool_call_is_safe)
        || !record_array_is_safe(&run["toolResults"], current_tool_result_is_safe)
        || !record_array_is_safe(&run["webSearchActivities"], current_web_activity_is_safe)
        || !record_array_is_safe(&run["readActivities"], current_read_activity_is_safe)
        || !record_array_is_safe(&run["approvals"], current_persisted_approval_is_safe)
        || !record_array_is_safe(
            &run["fileChangeProposals"],
            current_file_change_proposal_is_safe,
        )
        || !record_array_is_safe(&run["fileChanges"], current_file_change_snapshot_is_safe)
        || !run["mcpInvocations"].as_array().is_some_and(|invocations| {
            invocations.len() <= MAX_CURRENT_RUN_ITEMS
                && invocations.iter().all(current_mcp_invocation_is_safe)
        })
        || !record_array_is_safe(&run["timeline"], current_timeline_item_is_safe)
        || !current_message_stream_checkpoints_are_safe(&run["messageStreamCheckpoints"])
    {
        return false;
    }

    if run
        .get("todo")
        .is_some_and(|todo| !current_todo_is_safe(todo))
        || run.get("skillInstallations").is_some_and(|installations| {
            !record_array_is_safe(installations, current_skill_installation_is_safe)
        })
        || run
            .get("collaborationTimelineActivities")
            .is_some_and(|activities| {
                !record_array_is_safe(activities, current_collaboration_timeline_activity_is_safe)
            })
        || run
            .get("activatedSkills")
            .is_some_and(|skills| !record_array_is_safe(skills, current_activated_skill_is_safe))
        || run
            .get("explicitSkillSelections")
            .is_some_and(|selections| {
                !record_array_is_safe(selections, current_skill_selection_is_safe)
            })
    {
        return false;
    }
    if run
        .get("interruption")
        .is_some_and(|interruption| !current_interruption_is_safe(interruption))
    {
        return false;
    }
    if let Some(revision) = run.get("toolSetRevision") {
        let Some(revision) = revision.as_object() else {
            return false;
        };
        if !exact_keys(revision, &["stable", "dynamic", "effective"])
            || !["stable", "dynamic", "effective"]
                .iter()
                .all(|field| bounded_string(&revision[*field], 1_024, false))
        {
            return false;
        }
    }
    if let Some(state) = run.get("state") {
        let Some(state) = state.as_object() else {
            return false;
        };
        if !exact_keys(state, &["status", "activeRunId", "lastError", "updatedAt"])
            || !matches!(
                state["status"].as_str(),
                Some(
                    "idle"
                        | "queued"
                        | "running"
                        | "waiting_for_approval"
                        | "waiting_for_user_input"
                        | "completed"
                        | "failed"
                        | "cancelled"
                )
            )
            || !safe_integer(&state["updatedAt"])
            || !state
                .get("activeRunId")
                .is_some_and(|value| value.is_null() || bounded_string(value, 1_024, false))
            || !state
                .get("lastError")
                .is_some_and(|value| value.is_null() || bounded_string(value, 128 * 1_024, true))
        {
            return false;
        }
    }
    if let Some(usage) = run.get("usage") {
        let Some(usage) = usage.as_object() else {
            return false;
        };
        if !only_allowed_keys(
            usage,
            &[
                "inputTokens",
                "outputTokens",
                "outputThinkingTokens",
                "totalTokens",
                "cachedInputTokens",
                "cacheCreationInputTokens",
                "billableRequestCount",
            ],
        ) || !usage.values().all(safe_integer)
        {
            return false;
        }
    }
    if !optional_bounded_string(run, "error", 128 * 1_024, true)
        || !optional_bounded_string(run, "finishReason", 1_024, true)
        || !optional_bounded_string(run, "skillActivationRevision", 1_024, false)
    {
        return false;
    }

    let tool_call_ids = run["toolCalls"]
        .as_array()
        .expect("validated ToolCall array")
        .iter()
        .map(|call| call["id"].as_str().expect("validated ToolCall id"))
        .collect::<Vec<_>>();
    let tool_result_ids = run["toolResults"]
        .as_array()
        .expect("validated ToolResult array")
        .iter()
        .map(|result| result["callId"].as_str().expect("validated ToolResult id"))
        .collect::<Vec<_>>();
    let run_command_call_ids = run["toolCalls"]
        .as_array()
        .expect("validated ToolCall array")
        .iter()
        .filter(|call| call["tool"] == "run_command")
        .map(|call| call["id"].as_str().expect("validated ToolCall id"))
        .collect::<HashSet<_>>();
    if run
        .get("commandSessions")
        .is_some_and(|sessions| !current_command_sessions_are_safe(sessions, &run_command_call_ids))
    {
        return false;
    }
    let invocations = run["mcpInvocations"]
        .as_array()
        .expect("validated MCP invocation array");
    let mcp_action_ids = invocations
        .iter()
        .map(|invocation| {
            invocation["actionId"]
                .as_str()
                .expect("validated MCP action id")
        })
        .collect::<Vec<_>>();
    let mcp_invocation_ids = invocations
        .iter()
        .map(|invocation| {
            invocation["invocationId"]
                .as_str()
                .expect("validated MCP invocation id")
        })
        .collect::<Vec<_>>();
    let mcp_call_ids = invocations
        .iter()
        .map(|invocation| {
            invocation["callId"]
                .as_str()
                .expect("validated MCP call id")
        })
        .collect::<Vec<_>>();
    let timeline = run["timeline"]
        .as_array()
        .expect("validated Timeline array");
    let timeline_ids = timeline
        .iter()
        .map(|item| item["id"].as_str().expect("validated Timeline id"))
        .collect::<Vec<_>>();
    let timeline_mcp_ids = timeline
        .iter()
        .filter(|item| item["type"] == "mcp_tool_call")
        .map(|item| {
            item["invocationId"]
                .as_str()
                .expect("validated Timeline MCP invocation id")
        })
        .collect::<Vec<_>>();
    let tool_calls_by_id = run["toolCalls"]
        .as_array()
        .expect("validated ToolCall array")
        .iter()
        .map(|call| {
            (
                call["id"].as_str().expect("validated ToolCall id"),
                call["tool"].as_str().expect("validated ToolCall name"),
            )
        })
        .collect::<std::collections::HashMap<_, _>>();
    let unique =
        |values: &[&str]| values.iter().copied().collect::<HashSet<_>>().len() == values.len();
    if !unique(&tool_call_ids)
        || !unique(&tool_result_ids)
        || !unique(&mcp_action_ids)
        || !unique(&mcp_invocation_ids)
        || !unique(&mcp_call_ids)
        || !unique(&timeline_ids)
        || !unique(&timeline_mcp_ids)
        || tool_call_ids.iter().any(|id| mcp_call_ids.contains(id))
        || tool_result_ids.iter().any(|id| mcp_call_ids.contains(id))
        || timeline.iter().any(|item| {
            item["type"] == "tool_call"
                && item["callId"]
                    .as_str()
                    .is_some_and(|id| mcp_call_ids.contains(&id))
        })
        || (require_timeline_tool_identity_coherence
            && timeline.iter().any(|item| {
                if item["type"] != "tool_call" {
                    return false;
                }
                let Some(identity) = item.get("identity") else {
                    return false;
                };
                let Some(call_id) = item["callId"].as_str() else {
                    return true;
                };
                let Some(tool_name) = tool_calls_by_id.get(call_id) else {
                    return true;
                };
                serde_json::from_value::<crate::AgentToolIdentity>(identity.clone())
                    .map_err(|_| ())
                    .and_then(|identity| {
                        crate::conversation_trace::validate_tool_identity(tool_name, &identity)
                            .map_err(|_| ())
                    })
                    .is_err()
            }))
        || timeline_mcp_ids
            .iter()
            .any(|id| !mcp_invocation_ids.contains(id))
        || mcp_invocation_ids
            .iter()
            .any(|id| !timeline_mcp_ids.contains(id))
    {
        return false;
    }
    true
}

pub(crate) fn canonical_agent_run_lifecycle_projection(
    existing_agent_run_json: Option<&str>,
    run_id: &str,
    run_status: &str,
    started_at: i64,
    updated_at: i64,
    completed_at: Option<i64>,
) -> rusqlite::Result<String> {
    let mut run = existing_agent_run_json
        .and_then(|raw| serde_json::from_str::<serde_json::Value>(raw).ok())
        .and_then(|value| value.as_object().cloned())
        .filter(|run| current_agent_run_projection_is_safe(run, run_id))
        .unwrap_or_default();

    run.insert("runId".to_string(), run_id.into());
    run.insert("status".to_string(), run_status.into());
    run.entry("startedAt".to_string())
        .or_insert_with(|| started_at.into());
    if let Some(completed_at) = completed_at {
        run.insert("completedAt".to_string(), completed_at.into());
    } else {
        run.remove("completedAt");
    }

    for field in [
        "toolDefinitions",
        "toolCalls",
        "toolResults",
        "approvals",
        "fileChangeProposals",
        "fileChanges",
        "webSearchActivities",
        "readActivities",
        "mcpInvocations",
        "timeline",
    ] {
        if !run.get(field).is_some_and(serde_json::Value::is_array) {
            run.insert(field.to_string(), serde_json::json!([]));
        }
    }
    if !run
        .get("messageStreamCheckpoints")
        .is_some_and(serde_json::Value::is_object)
    {
        run.insert(
            "messageStreamCheckpoints".to_string(),
            serde_json::json!({}),
        );
    }

    let state = run
        .entry("state".to_string())
        .or_insert_with(|| serde_json::json!({}));
    if !state.is_object() {
        *state = serde_json::json!({});
    }
    let state = state
        .as_object_mut()
        .expect("canonical AgentRun lifecycle installs an object state");
    state.insert("status".to_string(), run_status.into());
    let active_run_id = if completed_at.is_none() {
        serde_json::Value::String(run_id.to_string())
    } else {
        serde_json::Value::Null
    };
    state.insert("activeRunId".to_string(), active_run_id);
    if !state
        .get("lastError")
        .is_some_and(|value| value.is_string() || value.is_null())
    {
        state.insert("lastError".to_string(), serde_json::Value::Null);
    }
    state.insert("updatedAt".to_string(), updated_at.into());

    serde_json::to_string(&serde_json::Value::Object(run))
        .map_err(|error| rusqlite::Error::ToSqlConversionFailure(Box::new(error)))
}
