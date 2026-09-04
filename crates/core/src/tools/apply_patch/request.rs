use super::support::{
    file_change_agent_error_for_direct, file_change_agent_error_with_continuation,
    freeze_observed_base, read_file_continuation,
};
use super::*;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ApplyPatchInput {
    request: ApplyPatchArgs,
}

#[derive(Debug, Deserialize)]
#[serde(
    tag = "action",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub(super) enum ApplyPatchArgs {
    Apply {
        operation: FileChangeOperation,
        file_path: String,
        observation_id: Option<String>,
        content: Option<String>,
        edits: Option<Vec<StructuredTextEdit>>,
        summary: Option<String>,
    },
    Begin {
        operation: FileChangeOperation,
        file_path: String,
        observation_id: Option<String>,
        strategy: Option<file_change_staged::StagedUpdateStrategy>,
    },
    Append {
        transaction_id: String,
        index: u64,
        expected_draft_revision: u64,
        content: String,
    },
    Edit {
        transaction_id: String,
        index: u64,
        expected_draft_revision: u64,
        edits: Vec<StructuredTextEdit>,
    },
    Commit {
        transaction_id: String,
        expected_draft_revision: u64,
        summary: Option<String>,
    },
    Status {
        transaction_id: String,
    },
    Abort {
        transaction_id: String,
    },
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct StructuredTextEdit {
    pub(super) kind: TextEditKind,
    pub(super) old_text: Option<String>,
    pub(super) new_text: Option<String>,
    pub(super) anchor: Option<String>,
    pub(super) text: Option<String>,
    pub(super) replace_all: Option<bool>,
}

#[derive(Debug, Deserialize, Clone, Copy)]
#[serde(rename_all = "snake_case")]
pub(super) enum TextEditKind {
    Replace,
    InsertBefore,
    InsertAfter,
    Append,
    Prepend,
}

pub(super) fn parse_args(value: Value) -> AgentResult<ApplyPatchArgs> {
    let expected_shape = wire_expected_shape(&value);
    serde_json::from_value::<ApplyPatchInput>(value)
        .map(|input| input.request)
        .map_err(|_| {
            file_change_agent_error_with_continuation(
                FileChangeError::new(FileChangeErrorCode::InvalidArguments),
                None,
                Some(expected_shape),
            )
        })
}

#[cfg(test)]
pub(super) fn direct_proposal_from_call(
    context: &ToolExecutionContext,
    call: &AgentToolCall,
) -> AgentResult<AgentFileChangeProposal> {
    validate_wire_shape(&call.args).map_err(|error| file_change_wire_error(error, &call.args))?;
    let args = parse_args(call.args.clone())?;
    direct_proposal_from_args(context, call, args)
}

pub(super) fn direct_proposal_from_args(
    context: &ToolExecutionContext,
    call: &AgentToolCall,
    args: ApplyPatchArgs,
) -> AgentResult<AgentFileChangeProposal> {
    let ApplyPatchArgs::Apply {
        operation,
        file_path,
        observation_id,
        content,
        edits,
        summary,
    } = args
    else {
        return Err(file_change_agent_error(FileChangeError::new(
            FileChangeErrorCode::InvalidArguments,
        )));
    };
    let staged_recovery = match (operation, content.is_some(), edits.is_some()) {
        (FileChangeOperation::Create, true, false) => Some(DirectStagedRecovery::Create),
        (FileChangeOperation::Update, true, false) => Some(DirectStagedRecovery::Rewrite),
        (FileChangeOperation::Update, false, true) => Some(DirectStagedRecovery::Modify),
        _ => None,
    };
    if context.permissions().write == AgentWritePermission::Denied {
        return Err(file_change_agent_error(FileChangeError::new(
            FileChangeErrorCode::PermissionDenied,
        )));
    }
    let workspace_root = context.workspace_root_optional()?;
    let target = FileChangePathPolicy::new(
        workspace_root.as_deref(),
        context.permissions().write == AgentWritePermission::All,
    )
    .resolve(&file_path)
    .map_err(file_change_agent_error)?;
    let public_observation_id = observation_id.as_deref();
    let observation = match operation {
        FileChangeOperation::Create => {
            if public_observation_id.is_some() {
                return Err(file_change_agent_error(FileChangeError::new(
                    FileChangeErrorCode::IllegalFieldCombination,
                )));
            }
            capture_missing_base_observation(context, &target).map_err(|error| {
                file_change_agent_error_for_direct(error, &file_path, None, staged_recovery)
            })?
        }
        FileChangeOperation::Update | FileChangeOperation::Delete => {
            let observation_id = public_observation_id.ok_or_else(|| {
                file_change_agent_error_with_continuation(
                    FileChangeError::new(FileChangeErrorCode::ObservationRequired),
                    Some(read_file_continuation(&file_path)),
                    None,
                )
            })?;
            context
                .file_observations()
                .validate(
                    observation_id,
                    context.conversation_id()?,
                    context.run_id()?,
                    target.absolute_path(),
                )
                .map_err(|error| {
                    file_change_agent_error_for_direct(
                        error,
                        &file_path,
                        Some(observation_id),
                        staged_recovery,
                    )
                })?
        }
    };
    validate_observation_operation(operation, observation.state()).map_err(|error| {
        file_change_agent_error_for_direct(
            error,
            &file_path,
            public_observation_id,
            staged_recovery,
        )
    })?;
    let frozen_base = freeze_observed_base(context, &target, &observation).map_err(|error| {
        file_change_agent_error_for_direct(
            error,
            &file_path,
            public_observation_id,
            staged_recovery,
        )
    })?;
    let mutation = mutation_from_args(operation, content, edits).map_err(|error| {
        file_change_agent_error_for_direct(
            error,
            &file_path,
            public_observation_id,
            staged_recovery,
        )
    })?;
    let plan = FileChangePlanner
        .plan(FileChangePlanRequest {
            operation,
            file_path: target.display_path(),
            base: frozen_base.as_plan_base(),
            mutation,
        })
        .map_err(|error| {
            file_change_agent_error_for_direct(
                error,
                &file_path,
                public_observation_id,
                staged_recovery,
            )
        })?;
    if plan
        .target_content
        .as_ref()
        .is_some_and(|content| content.len() > MAX_EDIT_CONTENT_BYTES)
    {
        return Err(file_change_agent_error_for_direct(
            FileChangeError::new(FileChangeErrorCode::ContentTooLarge),
            &file_path,
            public_observation_id,
            staged_recovery,
        ));
    }

    let now = now_ms();
    let transaction_id = format!("file-change-direct-v1:{}", Uuid::new_v4());
    let transaction = FileChangeTransaction {
        schema_version: FILE_CHANGE_SCHEMA_VERSION,
        id: transaction_id.clone(),
        operation: plan.operation,
        file_path: plan.file_path.clone(),
        status: FileChangeStatus::WaitingApproval,
        outcome: FileChangeOutcome::DefinitelyNotExecuted,
        base: plan.base.clone(),
        target: plan.target.clone(),
        proposal_digest: plan.proposal_digest.clone(),
        created_at: now,
        updated_at: now,
    };
    let proposal = FileChangeProposal {
        schema_version: FILE_CHANGE_SCHEMA_VERSION,
        id: call.id.clone(),
        transaction_id,
        operation: plan.operation,
        file_path: plan.file_path.clone(),
        base: plan.base.clone(),
        target: plan.target.clone(),
        diff_digest: plan.diff_digest.clone(),
        proposal_digest: plan.proposal_digest.clone(),
        additions: plan.additions,
        deletions: plan.deletions,
    };
    let delete_journal = if plan.operation == FileChangeOperation::Delete {
        Some(
            FileChangeCommitter
                .prepare_delete(&transaction.id, &target, &plan, now)
                .map_err(file_change_agent_error)?,
        )
    } else {
        None
    };
    let execution = FileChangeDirectBinding {
        schema_version: FILE_CHANGE_DIRECT_BINDING_SCHEMA_VERSION,
        transaction,
        proposal,
        observation_id: observation.id().to_string(),
        observation: observation.checkpoint(),
        source_tool_name: "apply_patch".to_string(),
        source_call_id: call.id.clone(),
        source_args_digest: crate::file_change::proposal_digest(&call.args).map_err(|error| {
            file_change_agent_error(FileChangeError::with_diagnostic(
                FileChangeErrorCode::Failed,
                error.to_string(),
            ))
        })?,
        trace_args_digest: crate::file_change_support::apply_patch_trace_args_digest(&call.args)
            .map_err(|error| {
                file_change_agent_error(FileChangeError::with_diagnostic(
                    FileChangeErrorCode::Failed,
                    error,
                ))
            })?,
        staged_transaction_id: None,
        conversation_id: context.conversation_id()?.to_string(),
        project_id: context.project_id().map(ToString::to_string),
        run_id: context.run_id()?.to_string(),
        staged_transaction_revision: None,
        canonical_target: target.absolute_path().to_string_lossy().into_owned(),
        base_content: plan.base_content.clone(),
        target_content: plan.target_content.clone(),
        delete_journal,
        receipt: None,
        permission_revision: context.file_change_permission_revision().to_string(),
        tool_set_revision: context.file_change_tool_set_revision().to_string(),
        provider_wire_revision: context.file_change_provider_wire_revision().to_string(),
    };
    execution.validate().map_err(file_change_agent_error)?;
    if let Some(observation_id) = public_observation_id {
        context
            .file_observations()
            .claim(
                observation_id,
                context.conversation_id()?,
                context.run_id()?,
                target.absolute_path(),
            )
            .map_err(|error| file_change_agent_error_for_path(error, &file_path))?;
    }
    let target_content = plan.target_content.as_deref().unwrap_or_default();
    Ok(AgentFileChangeProposal {
        schema_version: AGENT_FILE_CHANGE_PROTOCOL_SCHEMA_VERSION,
        id: call.id.clone(),
        transaction_id: execution.transaction.id.clone(),
        operation: patch_operation(plan.operation),
        update_strategy: None,
        file_path: plan.file_path,
        inline_diff: Some(AgentGitDiffSnapshot {
            patch: plan.diff,
            truncated: false,
        }),
        base_revision: state_revision(&plan.base).map(str::to_string),
        summary: sanitize_summary(summary),
        additions: plan.additions,
        deletions: plan.deletions,
        line_count: target_content.lines().count() as u64,
        byte_count: target_content.len() as u64,
        approval_status: AgentApprovalStatus::Required,
        execution: Box::new(execution),
    })
}

pub(crate) fn apply_patch_request(value: &Value) -> Option<&Map<String, Value>> {
    value.as_object()?.get("request")?.as_object()
}

pub(crate) fn apply_patch_action(value: &Value) -> Option<&str> {
    apply_patch_request(value)?.get("action")?.as_str()
}

pub(crate) fn apply_patch_wire_is_valid(value: &Value) -> bool {
    validate_wire_shape(value).is_ok()
}

pub(super) fn copy_bounded_enum(
    source: &Map<String, Value>,
    target: &mut Map<String, Value>,
    key: &str,
    allowed: &[&str],
) {
    if let Some(value) = source
        .get(key)
        .and_then(Value::as_str)
        .filter(|value| allowed.contains(value))
    {
        target.insert(key.to_string(), json!(value));
    }
}

pub(super) fn copy_bounded_string(
    source: &Map<String, Value>,
    target: &mut Map<String, Value>,
    key: &str,
    max_chars: usize,
) {
    if let Some(value) = source
        .get(key)
        .and_then(Value::as_str)
        .filter(|value| value.chars().count() <= max_chars)
    {
        target.insert(key.to_string(), json!(value));
    }
}

pub(super) fn copy_u64(source: &Map<String, Value>, target: &mut Map<String, Value>, key: &str) {
    if let Some(value) = source.get(key).and_then(Value::as_u64) {
        target.insert(key.to_string(), json!(value));
    }
}

pub(super) fn validate_wire_shape(value: &Value) -> Result<(), FileChangeError> {
    let root = value
        .as_object()
        .ok_or_else(|| FileChangeError::new(FileChangeErrorCode::InvalidArguments))?;
    reject_unknown_keys(root, &["request"])?;
    if root.len() != 1 || root.get("request").is_none_or(Value::is_null) {
        return Err(FileChangeError::new(FileChangeErrorCode::InvalidArguments));
    }
    let object = root
        .get("request")
        .and_then(Value::as_object)
        .ok_or_else(|| FileChangeError::new(FileChangeErrorCode::InvalidArguments))?;
    validate_request_wire_shape(object)
}

fn validate_request_wire_shape(object: &Map<String, Value>) -> Result<(), FileChangeError> {
    reject_unknown_keys(
        object,
        &[
            "action",
            "operation",
            "filePath",
            "observationId",
            "strategy",
            "transactionId",
            "index",
            "expectedDraftRevision",
            "content",
            "edits",
            "summary",
        ],
    )?;
    if object.is_empty() || object.values().any(Value::is_null) {
        return Err(FileChangeError::new(FileChangeErrorCode::InvalidArguments));
    }
    let action = object
        .get("action")
        .and_then(Value::as_str)
        .ok_or_else(|| FileChangeError::new(FileChangeErrorCode::InvalidArguments))?;
    let exact = |required: &[&str], optional: &[&str]| {
        required.iter().all(|key| object.contains_key(*key))
            && object
                .keys()
                .all(|key| required.contains(&key.as_str()) || optional.contains(&key.as_str()))
    };
    let valid = match action {
        "apply" => {
            let operation = object.get("operation").and_then(Value::as_str);
            match operation {
                Some("create") => exact(
                    &["action", "operation", "filePath", "content"],
                    &["summary"],
                ),
                Some("update") => {
                    let common = ["action", "operation", "filePath", "observationId"];
                    (exact(&common, &["content", "summary"])
                        || exact(&common, &["edits", "summary"]))
                        && (object.contains_key("content") ^ object.contains_key("edits"))
                }
                Some("delete") => exact(
                    &["action", "operation", "filePath", "observationId"],
                    &["summary"],
                ),
                _ => false,
            }
        }
        "begin" => match object.get("operation").and_then(Value::as_str) {
            Some("create") => exact(&["action", "operation", "filePath"], &[]),
            Some("update") => {
                exact(
                    &[
                        "action",
                        "operation",
                        "filePath",
                        "observationId",
                        "strategy",
                    ],
                    &[],
                ) && matches!(
                    object.get("strategy").and_then(Value::as_str),
                    Some("modify" | "rewrite")
                )
            }
            _ => false,
        },
        "append" => exact(
            &[
                "action",
                "transactionId",
                "index",
                "expectedDraftRevision",
                "content",
            ],
            &[],
        ),
        "edit" => exact(
            &[
                "action",
                "transactionId",
                "index",
                "expectedDraftRevision",
                "edits",
            ],
            &[],
        ),
        "commit" => exact(
            &["action", "transactionId", "expectedDraftRevision"],
            &["summary"],
        ),
        "status" | "abort" => exact(&["action", "transactionId"], &[]),
        _ => false,
    };
    if !valid {
        return Err(FileChangeError::new(
            FileChangeErrorCode::IllegalFieldCombination,
        ));
    }
    if object
        .get("summary")
        .and_then(Value::as_str)
        .is_some_and(|summary| summary.chars().count() > MAX_SUMMARY_CHARS)
    {
        return Err(FileChangeError::new(FileChangeErrorCode::InvalidArguments));
    }
    if let Some(content) = object.get("content").and_then(Value::as_str) {
        let content_too_large = match action {
            "apply" => content.len() > MAX_INLINE_CONTENT_BYTES,
            "append" => content.len() > file_change_staged::MAX_STAGED_CHUNK_BYTES,
            _ => false,
        };
        if content_too_large {
            return Err(FileChangeError::new(FileChangeErrorCode::ContentTooLarge));
        }
        if action == "append" && content.is_empty() {
            return Err(FileChangeError::new(FileChangeErrorCode::InvalidArguments));
        }
    }
    for key in [
        "action",
        "operation",
        "filePath",
        "observationId",
        "strategy",
        "transactionId",
        "content",
        "summary",
    ] {
        if let Some(value) = object.get(key) {
            if !value.is_string()
                || matches!(key, "filePath" | "observationId" | "transactionId")
                    && value.as_str().is_some_and(str::is_empty)
            {
                return Err(FileChangeError::new(FileChangeErrorCode::InvalidArguments));
            }
        }
    }
    for key in ["index", "expectedDraftRevision"] {
        if object
            .get(key)
            .is_some_and(|value| value.as_u64().is_none())
        {
            return Err(FileChangeError::new(FileChangeErrorCode::InvalidArguments));
        }
    }
    if let Some(edits) = object.get("edits") {
        validate_edit_wire_shapes(edits)?;
    }
    Ok(())
}

fn validate_edit_wire_shapes(value: &Value) -> Result<(), FileChangeError> {
    let edits = value
        .as_array()
        .filter(|edits| !edits.is_empty() && edits.len() <= 128)
        .ok_or_else(|| FileChangeError::new(FileChangeErrorCode::InvalidArguments))?;
    for edit in edits {
        let object = edit
            .as_object()
            .ok_or_else(|| FileChangeError::new(FileChangeErrorCode::InvalidArguments))?;
        reject_unknown_keys(
            object,
            &["kind", "oldText", "newText", "anchor", "text", "replaceAll"],
        )?;
        if object.values().any(Value::is_null) {
            return Err(FileChangeError::new(FileChangeErrorCode::InvalidArguments));
        }
        let kind = object
            .get("kind")
            .and_then(Value::as_str)
            .ok_or_else(|| FileChangeError::new(FileChangeErrorCode::InvalidArguments))?;
        let exact = |required: &[&str], optional: &[&str]| {
            required.iter().all(|key| object.contains_key(*key))
                && object
                    .keys()
                    .all(|key| required.contains(&key.as_str()) || optional.contains(&key.as_str()))
        };
        let valid = match kind {
            "replace" => {
                exact(&["kind", "oldText", "newText"], &["replaceAll"])
                    && object
                        .get("oldText")
                        .and_then(Value::as_str)
                        .is_some_and(|value| !value.is_empty())
                    && object.get("newText").is_some_and(Value::is_string)
                    && object.get("replaceAll").is_none_or(Value::is_boolean)
            }
            "insert_before" | "insert_after" => {
                exact(&["kind", "anchor", "text"], &[])
                    && object
                        .get("anchor")
                        .and_then(Value::as_str)
                        .is_some_and(|value| !value.is_empty())
                    && object.get("text").is_some_and(Value::is_string)
            }
            "append" | "prepend" => {
                exact(&["kind", "text"], &[]) && object.get("text").is_some_and(Value::is_string)
            }
            _ => false,
        };
        if !valid {
            return Err(FileChangeError::new(
                FileChangeErrorCode::IllegalFieldCombination,
            ));
        }
    }
    Ok(())
}

fn reject_unknown_keys(
    object: &Map<String, Value>,
    allowed: &[&str],
) -> Result<(), FileChangeError> {
    if object.keys().any(|key| !allowed.contains(&key.as_str())) {
        Err(FileChangeError::new(FileChangeErrorCode::UnknownField))
    } else {
        Ok(())
    }
}
