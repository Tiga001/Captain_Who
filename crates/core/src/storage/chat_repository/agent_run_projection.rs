use crate::AgentMcpServerScope;
use std::collections::HashSet;

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
    "diffs",
    "fileDrafts",
    "commandSessions",
    "mcpInvocations",
    "messageStreamCheckpoints",
    "timeline",
    "state",
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
pub(super) const MAX_JS_SAFE_INTEGER: u64 = 9_007_199_254_740_991;

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

pub(super) fn current_uuid_is_safe(value: &str) -> bool {
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

pub(super) fn current_persisted_approval_is_safe(
    approval: &serde_json::Map<String, serde_json::Value>,
) -> bool {
    match approval.get("type").and_then(serde_json::Value::as_str) {
        Some("tool_call") => {
            exact_keys(approval, &["type", "call"])
                && approval["call"]
                    .as_object()
                    .is_some_and(current_tool_call_is_safe)
        }
        Some("diff") => {
            exact_keys(approval, &["type", "diff"])
                && approval["diff"]
                    .as_object()
                    .is_some_and(current_diff_is_safe)
        }
        Some("file_write") => {
            exact_keys(approval, &["type", "fileWrite"])
                && approval["fileWrite"]
                    .as_object()
                    .is_some_and(current_file_write_approval_is_safe)
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

fn current_file_write_approval_is_safe(
    proposal: &serde_json::Map<String, serde_json::Value>,
) -> bool {
    exact_keys(
        proposal,
        &[
            "id",
            "draftId",
            "mode",
            "filePath",
            "baseRevision",
            "summary",
            "additions",
            "deletions",
            "lineCount",
            "byteCount",
            "approvalStatus",
        ],
    ) && bounded_string(&proposal["id"], 1_024, false)
        && bounded_string(&proposal["draftId"], 1_024, false)
        && matches!(
            proposal["mode"].as_str(),
            Some("create" | "rewrite" | "modify" | "append" | "upsert")
        )
        && bounded_string(&proposal["filePath"], 16 * 1_024, false)
        && nullable_bounded_string(&proposal["baseRevision"], 1_024, false)
        && nullable_bounded_string(&proposal["summary"], 16 * 1_024, true)
        && ["additions", "deletions", "lineCount", "byteCount"]
            .iter()
            .all(|field| safe_integer(&proposal[*field]))
        && current_approval_status_is_safe(&proposal["approvalStatus"])
        && serde_json::from_value::<crate::AgentFileWriteProposal>(serde_json::Value::Object(
            proposal.clone(),
        ))
        .is_ok()
}

fn current_skill_materialization_is_safe(
    materialization: &serde_json::Map<String, serde_json::Value>,
) -> bool {
    exact_keys(
        materialization,
        &[
            "id",
            "sourceUri",
            "sourcePrefix",
            "destination",
            "approvalStatus",
            "reason",
        ],
    ) && bounded_string(&materialization["id"], 1_024, false)
        && bounded_string(&materialization["sourceUri"], 16 * 1_024, false)
        && nullable_bounded_string(&materialization["sourcePrefix"], 16 * 1_024, true)
        && bounded_string(&materialization["destination"], 16 * 1_024, false)
        && current_approval_status_is_safe(&materialization["approvalStatus"])
        && nullable_bounded_string(&materialization["reason"], 16 * 1_024, true)
        && serde_json::from_value::<crate::AgentSkillMaterializationRequest>(
            serde_json::Value::Object(materialization.clone()),
        )
        .is_ok()
}

fn current_skill_script_requirements_are_safe(value: &serde_json::Value) -> bool {
    let Some(requirements) = value.as_object() else {
        return false;
    };
    exact_required_optional_keys(requirements, &[], &["pythonDistributions", "commands"])
        && requirements
            .get("pythonDistributions")
            .is_none_or(|value| current_string_array_is_safe(value, 1_024))
        && requirements
            .get("commands")
            .is_none_or(|value| current_string_array_is_safe(value, 1_024))
}

fn current_skill_script_preflight_is_safe(value: &serde_json::Value) -> bool {
    let Some(preflight) = value.as_object() else {
        return false;
    };
    exact_required_optional_keys(
        preflight,
        &["status", "interpreter", "runtimeFingerprint"],
        &["interpreterVersion", "dependencies", "errorCode", "message"],
    ) && matches!(
        preflight["status"].as_str(),
        Some("ready" | "missing_dependencies" | "unsupported" | "conflict")
    ) && preflight["interpreter"] == "python3"
        && bounded_string(&preflight["runtimeFingerprint"], 4_096, false)
        && optional_bounded_string(preflight, "interpreterVersion", 1_024, false)
        && preflight.get("dependencies").is_none_or(|dependencies| {
            record_array_is_safe(dependencies, |dependency| {
                exact_required_optional_keys(dependency, &["kind", "name", "status"], &["version"])
                    && matches!(
                        dependency["kind"].as_str(),
                        Some("python_distribution" | "command")
                    )
                    && bounded_string(&dependency["name"], 1_024, false)
                    && matches!(dependency["status"].as_str(), Some("available" | "missing"))
                    && optional_bounded_string(dependency, "version", 1_024, false)
            })
        })
        && optional_bounded_string(preflight, "errorCode", 1_024, false)
        && optional_bounded_string(preflight, "message", 128 * 1_024, true)
}

fn current_skill_script_is_safe(script: &serde_json::Map<String, serde_json::Value>) -> bool {
    exact_keys(
        script,
        &[
            "id",
            "scriptUri",
            "skillId",
            "skillRevision",
            "resourcePath",
            "resourceDigest",
            "interpreter",
            "args",
            "requirements",
            "preflight",
            "timeoutMs",
            "approvalStatus",
            "reason",
        ],
    ) && bounded_string(&script["id"], 1_024, false)
        && bounded_string(&script["scriptUri"], 16 * 1_024, false)
        && bounded_string(&script["skillId"], 1_024, false)
        && bounded_string(&script["skillRevision"], 1_024, false)
        && bounded_string(&script["resourcePath"], 16 * 1_024, false)
        && bounded_string(&script["resourceDigest"], 1_024, false)
        && script["interpreter"] == "python3"
        && current_string_array_is_safe(&script["args"], 64 * 1_024)
        && current_skill_script_requirements_are_safe(&script["requirements"])
        && current_skill_script_preflight_is_safe(&script["preflight"])
        && (script["timeoutMs"].is_null() || safe_integer(&script["timeoutMs"]))
        && current_approval_status_is_safe(&script["approvalStatus"])
        && nullable_bounded_string(&script["reason"], 16 * 1_024, true)
        && serde_json::from_value::<crate::AgentSkillScriptRequest>(serde_json::Value::Object(
            script.clone(),
        ))
        .is_ok()
}

fn current_file_input_ref_is_safe(value: &serde_json::Value) -> bool {
    let Some(reference) = value.as_object() else {
        return false;
    };
    match reference.get("type").and_then(serde_json::Value::as_str) {
        Some("attachment") => {
            exact_keys(reference, &["type", "readPath"])
                && bounded_string(&reference["readPath"], 16 * 1_024, false)
        }
        Some("workspace" | "external") => {
            exact_keys(reference, &["type", "path"])
                && bounded_string(&reference["path"], 16 * 1_024, false)
        }
        Some("generated_artifact") => {
            exact_keys(reference, &["type", "uri", "path"])
                && bounded_string(&reference["uri"], 16 * 1_024, false)
                && bounded_string(&reference["path"], 16 * 1_024, false)
        }
        Some("skill_resource") => {
            exact_keys(reference, &["type", "uri"])
                && bounded_string(&reference["uri"], 16 * 1_024, false)
        }
        Some(_) | None => false,
    }
}

fn current_file_input_spec_is_safe(value: &serde_json::Value) -> bool {
    let Some(input) = value.as_object() else {
        return false;
    };
    exact_keys(input, &["mountPath", "source"])
        && bounded_string(&input["mountPath"], 16 * 1_024, false)
        && current_file_input_ref_is_safe(&input["source"])
}

fn current_file_input_binding_is_safe(value: &serde_json::Value) -> bool {
    let Some(binding) = value.as_object() else {
        return false;
    };
    exact_keys(
        binding,
        &[
            "schemaVersion",
            "mountPath",
            "source",
            "sizeBytes",
            "sha256",
        ],
    ) && binding["schemaVersion"] == crate::AGENT_FILE_INPUT_BINDING_SCHEMA_VERSION
        && bounded_string(&binding["mountPath"], 16 * 1_024, false)
        && current_file_input_ref_is_safe(&binding["source"])
        && safe_integer(&binding["sizeBytes"])
        && binding["sha256"].as_str().is_some_and(|digest| {
            digest.len() == 64
                && digest
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        })
}

fn current_office_property_map_is_safe(value: &serde_json::Value) -> bool {
    value.as_object().is_some_and(|properties| {
        properties.values().all(|property| {
            property.is_string()
                || property.is_number()
                || property.is_boolean()
                || property.as_object().is_some_and(|resource| {
                    exact_keys(resource, &["resourcePath"])
                        && bounded_string(&resource["resourcePath"], 16 * 1_024, false)
                })
        })
    })
}

fn current_office_position_is_safe(value: &serde_json::Value) -> bool {
    let Some(position) = value.as_object() else {
        return false;
    };
    match position.get("type").and_then(serde_json::Value::as_str) {
        Some("index") => {
            exact_keys(position, &["type", "index"]) && safe_integer(&position["index"])
        }
        Some("after" | "before") => {
            exact_keys(position, &["type", "target"])
                && bounded_string(&position["target"], 16 * 1_024, false)
        }
        Some(_) | None => false,
    }
}

fn current_office_parameters_are_safe(value: &serde_json::Value) -> bool {
    let Some(parameters) = value.as_object() else {
        return false;
    };
    match parameters.get("type").and_then(serde_json::Value::as_str) {
        Some("help") => {
            exact_required_optional_keys(parameters, &["type"], &["verb", "element"])
                && parameters.get("verb").is_none_or(|value| {
                    matches!(
                        value.as_str(),
                        Some(
                            "status"
                                | "help"
                                | "create"
                                | "view"
                                | "get"
                                | "query"
                                | "validate"
                                | "set"
                                | "add"
                                | "remove"
                                | "move"
                                | "swap"
                        )
                    )
                })
                && optional_bounded_string(parameters, "element", 1_024, false)
        }
        Some("create") => {
            exact_required_optional_keys(parameters, &["type"], &["locale", "minimal", "overwrite"])
                && optional_bounded_string(parameters, "locale", 1_024, false)
                && parameters
                    .get("minimal")
                    .is_none_or(serde_json::Value::is_boolean)
                && parameters
                    .get("overwrite")
                    .is_none_or(serde_json::Value::is_boolean)
        }
        Some("view") => {
            exact_required_optional_keys(
                parameters,
                &["type", "mode"],
                &[
                    "start",
                    "end",
                    "maxLines",
                    "issueType",
                    "limit",
                    "columns",
                    "pages",
                    "range",
                    "viewport",
                    "grid",
                    "renderMode",
                    "pageCount",
                ],
            ) && matches!(
                parameters["mode"].as_str(),
                Some(
                    "text"
                        | "annotated"
                        | "outline"
                        | "stats"
                        | "issues"
                        | "html"
                        | "svg"
                        | "screenshot"
                        | "forms"
                )
            ) && ["start", "end", "maxLines", "limit"]
                .iter()
                .all(|field| optional_safe_integer(parameters, field))
                && optional_bounded_string(parameters, "issueType", 1_024, false)
                && parameters
                    .get("columns")
                    .is_none_or(|value| current_string_array_is_safe(value, 1_024))
                && parameters.get("pages").is_none_or(|pages| {
                    record_array_is_safe(pages, |page| {
                        exact_required_optional_keys(page, &["start"], &["end"])
                            && safe_integer(&page["start"])
                            && optional_safe_integer(page, "end")
                    })
                })
                && optional_bounded_string(parameters, "range", 16 * 1_024, false)
                && parameters.get("viewport").is_none_or(|viewport| {
                    viewport.as_object().is_some_and(|viewport| {
                        exact_keys(viewport, &["width", "height"])
                            && safe_integer(&viewport["width"])
                            && viewport["width"].as_u64() != Some(0)
                            && safe_integer(&viewport["height"])
                            && viewport["height"].as_u64() != Some(0)
                    })
                })
                && parameters.get("grid").is_none_or(|grid| {
                    grid.as_object().is_some_and(|grid| {
                        matches!(
                            grid.get("mode").and_then(serde_json::Value::as_str),
                            Some("auto")
                        ) && exact_keys(grid, &["mode"])
                            || matches!(
                                grid.get("mode").and_then(serde_json::Value::as_str),
                                Some("columns")
                            ) && exact_keys(grid, &["mode", "columns"])
                                && safe_integer(&grid["columns"])
                                && grid["columns"].as_u64() != Some(0)
                    })
                })
                && parameters
                    .get("renderMode")
                    .is_none_or(|value| matches!(value.as_str(), Some("auto" | "html")))
                && parameters
                    .get("pageCount")
                    .is_none_or(serde_json::Value::is_boolean)
        }
        Some("get") => {
            exact_required_optional_keys(parameters, &["type"], &["target", "depth"])
                && optional_bounded_string(parameters, "target", 16 * 1_024, false)
                && optional_safe_integer(parameters, "depth")
        }
        Some("query") => {
            exact_required_optional_keys(
                parameters,
                &["type", "selector"],
                &["contains", "compact", "fields"],
            ) && bounded_string(&parameters["selector"], 16 * 1_024, false)
                && optional_bounded_string(parameters, "contains", 16 * 1_024, true)
                && parameters
                    .get("compact")
                    .is_none_or(serde_json::Value::is_boolean)
                && parameters
                    .get("fields")
                    .is_none_or(|value| current_string_array_is_safe(value, 1_024))
        }
        Some("validate") => exact_keys(parameters, &["type"]),
        Some("set") => {
            exact_required_optional_keys(
                parameters,
                &["type", "target"],
                &["properties", "replacement", "force"],
            ) && bounded_string(&parameters["target"], 16 * 1_024, false)
                && parameters
                    .get("properties")
                    .is_none_or(current_office_property_map_is_safe)
                && parameters.get("replacement").is_none_or(|replacement| {
                    replacement.as_object().is_some_and(|replacement| {
                        exact_keys(replacement, &["find", "replace"])
                            && bounded_string(&replacement["find"], 128 * 1_024, true)
                            && bounded_string(&replacement["replace"], 128 * 1_024, true)
                    })
                })
                && parameters
                    .get("force")
                    .is_none_or(serde_json::Value::is_boolean)
        }
        Some("add") => {
            exact_required_optional_keys(
                parameters,
                &["type", "parent", "elementType"],
                &["copyFrom", "position", "properties", "force"],
            ) && bounded_string(&parameters["parent"], 16 * 1_024, false)
                && bounded_string(&parameters["elementType"], 1_024, false)
                && optional_bounded_string(parameters, "copyFrom", 16 * 1_024, false)
                && parameters
                    .get("position")
                    .is_none_or(current_office_position_is_safe)
                && parameters
                    .get("properties")
                    .is_none_or(current_office_property_map_is_safe)
                && parameters
                    .get("force")
                    .is_none_or(serde_json::Value::is_boolean)
        }
        Some("remove") => {
            exact_required_optional_keys(parameters, &["type", "target"], &["shift", "properties"])
                && bounded_string(&parameters["target"], 16 * 1_024, false)
                && parameters
                    .get("shift")
                    .is_none_or(|value| matches!(value.as_str(), Some("left" | "up")))
                && parameters
                    .get("properties")
                    .is_none_or(current_office_property_map_is_safe)
        }
        Some("move") => {
            exact_required_optional_keys(
                parameters,
                &["type", "target"],
                &["newParent", "position", "properties"],
            ) && bounded_string(&parameters["target"], 16 * 1_024, false)
                && optional_bounded_string(parameters, "newParent", 16 * 1_024, false)
                && parameters
                    .get("position")
                    .is_none_or(current_office_position_is_safe)
                && parameters
                    .get("properties")
                    .is_none_or(current_office_property_map_is_safe)
        }
        Some("swap") => {
            exact_keys(parameters, &["type", "firstTarget", "secondTarget"])
                && bounded_string(&parameters["firstTarget"], 16 * 1_024, false)
                && bounded_string(&parameters["secondTarget"], 16 * 1_024, false)
        }
        Some(_) | None => false,
    }
}

fn current_office_request_is_safe(value: &serde_json::Value) -> bool {
    let Some(request) = value.as_object() else {
        return false;
    };
    exact_keys(
        request,
        &[
            "documentKind",
            "operation",
            "documentPath",
            "outputPath",
            "destinationPath",
            "inputs",
            "timeoutMs",
            "parameters",
        ],
    ) && matches!(
        request["documentKind"].as_str(),
        Some("document" | "spreadsheet" | "presentation")
    ) && matches!(
        request["operation"].as_str(),
        Some(
            "help"
                | "create"
                | "view"
                | "get"
                | "query"
                | "validate"
                | "set"
                | "add"
                | "remove"
                | "move"
                | "swap"
        )
    ) && nullable_bounded_string(&request["documentPath"], 16 * 1_024, false)
        && nullable_bounded_string(&request["outputPath"], 16 * 1_024, false)
        && nullable_bounded_string(&request["destinationPath"], 16 * 1_024, false)
        && request["inputs"].as_array().is_some_and(|inputs| {
            inputs.len() <= MAX_CURRENT_RUN_ITEMS
                && inputs.iter().all(current_file_input_spec_is_safe)
        })
        && (request["timeoutMs"].is_null() || safe_integer(&request["timeoutMs"]))
        && current_office_parameters_are_safe(&request["parameters"])
        && request["parameters"]
            .get("type")
            .and_then(serde_json::Value::as_str)
            == request["operation"].as_str()
}

fn current_office_path_slot_is_safe(value: &serde_json::Value) -> bool {
    let Some(slot) = value.as_object() else {
        return false;
    };
    match slot.get("type").and_then(serde_json::Value::as_str) {
        Some("document" | "output" | "destination") => exact_keys(slot, &["type"]),
        Some("resource") => exact_keys(slot, &["type", "index"]) && safe_integer(&slot["index"]),
        Some(_) | None => false,
    }
}

fn current_office_path_identity_is_safe(value: &serde_json::Value) -> bool {
    let Some(identity) = value.as_object() else {
        return false;
    };
    exact_keys(identity, &["revision", "device", "inode"])
        && bounded_string(&identity["revision"], 1_024, false)
        && (identity["device"].is_null() || safe_integer(&identity["device"]))
        && (identity["inode"].is_null() || safe_integer(&identity["inode"]))
}

fn current_office_frozen_path_is_safe(value: &serde_json::Value) -> bool {
    let Some(path) = value.as_object() else {
        return false;
    };
    exact_keys(
        path,
        &[
            "slot",
            "logicalPath",
            "purpose",
            "scope",
            "normalizedPath",
            "state",
            "objectIdentity",
            "parentIdentity",
            "contentRevision",
            "size",
            "writeDisposition",
        ],
    ) && current_office_path_slot_is_safe(&path["slot"])
        && bounded_string(&path["logicalPath"], 16 * 1_024, false)
        && matches!(
            path["purpose"].as_str(),
            Some("readSource" | "writeTarget" | "inPlaceTarget")
        )
        && matches!(
            path["scope"].as_str(),
            Some("workspace" | "external" | "attachment")
        )
        && bounded_string(&path["normalizedPath"], 16 * 1_024, false)
        && matches!(path["state"].as_str(), Some("missing" | "present"))
        && (path["objectIdentity"].is_null()
            || current_office_path_identity_is_safe(&path["objectIdentity"]))
        && current_office_path_identity_is_safe(&path["parentIdentity"])
        && nullable_bounded_string(&path["contentRevision"], 1_024, false)
        && (path["size"].is_null() || safe_integer(&path["size"]))
        && (path["writeDisposition"].is_null()
            || matches!(
                path["writeDisposition"].as_str(),
                Some("createNew" | "replaceExisting")
            ))
}

fn current_office_render_plan_is_safe(value: &serde_json::Value) -> bool {
    let Ok(plan) =
        serde_json::from_value::<crate::office::OfficePresentationRenderPlan>(value.clone())
    else {
        return false;
    };
    !plan.requested_pages.is_empty()
        && plan.requested_pages.len() <= crate::office::MAX_OFFICE_TOTAL_PAGES as usize
        && plan
            .requested_pages
            .iter()
            .all(|page| *page > 0 && *page <= crate::office::MAX_OFFICE_PAGE_NUMBER)
        && plan
            .requested_pages
            .windows(2)
            .all(|pages| pages[0] < pages[1])
        && plan.slide_width_emu > 0
        && plan.slide_height_emu > 0
        && plan.viewport.width > 0
        && plan.viewport.width <= crate::office::MAX_OFFICE_SCREENSHOT_DIMENSION
        && plan.viewport.height > 0
        && plan.viewport.height <= crate::office::MAX_OFFICE_SCREENSHOT_DIMENSION
        && match plan.grid {
            None => true,
            Some(crate::office::OfficeGridLayout::Columns { columns }) => {
                columns > 0 && columns <= crate::office::MAX_OFFICE_GRID_COLUMNS
            }
            Some(crate::office::OfficeGridLayout::Auto) => false,
        }
}

fn current_office_request_requires_render_plan(value: &serde_json::Value) -> bool {
    value["documentKind"] == "presentation"
        && value["operation"] == "view"
        && value["parameters"]["type"] == "view"
        && value["parameters"]["mode"] == "screenshot"
}

fn current_office_prepared_is_safe(value: &serde_json::Value) -> bool {
    let Some(prepared) = value.as_object() else {
        return false;
    };
    exact_keys(
        prepared,
        &[
            "schemaVersion",
            "providerId",
            "engineRevision",
            "workspaceRevision",
            "access",
            "request",
            "argv",
            "resolvedRenderPlan",
            "paths",
            "inputBindings",
        ],
    ) && prepared["schemaVersion"] == crate::office::OFFICE_PREPARED_EXECUTION_SCHEMA_VERSION
        && bounded_string(&prepared["providerId"], 1_024, false)
        && bounded_string(&prepared["engineRevision"], 4_096, false)
        && nullable_bounded_string(&prepared["workspaceRevision"], 4_096, false)
        && matches!(prepared["access"].as_str(), Some("readOnly" | "fileWrite"))
        && current_office_request_is_safe(&prepared["request"])
        && current_string_array_is_safe(&prepared["argv"], 64 * 1_024)
        && if current_office_request_requires_render_plan(&prepared["request"]) {
            current_office_render_plan_is_safe(&prepared["resolvedRenderPlan"])
        } else {
            prepared["resolvedRenderPlan"].is_null()
        }
        && prepared["paths"].as_array().is_some_and(|paths| {
            paths.len() <= MAX_CURRENT_RUN_ITEMS
                && paths.iter().all(current_office_frozen_path_is_safe)
        })
        && prepared["inputBindings"]
            .as_array()
            .is_some_and(|bindings| {
                bindings.len() <= MAX_CURRENT_RUN_ITEMS
                    && bindings.iter().all(current_file_input_binding_is_safe)
            })
}

fn current_office_operation_is_safe(
    operation: &serde_json::Map<String, serde_json::Value>,
) -> bool {
    exact_keys(
        operation,
        &[
            "schemaVersion",
            "id",
            "semanticArgs",
            "prepared",
            "approvalStatus",
            "reason",
        ],
    ) && operation["schemaVersion"] == crate::AGENT_OFFICE_OPERATION_SCHEMA_VERSION
        && bounded_string(&operation["id"], 1_024, false)
        && operation["semanticArgs"].is_object()
        && current_office_prepared_is_safe(&operation["prepared"])
        && current_approval_status_is_safe(&operation["approvalStatus"])
        && bounded_string(&operation["reason"], 16 * 1_024, true)
}

fn current_diff_is_safe(diff: &serde_json::Map<String, serde_json::Value>) -> bool {
    exact_keys(
        diff,
        &[
            "id",
            "operation",
            "filePath",
            "patch",
            "baseRevision",
            "summary",
            "approvalStatus",
        ],
    ) && bounded_string(&diff["id"], 1_024, false)
        && bounded_string(&diff["filePath"], 16 * 1_024, false)
        && bounded_string(&diff["patch"], 4 * 1_024 * 1_024, true)
        && (diff["baseRevision"].is_null() || bounded_string(&diff["baseRevision"], 1_024, false))
        && (diff["summary"].is_null() || bounded_string(&diff["summary"], 16 * 1_024, true))
        && serde_json::from_value::<crate::AgentDiffProposal>(serde_json::Value::Object(
            diff.clone(),
        ))
        .is_ok()
}

fn current_todo_is_safe(value: &serde_json::Value) -> bool {
    let Some(todo) = value.as_object() else {
        return false;
    };
    exact_keys(todo, &["revision", "items", "updatedAt"])
        && safe_integer(&todo["revision"])
        && safe_integer(&todo["updatedAt"])
        && record_array_is_safe(&todo["items"], |item| {
            exact_required_optional_keys(
                item,
                &["id", "title", "status", "createdAt", "updatedAt"],
                &["note"],
            ) && bounded_string(&item["id"], 1_024, false)
                && bounded_string(&item["title"], 64 * 1_024, true)
                && matches!(
                    item["status"].as_str(),
                    Some("pending" | "in_progress" | "completed" | "blocked")
                )
                && safe_integer(&item["createdAt"])
                && safe_integer(&item["updatedAt"])
                && optional_bounded_string(item, "note", 64 * 1_024, true)
        })
}

fn current_skill_installation_is_safe(
    installation: &serde_json::Map<String, serde_json::Value>,
) -> bool {
    if !exact_keys(installation, &["action", "status"])
        || !matches!(
            installation["status"].as_str(),
            Some(
                "waiting_for_approval"
                    | "installing"
                    | "installed"
                    | "already_installed"
                    | "rejected"
                    | "failed"
                    | "uncertain"
            )
        )
    {
        return false;
    }
    let Some(action) = installation["action"].as_object() else {
        return false;
    };
    let Some(preview) = action.get("preview").and_then(serde_json::Value::as_object) else {
        return false;
    };
    let Some(resource_summary) = preview
        .get("resourceSummary")
        .and_then(serde_json::Value::as_object)
    else {
        return false;
    };
    exact_keys(
        action,
        &[
            "schemaVersion",
            "id",
            "installRef",
            "preview",
            "approvalStatus",
            "expiresAt",
        ],
    ) && action["schemaVersion"] == crate::AGENT_SKILL_INSTALLATION_SCHEMA_VERSION
        && bounded_string(&action["id"], 1_024, false)
        && bounded_string(&action["installRef"], 4_096, false)
        && safe_integer(&action["expiresAt"])
        && exact_keys(
            preview,
            &[
                "name",
                "description",
                "sourceSummary",
                "resolvedRevision",
                "fileCount",
                "totalBytes",
                "resourceSummary",
                "containsScripts",
                "warnings",
                "compatibility",
                "operation",
                "impact",
            ],
        )
        && bounded_string(&preview["name"], 1_024, false)
        && bounded_string(&preview["description"], 128 * 1_024, true)
        && bounded_string(&preview["resolvedRevision"], 4_096, false)
        && safe_integer(&preview["fileCount"])
        && safe_integer(&preview["totalBytes"])
        && exact_keys(
            resource_summary,
            &["total", "references", "assets", "scripts", "bytes"],
        )
        && resource_summary.values().all(safe_integer)
        && preview["containsScripts"].is_boolean()
        && record_array_is_safe(&preview["warnings"], |warning| {
            exact_keys(warning, &["code", "message", "requiresAcknowledgement"])
                && bounded_string(&warning["code"], 1_024, false)
                && bounded_string(&warning["message"], 128 * 1_024, true)
                && warning["requiresAcknowledgement"].is_boolean()
        })
        && bounded_string(&preview["compatibility"], 1_024, false)
        && bounded_string(&preview["operation"], 1_024, false)
        && bounded_string(&preview["impact"], 1_024, false)
        && serde_json::from_value::<crate::AgentSkillInstallationRequest>(
            serde_json::Value::Object(action.clone()),
        )
        .is_ok()
}

fn current_activated_skill_is_safe(skill: &serde_json::Map<String, serde_json::Value>) -> bool {
    let Some(source) = skill.get("source").and_then(serde_json::Value::as_object) else {
        return false;
    };
    exact_keys(skill, &["id", "name", "revision", "source"])
        && bounded_string(&skill["id"], 1_024, false)
        && bounded_string(&skill["name"], 1_024, false)
        && bounded_string(&skill["revision"], 1_024, false)
        && exact_keys(source, &["kind", "id"])
        && matches!(
            source["kind"].as_str(),
            Some("workspace" | "bundled" | "installed")
        )
        && bounded_string(&source["id"], 1_024, false)
}

fn current_skill_selection_is_safe(selection: &serde_json::Map<String, serde_json::Value>) -> bool {
    exact_keys(selection, &["id", "revision"])
        && bounded_string(&selection["id"], 1_024, false)
        && bounded_string(&selection["revision"], 1_024, false)
}

fn current_command_output_is_safe(output: &serde_json::Map<String, serde_json::Value>) -> bool {
    if !exact_required_optional_keys(
        output,
        &[
            "name",
            "kind",
            "readPath",
            "mimeType",
            "sizeBytes",
            "sha256",
        ],
        &["width", "height"],
    ) || !bounded_string(&output["name"], 1_024, false)
        || !output["name"].as_str().is_some_and(|name| {
            name == name.trim()
                && !name
                    .chars()
                    .any(|character| character <= '\u{001f}' || character == '\u{007f}')
        })
        || !safe_integer(&output["sizeBytes"])
        || output["sizeBytes"].as_u64() == Some(0)
        || !output["sha256"].as_str().is_some_and(|digest| {
            digest.len() == 64
                && digest
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        })
    {
        return false;
    }
    let digest = output["sha256"].as_str().expect("validated digest");
    match output["kind"].as_str() {
        Some("document") => {
            output["readPath"] == format!("artifact://sha256/{digest}")
                && output["mimeType"] == "application/pdf"
                && !output.contains_key("width")
                && !output.contains_key("height")
        }
        Some("image") => {
            output["readPath"] == format!("image-artifact://sha256/{digest}")
                && matches!(
                    output["mimeType"].as_str(),
                    Some("image/png" | "image/jpeg" | "image/webp")
                )
                && output.get("width").is_some_and(safe_integer)
                && output.get("width").and_then(serde_json::Value::as_u64) != Some(0)
                && output.get("height").is_some_and(safe_integer)
                && output.get("height").and_then(serde_json::Value::as_u64) != Some(0)
        }
        _ => false,
    }
}

fn current_command_sessions_are_safe(
    value: &serde_json::Value,
    run_command_call_ids: &HashSet<&str>,
) -> bool {
    let Some(sessions) = value.as_object() else {
        return false;
    };
    sessions.len() <= MAX_CURRENT_RUN_ITEMS
        && sessions.iter().all(|(call_id, value)| {
            let Some(session) = value.as_object() else {
                return false;
            };
            run_command_call_ids.contains(call_id.as_str())
                && exact_required_optional_keys(
                    session,
                    &["callId", "status", "latestSequence", "outputTruncated"],
                    &["startedAt", "endedAt", "exitCode", "outputs"],
                )
                && session["callId"] == *call_id
                && matches!(
                    session["status"].as_str(),
                    Some("exited" | "interrupted" | "timed_out" | "failed" | "outcome_unknown")
                )
                && safe_integer(&session["latestSequence"])
                && session["outputTruncated"].is_boolean()
                && session.get("startedAt").is_none_or(safe_integer)
                && session.get("endedAt").is_none_or(safe_integer)
                && session.get("exitCode").is_none_or(|value| {
                    value.as_i64().is_some_and(|value| {
                        (-(MAX_JS_SAFE_INTEGER as i64)..=MAX_JS_SAFE_INTEGER as i64)
                            .contains(&value)
                    })
                })
                && session.get("outputs").is_none_or(|outputs| {
                    outputs.as_array().is_some_and(|outputs| {
                        outputs.len() <= 32
                            && outputs.iter().all(|output| {
                                output
                                    .as_object()
                                    .is_some_and(current_command_output_is_safe)
                            })
                    })
                })
        })
}

fn current_file_draft_is_safe(draft: &serde_json::Map<String, serde_json::Value>) -> bool {
    let required = [
        "draftId",
        "conversationId",
        "filePath",
        "mode",
        "status",
        "additions",
        "deletions",
        "lineCount",
        "byteCount",
        "chunkCount",
        "nextChunkIndex",
        "statsFinal",
        "createdAt",
        "updatedAt",
    ];
    exact_required_optional_keys(draft, &required, &["projectId", "baseRevision", "summary"])
        && bounded_string(&draft["draftId"], 1_024, false)
        && bounded_string(&draft["conversationId"], 1_024, true)
        && bounded_string(&draft["filePath"], 16 * 1_024, false)
        && matches!(
            draft["mode"].as_str(),
            Some("create" | "rewrite" | "modify" | "append" | "upsert")
        )
        && matches!(
            draft["status"].as_str(),
            Some(
                "drafting"
                    | "ready"
                    | "waiting_approval"
                    | "applying"
                    | "applied"
                    | "rejected"
                    | "conflict"
                    | "failed"
                    | "aborted"
                    | "expired"
            )
        )
        && [
            "additions",
            "deletions",
            "lineCount",
            "byteCount",
            "chunkCount",
            "nextChunkIndex",
            "createdAt",
            "updatedAt",
        ]
        .iter()
        .all(|field| safe_integer(&draft[*field]))
        && draft["statsFinal"].is_boolean()
        && optional_bounded_string(draft, "projectId", 1_024, false)
        && optional_bounded_string(draft, "baseRevision", 1_024, false)
        && optional_bounded_string(draft, "summary", 16 * 1_024, true)
}

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

fn current_web_activity_is_safe(activity: &serde_json::Map<String, serde_json::Value>) -> bool {
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

fn current_read_activity_is_safe(activity: &serde_json::Map<String, serde_json::Value>) -> bool {
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

fn current_timeline_item_is_safe(item: &serde_json::Map<String, serde_json::Value>) -> bool {
    if !bounded_string(
        item.get("id").unwrap_or(&serde_json::Value::Null),
        1_024,
        false,
    ) {
        return false;
    }
    match item.get("type").and_then(serde_json::Value::as_str) {
        Some("message") => {
            exact_required_optional_keys(item, &["id", "type", "content"], &["streamId"])
                && bounded_string(&item["content"], 4 * 1_024 * 1_024, true)
                && optional_bounded_string(item, "streamId", 1_024, false)
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
            exact_keys(item, &["id", "type", "invocationId"])
                && bounded_string(&item["invocationId"], 1_024, false)
        }
        Some("context_compaction") => {
            exact_keys(item, &["id", "type", "operationId", "status"])
                && bounded_string(&item["operationId"], 1_024, false)
                && matches!(
                    item["status"].as_str(),
                    Some("running" | "applied" | "skipped" | "failed" | "cancelled")
                )
        }
        Some("error") => {
            exact_keys(item, &["id", "type", "message"])
                && bounded_string(&item["message"], 128 * 1_024, true)
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
        }
        _ => false,
    }
}

fn current_message_stream_checkpoints_are_safe(value: &serde_json::Value) -> bool {
    value.as_object().is_some_and(|checkpoints| {
        checkpoints.len() <= MAX_CURRENT_RUN_ITEMS
            && checkpoints.values().all(|checkpoint| {
                checkpoint.as_object().is_some_and(|checkpoint| {
                    exact_keys(checkpoint, &["baseContentLength", "baseWasThinking"])
                        && safe_integer(&checkpoint["baseContentLength"])
                        && checkpoint["baseWasThinking"].is_boolean()
                })
            })
    })
}

fn current_mcp_invocation_is_safe(value: &serde_json::Value) -> bool {
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

pub(super) fn current_agent_run_projection_is_safe(
    run: &serde_json::Map<String, serde_json::Value>,
    expected_run_id: &str,
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
        "diffs",
        "fileDrafts",
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
        || !record_array_is_safe(&run["diffs"], current_diff_is_safe)
        || !record_array_is_safe(&run["fileDrafts"], current_file_draft_is_safe)
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
        || timeline.iter().any(|item| {
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
        })
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
        "diffs",
        "fileDrafts",
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
