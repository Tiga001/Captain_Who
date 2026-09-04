use super::*;
pub(super) fn current_skill_materialization_is_safe(
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

pub(super) fn current_skill_script_is_safe(
    script: &serde_json::Map<String, serde_json::Value>,
) -> bool {
    exact_keys(
        script,
        &[
            "id",
            "scriptUri",
            "skillId",
            "skillRevision",
            "resourcePath",
            "resourceDigest",
            "source",
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
        && current_skill_script_source_is_safe(&script["source"])
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

fn current_skill_script_source_is_safe(value: &serde_json::Value) -> bool {
    let Some(source) = value.as_object() else {
        return false;
    };
    exact_keys(source, &["sourceId", "sourceKind", "trust"])
        && bounded_string(&source["sourceId"], 2_048, false)
        && matches!(
            source["sourceKind"].as_str(),
            Some("workspace" | "bundled" | "installed")
        )
        && matches!(
            source["trust"].as_str(),
            Some("untrusted" | "user_approved" | "application")
        )
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

pub(super) fn current_office_operation_is_safe(
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

pub(super) fn current_todo_is_safe(value: &serde_json::Value) -> bool {
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

pub(super) fn current_skill_installation_is_safe(
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

pub(super) fn current_activated_skill_is_safe(
    skill: &serde_json::Map<String, serde_json::Value>,
) -> bool {
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

pub(super) fn current_skill_selection_is_safe(
    selection: &serde_json::Map<String, serde_json::Value>,
) -> bool {
    exact_keys(selection, &["id", "revision"])
        && bounded_string(&selection["id"], 1_024, false)
        && bounded_string(&selection["revision"], 1_024, false)
}
