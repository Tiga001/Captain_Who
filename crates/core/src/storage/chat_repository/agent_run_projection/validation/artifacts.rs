use super::*;
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

const MAX_CURRENT_ARTIFACT_OBSERVATION_BYTES: usize = 512 * 1024;
const MAX_CURRENT_ARTIFACT_CHANGES: usize = 256;
const MAX_CURRENT_ARTIFACT_EXPECTED_OUTPUTS: usize = 32;
const MAX_CURRENT_ARTIFACT_WARNINGS: usize = 64;
const MAX_CURRENT_ARTIFACT_PATH_BYTES: usize = 16 * 1024;
const MAX_CURRENT_ARTIFACT_TEXT_BYTES: usize = 16 * 1024;

fn current_artifact_text_is_safe(value: &str, maximum: usize) -> bool {
    !value.trim().is_empty()
        && value.len() <= maximum
        && !value.bytes().any(|byte| byte <= b'\x1f' || byte == b'\x7f')
}

fn current_artifact_metadata_is_safe(metadata: &crate::AgentCommandArtifactMetadata) -> bool {
    metadata.size_bytes <= MAX_JS_SAFE_INTEGER
        && metadata.sha256.as_ref().is_none_or(|digest| {
            digest.len() == 64
                && digest
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        })
        && metadata.validation.code.as_deref().is_none_or(|value| {
            current_artifact_text_is_safe(value, MAX_CURRENT_ARTIFACT_TEXT_BYTES)
        })
        && metadata.validation.message.as_deref().is_none_or(|value| {
            current_artifact_text_is_safe(value, MAX_CURRENT_ARTIFACT_TEXT_BYTES)
        })
}

fn current_artifact_snapshot_coverage_is_safe(
    coverage: &crate::AgentCommandArtifactSnapshotCoverage,
) -> bool {
    [
        coverage.roots_scanned,
        coverage.directory_entries_scanned,
        coverage.office_files_seen,
        coverage.files_hashed,
        coverage.files_unhashed,
        coverage.bytes_hashed,
        coverage.symlinks_skipped,
        coverage.excluded_directories,
        coverage.duration_ms,
    ]
    .into_iter()
    .all(|value| value <= MAX_JS_SAFE_INTEGER)
}

fn current_command_artifact_observation_is_safe(value: &serde_json::Value) -> bool {
    if serde_json::to_vec(value)
        .ok()
        .is_none_or(|encoded| encoded.len() > MAX_CURRENT_ARTIFACT_OBSERVATION_BYTES)
    {
        return false;
    }
    let Ok(observation) =
        serde_json::from_value::<crate::AgentCommandArtifactObservation>(value.clone())
    else {
        return false;
    };
    let complete = matches!(
        observation.status,
        crate::AgentCommandArtifactObservationStatus::Complete
    );
    if observation.schema_version != crate::AGENT_COMMAND_ARTIFACT_OBSERVATION_SCHEMA_VERSION
        || complete == observation.partial
        || observation.stop_reasons.len() > MAX_CURRENT_ARTIFACT_WARNINGS
        || observation.changes.len() > MAX_CURRENT_ARTIFACT_CHANGES
        || observation.expected_outputs.len() > MAX_CURRENT_ARTIFACT_EXPECTED_OUTPUTS
        || observation.warnings.len() > MAX_CURRENT_ARTIFACT_WARNINGS
        || observation.returned != observation.changes.len() as u64
        || [
            observation.scanned,
            observation.returned,
            observation.omitted,
            observation.changes_omitted,
            observation.coverage.expected_output_count,
            observation.coverage.additional_root_count,
        ]
        .into_iter()
        .any(|number| number > MAX_JS_SAFE_INTEGER)
        || !current_artifact_snapshot_coverage_is_safe(&observation.coverage.before)
        || !current_artifact_snapshot_coverage_is_safe(&observation.coverage.after)
        || observation
            .stop_reasons
            .iter()
            .any(|reason| !current_artifact_text_is_safe(reason, MAX_CURRENT_ARTIFACT_TEXT_BYTES))
    {
        return false;
    }

    let changes_are_safe = observation.changes.iter().all(|change| {
        current_artifact_text_is_safe(&change.path, MAX_CURRENT_ARTIFACT_PATH_BYTES)
            && change.previous_path.as_deref().is_none_or(|path| {
                current_artifact_text_is_safe(path, MAX_CURRENT_ARTIFACT_PATH_BYTES)
            })
            && (change.previous_path.is_some() == change.previous_scope.is_some())
            && change
                .before
                .as_ref()
                .is_none_or(current_artifact_metadata_is_safe)
            && change
                .after
                .as_ref()
                .is_none_or(current_artifact_metadata_is_safe)
    });
    let expected_outputs_are_safe = observation.expected_outputs.iter().all(|output| {
        current_artifact_text_is_safe(&output.requested_path, MAX_CURRENT_ARTIFACT_PATH_BYTES)
            && output.path.as_deref().is_none_or(|path| {
                current_artifact_text_is_safe(path, MAX_CURRENT_ARTIFACT_PATH_BYTES)
            })
            && output
                .metadata
                .as_ref()
                .is_none_or(current_artifact_metadata_is_safe)
    });
    let warnings_are_safe = observation.warnings.iter().all(|warning| {
        current_artifact_text_is_safe(&warning.code, MAX_CURRENT_ARTIFACT_TEXT_BYTES)
            && warning.path.as_deref().is_none_or(|path| {
                current_artifact_text_is_safe(path, MAX_CURRENT_ARTIFACT_PATH_BYTES)
            })
            && current_artifact_text_is_safe(&warning.message, MAX_CURRENT_ARTIFACT_TEXT_BYTES)
    });

    changes_are_safe && expected_outputs_are_safe && warnings_are_safe
}

pub(super) fn current_command_sessions_are_safe(
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
                    &[
                        "startedAt",
                        "endedAt",
                        "exitCode",
                        "outputs",
                        "artifactObservation",
                    ],
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
                && session
                    .get("artifactObservation")
                    .is_none_or(current_command_artifact_observation_is_safe)
        })
}

pub(super) fn current_file_change_snapshot_is_safe(
    snapshot: &serde_json::Map<String, serde_json::Value>,
) -> bool {
    if !exact_keys(
        snapshot,
        &[
            "schemaVersion",
            "transactionId",
            "conversationId",
            "projectId",
            "filePath",
            "operation",
            "updateStrategy",
            "status",
            "baseRevision",
            "additions",
            "deletions",
            "lineCount",
            "byteCount",
            "mutationCount",
            "nextMutationIndex",
            "statsFinal",
            "summary",
            "createdAt",
            "updatedAt",
        ],
    ) || snapshot["schemaVersion"] != crate::AGENT_FILE_CHANGE_PROTOCOL_SCHEMA_VERSION
        || !bounded_string(&snapshot["transactionId"], 1_024, false)
        || !bounded_string(&snapshot["conversationId"], 1_024, false)
        || !nullable_bounded_string(&snapshot["projectId"], 1_024, false)
        || !bounded_string(&snapshot["filePath"], 16 * 1_024, false)
        || !current_file_change_snapshot_operation_is_safe(snapshot)
        || !nullable_bounded_string(&snapshot["summary"], 16 * 1_024, true)
        || ![
            "additions",
            "deletions",
            "lineCount",
            "byteCount",
            "mutationCount",
            "nextMutationIndex",
            "createdAt",
            "updatedAt",
        ]
        .iter()
        .all(|field| safe_integer(&snapshot[*field]))
        || snapshot["statsFinal"].as_bool().is_none()
    {
        return false;
    }

    let Some(status) = snapshot["status"].as_str() else {
        return false;
    };
    let stats_final = snapshot["statsFinal"].as_bool().unwrap_or(false);
    let stats_state_is_safe = match status {
        "drafting" | "ready" => !stats_final,
        "waiting_approval" | "applying" | "applied" | "already_applied" | "rejected"
        | "conflict" | "failed" | "outcome_unknown" | "aborted" | "expired" => stats_final,
        _ => false,
    };
    stats_state_is_safe
        && snapshot["mutationCount"] == snapshot["nextMutationIndex"]
        && snapshot["updatedAt"]
            .as_u64()
            .zip(snapshot["createdAt"].as_u64())
            .is_some_and(|(updated_at, created_at)| updated_at >= created_at)
        && (snapshot["operation"] != "delete"
            || (snapshot["lineCount"] == 0 && snapshot["byteCount"] == 0))
}

fn current_file_change_snapshot_operation_is_safe(
    snapshot: &serde_json::Map<String, serde_json::Value>,
) -> bool {
    let operation = snapshot["operation"].as_str();
    let strategy = snapshot["updateStrategy"].as_str();
    let strategy_is_safe =
        snapshot["updateStrategy"].is_null() || matches!(strategy, Some("modify" | "rewrite"));
    let base_revision_is_safe = nullable_bounded_string(&snapshot["baseRevision"], 2_048, false);
    strategy_is_safe
        && base_revision_is_safe
        && match operation {
            Some("create") => strategy.is_none() && snapshot["baseRevision"].is_null(),
            Some("update") => strategy.is_some() && !snapshot["baseRevision"].is_null(),
            Some("delete") => strategy.is_none() && !snapshot["baseRevision"].is_null(),
            _ => false,
        }
}
