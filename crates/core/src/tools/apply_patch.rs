use super::file_change_stream::FileChangeInputStreamObserver;
use super::{
    file_change_staged, AgentTool, AgentToolPermissionPolicy, FileChangeToolAccess,
    ToolExecutionContext, ToolInputStreamObserver,
};
use crate::file_change::{
    BoundParent, FileChangeBase, FileChangeCommitter, FileChangeContentState,
    FileChangeDirectBinding, FileChangeEdit, FileChangeError, FileChangeErrorCode,
    FileChangeMutation, FileChangeOperation, FileChangeOutcome, FileChangePathPolicy,
    FileChangePlanRequest, FileChangePlanner, FileChangeProposal, FileChangeStatus,
    FileChangeTransaction, FileObservationIdentity, FileObservationState,
    FILE_CHANGE_SCHEMA_VERSION,
};
use crate::protocol::{
    AgentApprovalStatus, AgentError, AgentFileChangeOperation, AgentFileChangeProposal,
    AgentGitDiffSnapshot, AgentProposedAction, AgentResult, AgentToolCall, AgentToolDefinition,
    AgentToolResult, AgentToolSafety, AgentWritePermission,
    AGENT_FILE_CHANGE_PROTOCOL_SCHEMA_VERSION,
};
use crate::revision::content_revision;
use serde::Deserialize;
use serde_json::{json, Map, Value};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use uuid::Uuid;

const MAX_SUMMARY_CHARS: usize = 2_000;
const MAX_EDIT_CONTENT_BYTES: usize = 240_000;
const MAX_INLINE_CONTENT_BYTES: usize = 32 * 1024;

pub(super) struct ApplyPatchTool;

impl AgentTool for ApplyPatchTool {
    fn exposure(&self) -> super::AgentToolExposure {
        super::AgentToolExposure::Stable
    }

    fn definition(&self) -> AgentToolDefinition {
        AgentToolDefinition {
            name: "apply_patch".to_string(),
            description: "Create or update UTF-8 text files through one strict FileChange protocol. Before action=apply or action=begin, use read_file on the exact target path and copy its observationId. Use action=apply for one short complete create/update/delete. For long content use action=begin, then append/edit with the returned transactionId, nextIndex, and draftRevision, and finally action=commit; settle an unfinished transaction with commit or abort before user-visible narration. create is always no-clobber. Staged delete is not supported. Without a workspace, filePath must be an authorized absolute path or @home, @desktop, @documents, or @downloads. Never use run_command, redirection, or scripts to bypass file-change approval."
                .to_string(),
            input_schema: patch_input_schema(),
            safety: AgentToolSafety::RequiresApproval,
            requires_workspace: false,
            requires_approval: true,
            approval_mode: crate::protocol::AgentToolApprovalMode::Dynamic,
        }
    }

    fn execute(&self, context: &ToolExecutionContext, args: Value) -> AgentResult<Value> {
        validate_wire_shape(&args).map_err(file_change_agent_error)?;
        let source_args_digest = crate::file_change::proposal_digest(&args).map_err(|error| {
            file_change_agent_error(FileChangeError::with_diagnostic(
                FileChangeErrorCode::Failed,
                error.to_string(),
            ))
        })?;
        let args = parse_args(args)?;
        match args {
            ApplyPatchArgs::Begin {
                operation,
                file_path,
                observation_id,
                strategy,
            } => file_change_staged::begin(
                context,
                file_change_staged::StagedSource::new("apply_patch", source_args_digest),
                operation,
                strategy,
                file_path,
                observation_id,
                None,
            ),
            ApplyPatchArgs::Append {
                transaction_id,
                index,
                expected_draft_revision,
                content,
            } => file_change_staged::append(
                context,
                "apply_patch",
                transaction_id,
                index,
                expected_draft_revision,
                content,
                source_args_digest,
            ),
            ApplyPatchArgs::Edit {
                transaction_id,
                index,
                expected_draft_revision,
                edits,
            } => file_change_staged::edit(
                context,
                "apply_patch",
                transaction_id,
                index,
                expected_draft_revision,
                edits
                    .into_iter()
                    .map(domain_edit)
                    .collect::<Result<_, _>>()
                    .map_err(file_change_agent_error)?,
                source_args_digest,
            ),
            ApplyPatchArgs::Status { transaction_id } => {
                file_change_staged::status(context, "apply_patch", transaction_id)
            }
            ApplyPatchArgs::Abort { transaction_id } => {
                file_change_staged::abort(context, "apply_patch", transaction_id)
            }
            ApplyPatchArgs::Apply { .. } | ApplyPatchArgs::Commit { .. } => Err(AgentError::new(
                "apply_patch apply/commit 需要用户审批和 Host 执行层。",
            )),
        }
    }

    fn permission_policy(&self) -> AgentToolPermissionPolicy {
        // status/abort remain available after write authority is tightened so a dirty transaction
        // can always be inspected or safely settled. Mutating actions enforce current authority.
        AgentToolPermissionPolicy::FileChange(FileChangeToolAccess::ReadWrite)
    }

    fn proposed_action(
        &self,
        context: &ToolExecutionContext,
        call: &AgentToolCall,
    ) -> AgentResult<AgentProposedAction> {
        validate_wire_shape(&call.args).map_err(file_change_agent_error)?;
        match parse_args(call.args.clone())? {
            args @ ApplyPatchArgs::Apply { .. } => Ok(AgentProposedAction::FileChange {
                file_change: direct_proposal_from_args(context, call, args)?,
            }),
            ApplyPatchArgs::Commit {
                transaction_id,
                expected_draft_revision,
                summary,
            } => Ok(AgentProposedAction::FileChange {
                file_change: file_change_staged::commit(
                    context,
                    call,
                    "apply_patch",
                    transaction_id,
                    expected_draft_revision,
                    summary,
                )?,
            }),
            _ => Err(AgentError::new(
                "只有 apply_patch action=apply/commit 可以产生审批提案。",
            )),
        }
    }

    fn requires_approval_for_call(&self, args: &Value) -> bool {
        matches!(
            args.get("action").and_then(Value::as_str),
            Some("apply" | "commit")
        )
    }

    fn input_stream_observer(
        &self,
        context: ToolExecutionContext,
    ) -> Option<Box<dyn ToolInputStreamObserver>> {
        Some(Box::new(FileChangeInputStreamObserver::apply_patch(
            context,
        )))
    }

    fn event_call_projection(&self, call: &AgentToolCall) -> AgentToolCall {
        let mut projection = call.clone();
        let Some(args) = projection.args.as_object_mut() else {
            return projection;
        };
        if let Some(content) = args.remove("content").and_then(|value| {
            value.as_str().map(|content| {
                (
                    content.len(),
                    crate::file_change::content_digest(content.as_bytes()),
                )
            })
        }) {
            args.insert("contentBytes".to_string(), json!(content.0));
            args.insert("contentDigest".to_string(), json!(content.1));
        }
        if let Some(edits) = args.remove("edits").and_then(|value| {
            value.as_array().map(|edits| {
                let digest = crate::file_change::proposal_digest(&Value::Array(edits.clone())).ok();
                (edits.len(), digest)
            })
        }) {
            args.insert("editCount".to_string(), json!(edits.0));
            if let Some(digest) = edits.1 {
                args.insert("editsDigest".to_string(), json!(digest));
            }
        }
        args.remove("observationId");
        projection
    }

    fn event_projection(&self, result: &AgentToolResult) -> AgentToolResult {
        file_change_staged::public_result_projection(result)
    }

    fn checkpoint_projection(&self, result: &AgentToolResult) -> AgentToolResult {
        file_change_staged::public_result_projection(result)
    }
}

fn patch_input_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "action": {
                "type": "string",
                "enum": ["apply", "begin", "append", "edit", "commit", "status", "abort"],
                "description": "Direct apply or the persistent Staged transaction lifecycle."
            },
            "operation": {
                "type": "string",
                "enum": ["create", "update", "delete"],
                "description": "Read the exact target with read_file first and pass its observationId. create requires a missing observation and content; update requires an existing observation and exactly one of content or edits; delete requires an existing observation and no content or edits."
            },
            "filePath": {
                "type": "string",
                "description": "The exact path passed to read_file: workspace-relative, an authorized absolute local path, or a supported system alias."
            },
            "observationId": {
                "type": "string",
                "description": "Opaque fobs_ identifier returned by read_file for this exact target."
            },
            "strategy": {
                "type": "string",
                "enum": ["modify", "rewrite"],
                "description": "Required only by begin/update. modify starts from observed content; rewrite starts empty."
            },
            "transactionId": {
                "type": "string",
                "description": "Opaque transaction id returned by begin."
            },
            "index": {
                "type": "integer",
                "minimum": 0,
                "description": "Exact shared mutation index returned as nextIndex."
            },
            "expectedDraftRevision": {
                "type": "integer",
                "minimum": 0,
                "description": "Exact draftRevision returned by the preceding successful mutation."
            },
            "content": {
                "type": "string",
                "maxLength": 1048576,
                "description": "Complete short Direct content or one bounded Staged append chunk."
            },
            "edits": {
                "type": "array",
                "minItems": 1,
                "maxItems": 128,
                "description": "Ordered exact edits for update. replace requires one exact match unless replaceAll is explicitly true.",
                "items": {
                    "type": "object",
                    "properties": {
                        "kind": { "type": "string", "enum": ["replace", "insert_before", "insert_after", "append", "prepend"] },
                        "oldText": { "type": "string" },
                        "newText": { "type": "string" },
                        "anchor": { "type": "string" },
                        "text": { "type": "string" },
                        "replaceAll": { "type": "boolean" }
                    },
                    "required": ["kind"],
                    "additionalProperties": false
                }
            },
            "summary": { "type": "string", "description": "Short human-readable summary." }
        },
        "required": ["action"],
        "additionalProperties": false
    })
}

#[derive(Debug, Deserialize)]
#[serde(
    tag = "action",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
enum ApplyPatchArgs {
    Apply {
        operation: FileChangeOperation,
        file_path: String,
        observation_id: String,
        content: Option<String>,
        edits: Option<Vec<StructuredTextEdit>>,
        summary: Option<String>,
    },
    Begin {
        operation: FileChangeOperation,
        file_path: String,
        observation_id: String,
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
    kind: TextEditKind,
    old_text: Option<String>,
    new_text: Option<String>,
    anchor: Option<String>,
    text: Option<String>,
    replace_all: Option<bool>,
}

#[derive(Debug, Deserialize, Clone, Copy)]
#[serde(rename_all = "snake_case")]
enum TextEditKind {
    Replace,
    InsertBefore,
    InsertAfter,
    Append,
    Prepend,
}

fn parse_args(value: Value) -> AgentResult<ApplyPatchArgs> {
    serde_json::from_value(value).map_err(|_| {
        file_change_agent_error(FileChangeError::new(FileChangeErrorCode::InvalidArguments))
    })
}

#[cfg(test)]
fn direct_proposal_from_call(
    context: &ToolExecutionContext,
    call: &AgentToolCall,
) -> AgentResult<AgentFileChangeProposal> {
    validate_wire_shape(&call.args).map_err(file_change_agent_error)?;
    let args = parse_args(call.args.clone())?;
    direct_proposal_from_args(context, call, args)
}

fn direct_proposal_from_args(
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
    let observation = context
        .file_observations()
        .validate(
            &observation_id,
            context.conversation_id()?,
            context.run_id()?,
            target.absolute_path(),
        )
        .map_err(file_change_agent_error)?;
    validate_observation_operation(operation, observation.state())
        .map_err(file_change_agent_error)?;
    let frozen_base =
        freeze_observed_base(context, &target, &observation).map_err(file_change_agent_error)?;
    let mutation =
        mutation_from_args(operation, content, edits).map_err(file_change_agent_error)?;
    let plan = FileChangePlanner
        .plan(FileChangePlanRequest {
            operation,
            file_path: target.display_path(),
            base: frozen_base.as_plan_base(),
            mutation,
        })
        .map_err(file_change_agent_error)?;
    if plan
        .target_content
        .as_ref()
        .is_some_and(|content| content.len() > MAX_EDIT_CONTENT_BYTES)
    {
        return Err(file_change_agent_error(FileChangeError::new(
            FileChangeErrorCode::ContentTooLarge,
        )));
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
        schema_version: FILE_CHANGE_SCHEMA_VERSION,
        transaction,
        proposal,
        observation_id,
        observation: observation.checkpoint(),
        source_tool_name: "apply_patch".to_string(),
        source_call_id: call.id.clone(),
        source_args_digest: crate::file_change::proposal_digest(&call.args).map_err(|error| {
            file_change_agent_error(FileChangeError::with_diagnostic(
                FileChangeErrorCode::Failed,
                error.to_string(),
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
    context
        .file_observations()
        .claim(
            &execution.observation_id,
            context.conversation_id()?,
            context.run_id()?,
            target.absolute_path(),
        )
        .map_err(file_change_agent_error)?;
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

fn validate_wire_shape(value: &Value) -> Result<(), FileChangeError> {
    let object = value
        .as_object()
        .ok_or_else(|| FileChangeError::new(FileChangeErrorCode::InvalidArguments))?;
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
                    &[
                        "action",
                        "operation",
                        "filePath",
                        "observationId",
                        "content",
                    ],
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
            Some("create") => exact(&["action", "operation", "filePath", "observationId"], &[]),
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
            if !value.is_string() {
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
            "replace" => exact(&["kind", "oldText", "newText"], &["replaceAll"]),
            "insert_before" | "insert_after" => exact(&["kind", "anchor", "text"], &[]),
            "append" | "prepend" => exact(&["kind", "text"], &[]),
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

fn mutation_from_args(
    operation: FileChangeOperation,
    content: Option<String>,
    edits: Option<Vec<StructuredTextEdit>>,
) -> Result<FileChangeMutation, FileChangeError> {
    match operation {
        FileChangeOperation::Create | FileChangeOperation::Update if content.is_some() => {
            let content = content.expect("checked content presence");
            if content.len() > MAX_INLINE_CONTENT_BYTES {
                return Err(FileChangeError::new(FileChangeErrorCode::ContentTooLarge));
            }
            Ok(FileChangeMutation::Complete(content))
        }
        FileChangeOperation::Update => Ok(FileChangeMutation::Edits(
            edits
                .ok_or_else(|| FileChangeError::new(FileChangeErrorCode::InvalidArguments))?
                .into_iter()
                .map(domain_edit)
                .collect::<Result<Vec<_>, _>>()?,
        )),
        FileChangeOperation::Delete => Ok(FileChangeMutation::Delete),
        FileChangeOperation::Create => Err(FileChangeError::new(
            FileChangeErrorCode::IllegalFieldCombination,
        )),
    }
}

pub(super) fn domain_edit(edit: StructuredTextEdit) -> Result<FileChangeEdit, FileChangeError> {
    let invalid = || FileChangeError::new(FileChangeErrorCode::IllegalFieldCombination);
    match edit.kind {
        TextEditKind::Replace => {
            if edit.anchor.is_some() || edit.text.is_some() {
                return Err(invalid());
            }
            Ok(FileChangeEdit::Replace {
                old_text: edit.old_text.ok_or_else(invalid)?,
                new_text: edit.new_text.ok_or_else(invalid)?,
                replace_all: edit.replace_all.unwrap_or(false),
            })
        }
        TextEditKind::InsertBefore | TextEditKind::InsertAfter => {
            if edit.old_text.is_some() || edit.new_text.is_some() || edit.replace_all.is_some() {
                return Err(invalid());
            }
            let anchor = edit.anchor.ok_or_else(invalid)?;
            let text = edit.text.ok_or_else(invalid)?;
            if matches!(edit.kind, TextEditKind::InsertBefore) {
                Ok(FileChangeEdit::InsertBefore { anchor, text })
            } else {
                Ok(FileChangeEdit::InsertAfter { anchor, text })
            }
        }
        TextEditKind::Append | TextEditKind::Prepend => {
            if edit.old_text.is_some()
                || edit.new_text.is_some()
                || edit.anchor.is_some()
                || edit.replace_all.is_some()
            {
                return Err(invalid());
            }
            let text = edit.text.ok_or_else(invalid)?;
            if matches!(edit.kind, TextEditKind::Append) {
                Ok(FileChangeEdit::Append { text })
            } else {
                Ok(FileChangeEdit::Prepend { text })
            }
        }
    }
}

pub(super) fn validate_observation_operation(
    operation: FileChangeOperation,
    state: &FileObservationState,
) -> Result<(), FileChangeError> {
    match (operation, state) {
        (FileChangeOperation::Create, FileObservationState::Missing)
        | (
            FileChangeOperation::Update | FileChangeOperation::Delete,
            FileObservationState::Existing { .. },
        ) => Ok(()),
        (FileChangeOperation::Create, FileObservationState::Existing { .. }) => {
            Err(FileChangeError::new(FileChangeErrorCode::FileExists))
        }
        (
            FileChangeOperation::Update | FileChangeOperation::Delete,
            FileObservationState::Missing,
        ) => Err(FileChangeError::new(FileChangeErrorCode::FileMissing)),
    }
}

#[derive(Debug)]
pub(super) struct FrozenBase {
    pub(super) content: Option<String>,
    pub(super) revision: Option<String>,
}

impl FrozenBase {
    fn as_plan_base(&self) -> FileChangeBase<'_> {
        match (&self.content, &self.revision) {
            (None, None) => FileChangeBase::Missing,
            (Some(content), Some(revision)) => FileChangeBase::Existing { content, revision },
            _ => unreachable!("frozen base content and revision are total"),
        }
    }
}

fn freeze_observed_base(
    context: &ToolExecutionContext,
    target: &crate::file_change::ResolvedFileChangeTarget,
    observation: &crate::file_change::FileObservation,
) -> Result<FrozenBase, FileChangeError> {
    freeze_observed_base_with_limit_and_hook(
        context,
        target,
        observation,
        MAX_EDIT_CONTENT_BYTES,
        || {},
    )
}

pub(super) fn freeze_staged_observed_base(
    context: &ToolExecutionContext,
    target: &crate::file_change::ResolvedFileChangeTarget,
    observation: &crate::file_change::FileObservation,
    max_bytes: usize,
) -> Result<FrozenBase, FileChangeError> {
    freeze_observed_base_with_limit_and_hook(context, target, observation, max_bytes, || {})
}

#[cfg(test)]
fn freeze_observed_base_with_hook(
    context: &ToolExecutionContext,
    target: &crate::file_change::ResolvedFileChangeTarget,
    observation: &crate::file_change::FileObservation,
    after_parent_bind: impl FnOnce(),
) -> Result<FrozenBase, FileChangeError> {
    freeze_observed_base_with_limit_and_hook(
        context,
        target,
        observation,
        MAX_EDIT_CONTENT_BYTES,
        after_parent_bind,
    )
}

fn freeze_observed_base_with_limit_and_hook(
    context: &ToolExecutionContext,
    target: &crate::file_change::ResolvedFileChangeTarget,
    observation: &crate::file_change::FileObservation,
    max_bytes: usize,
    after_parent_bind: impl FnOnce(),
) -> Result<FrozenBase, FileChangeError> {
    context.check_cancelled().map_err(|error| {
        FileChangeError::with_diagnostic(FileChangeErrorCode::Failed, error.to_string())
    })?;
    let parent = BoundParent::open(target)?;
    after_parent_bind();
    let current = parent.read_optional(target.absolute_path())?;
    // The held descriptor prevents redirection; this pathname revalidation only proves that the
    // authorized target still names that descriptor before a proposal can expose its Diff.
    parent.revalidate()?;
    let parent_metadata = parent.parent_metadata()?;
    if !observation.parent_identity_matches(&parent_metadata) {
        return Err(FileChangeError::new(FileChangeErrorCode::ObservationStale));
    }
    match (observation.state(), current) {
        (FileObservationState::Missing, None) => Ok(FrozenBase {
            content: None,
            revision: None,
        }),
        (FileObservationState::Missing, Some(_)) => {
            Err(FileChangeError::new(FileChangeErrorCode::FileExists))
        }
        (FileObservationState::Existing { .. }, None) => {
            Err(FileChangeError::new(FileChangeErrorCode::FileMissing))
        }
        (FileObservationState::Existing { revision, identity }, Some(current)) => {
            if FileObservationIdentity::from_metadata(&current.metadata) != *identity {
                return Err(FileChangeError::new(FileChangeErrorCode::ObservationStale));
            }
            if current.bytes.len() > max_bytes {
                return Err(FileChangeError::new(FileChangeErrorCode::ContentTooLarge));
            }
            let content = String::from_utf8(current.bytes).map_err(|error| {
                FileChangeError::with_diagnostic(
                    FileChangeErrorCode::UnsupportedFileType,
                    error.to_string(),
                )
            })?;
            let actual_revision = content_revision(content.as_bytes());
            if actual_revision != *revision {
                return Err(FileChangeError::new(FileChangeErrorCode::ObservationStale));
            }
            Ok(FrozenBase {
                content: Some(content),
                revision: Some(actual_revision),
            })
        }
    }
}

pub(super) fn patch_operation(operation: FileChangeOperation) -> AgentFileChangeOperation {
    match operation {
        FileChangeOperation::Create => AgentFileChangeOperation::Create,
        FileChangeOperation::Update => AgentFileChangeOperation::Update,
        FileChangeOperation::Delete => AgentFileChangeOperation::Delete,
    }
}

pub(super) fn state_revision(state: &FileChangeContentState) -> Option<&str> {
    match state {
        FileChangeContentState::Missing => None,
        FileChangeContentState::Present { revision, .. } => Some(revision),
    }
}

pub(super) fn sanitize_summary(summary: Option<String>) -> Option<String> {
    summary
        .map(|summary| summary.trim().chars().take(MAX_SUMMARY_CHARS).collect())
        .filter(|summary: &String| !summary.is_empty())
}

pub(super) fn file_change_agent_error(error: FileChangeError) -> AgentError {
    let code = serde_json::to_value(error.code())
        .ok()
        .and_then(|value| value.as_str().map(str::to_string))
        .unwrap_or_else(|| "failed".to_string());
    AgentError::structured(
        format!("agent.apply_patch.{code}"),
        error.to_string(),
        json!({
            "type": "file_change",
            "code": error.failure().code,
            "category": error.failure().category,
            "message": error.failure().message,
            "recovery": error.failure().recovery,
            "continueWith": if matches!(
                error.code(),
                FileChangeErrorCode::ObservationRequired
                    | FileChangeErrorCode::ObservationExpired
                    | FileChangeErrorCode::ObservationOwnerMismatch
                    | FileChangeErrorCode::ObservationPathMismatch
                    | FileChangeErrorCode::ObservationStale
                    | FileChangeErrorCode::RevisionConflict
            ) {
                json!({"tool": "read_file", "args": {"path": "<exact filePath>"}})
            } else {
                Value::Null
            }
        }),
    )
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or(Duration::ZERO)
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{
        AgentCommandPermission, AgentPermissions, AgentReadPermission, AgentRunContext,
        AgentWorkspaceContext,
    };
    use crate::tools::ToolRegistry;
    use serde_json::json;
    use std::fs;
    #[cfg(unix)]
    use std::os::unix::fs::symlink;
    use std::sync::atomic::{AtomicU64, Ordering};

    static TEST_COUNTER: AtomicU64 = AtomicU64::new(1);

    #[test]
    fn unified_schema_is_portable_strict_and_has_no_raw_patch_or_revision() {
        let schema = patch_input_schema();
        assert_eq!(schema["type"], "object");
        assert_eq!(schema["additionalProperties"], false);
        assert_eq!(schema["required"], json!(["action"]));
        assert!(schema.get("oneOf").is_none());
        assert!(schema.get("anyOf").is_none());
        assert!(schema.get("allOf").is_none());
        let properties = schema["properties"].as_object().unwrap();
        assert!(!properties.contains_key("patch"));
        assert!(!properties.contains_key("expectedRevision"));
        assert!(properties.contains_key("transactionId"));
        assert!(properties.contains_key("expectedDraftRevision"));
        assert_eq!(properties["edits"]["items"]["additionalProperties"], false);
    }

    #[test]
    fn staged_wire_enforces_every_exact_action_matrix() {
        let valid = [
            json!({"action":"begin","operation":"create","filePath":"a.txt","observationId":"fobs_x"}),
            json!({"action":"begin","operation":"update","filePath":"a.txt","observationId":"fobs_x","strategy":"modify"}),
            json!({"action":"begin","operation":"update","filePath":"a.txt","observationId":"fobs_x","strategy":"rewrite"}),
            json!({"action":"append","transactionId":"file-change-staged-v1:x","index":0,"expectedDraftRevision":0,"content":"x"}),
            json!({"action":"edit","transactionId":"file-change-staged-v1:x","index":1,"expectedDraftRevision":1,"edits":[{"kind":"append","text":"y"}]}),
            json!({"action":"commit","transactionId":"file-change-staged-v1:x","expectedDraftRevision":2,"summary":"done"}),
            json!({"action":"status","transactionId":"file-change-staged-v1:x"}),
            json!({"action":"abort","transactionId":"file-change-staged-v1:x"}),
        ];
        for value in valid {
            validate_wire_shape(&value).unwrap_or_else(|error| panic!("rejected {value}: {error}"));
        }

        let invalid = [
            json!({"action":"begin","operation":"delete","filePath":"a.txt","observationId":"fobs_x"}),
            json!({"action":"begin","operation":"create","filePath":"a.txt","observationId":"fobs_x","strategy":"rewrite"}),
            json!({"action":"begin","operation":"update","filePath":"a.txt","observationId":"fobs_x"}),
            json!({"action":"append","transactionId":"x","index":0,"expectedDraftRevision":null,"content":"x"}),
            json!({"action":"edit","transactionId":"x","index":0,"expectedDraftRevision":0,"edits":[],"content":"x"}),
            json!({"action":"commit","transactionId":"x","expected_draft_revision":0}),
            json!({"action":"status","transactionId":"x","summary":"extra"}),
            json!({"action":"abort","transactionId":"x","extra":true}),
        ];
        for value in invalid {
            assert!(validate_wire_shape(&value).is_err(), "accepted {value}");
        }
    }

    #[test]
    fn strict_wire_rejects_missing_null_unknown_snake_case_raw_patch_and_bad_combinations() {
        let valid = json!({
            "action": "apply", "operation": "delete", "filePath": "a.txt",
            "observationId": "fobs_example"
        });
        let invalid = [
            json!({"operation":"delete","filePath":"a.txt","observationId":"fobs_example"}),
            json!({"action":"apply","operation":"delete","filePath":"a.txt","observationId":null}),
            json!({"action":"apply","operation":"delete","filePath":"a.txt","observationId":"fobs_example","extra":true}),
            json!({"action":"apply","operation":"delete","file_path":"a.txt","observationId":"fobs_example"}),
            json!({"action":"apply","operation":"update","filePath":"a.txt","observationId":"fobs_example","patch":"@@"}),
            json!({"action":"apply","operation":"update","filePath":"a.txt","observationId":"fobs_example","content":"x","edits":[]}),
            json!({"action":"apply","operation":"update","filePath":"a.txt","observationId":"fobs_example","edits":[{"kind":"replace","oldText":"a","newText":"b","replaceAll":null}]}),
            json!({"action":"apply","operation":"update","filePath":"a.txt","observationId":"fobs_example","edits":[{"kind":"append","text":"x","extra":true}]}),
            json!({"action":"apply","operation":"update","filePath":"a.txt","observationId":"fobs_example","edits":[{"kind":"insert_before","text":"x"}]}),
            json!({"action":"apply","operation":"delete","filePath":"a.txt","observationId":"fobs_example","content":"x"}),
        ];
        assert!(validate_wire_shape(&valid).is_ok());
        for value in invalid {
            assert!(validate_wire_shape(&value).is_err(), "accepted {value}");
        }
    }

    #[test]
    fn registry_entry_returns_stable_typed_errors_for_invalid_direct_json() {
        let workspace = TestWorkspace::new();
        let context = workspace.context();
        let registry = ToolRegistry::defaults_with_search(None);
        let cases = [
            (
                json!({"action":1,"operation":"delete","filePath":"a.txt","observationId":"fobs_x"}),
                "agent.apply_patch.invalid_arguments",
            ),
            (
                json!({"action":"apply","operation":"delete","filePath":"","observationId":"fobs_x"}),
                "agent.apply_patch.invalid_arguments",
            ),
            (
                json!({"action":"apply","operation":"delete","filePath":"a.txt","observationId":""}),
                "agent.apply_patch.observation_required",
            ),
            (
                json!({"action":"apply","operation":"update","filePath":"a.txt","observationId":"fobs_x","patch":"@@"}),
                "agent.apply_patch.unknown_field",
            ),
            (
                json!({"action":"apply","operation":"delete","filePath":"a.txt","observationId":"fobs_x","summary":null}),
                "agent.apply_patch.invalid_arguments",
            ),
        ];

        for (args, expected_code) in cases {
            let call = AgentToolCall {
                id: format!("invalid-{}", Uuid::new_v4()),
                tool: "apply_patch".to_string(),
                args,
                approval_status: AgentApprovalStatus::Required,
                reason: None,
            };
            let error = registry.proposed_action(&context, &call).unwrap_err();
            assert_eq!(error.code(), Some(expected_code));
            assert!(!error.to_string().contains("serde"));
            assert!(!error.to_string().contains("structured_edit_error"));
        }
    }

    #[test]
    fn renderer_event_projection_excludes_content_edits_and_observation_authority() {
        let call = AgentToolCall {
            id: "apply-1".to_string(),
            tool: "apply_patch".to_string(),
            args: json!({
                "action":"apply",
                "operation":"update",
                "filePath":"a.txt",
                "observationId":"fobs_secret",
                "edits":[{"kind":"append","text":"secret content"}]
            }),
            approval_status: AgentApprovalStatus::Required,
            reason: None,
        };
        let projected = ApplyPatchTool.event_call_projection(&call);
        assert!(projected.args.get("observationId").is_none());
        assert_eq!(projected.args["editCount"], 1);
        assert!(!projected.args.to_string().contains("secret content"));

        let staged = AgentToolCall {
            id: "append-1".to_string(),
            tool: "apply_patch".to_string(),
            args: json!({
                "action":"append",
                "transactionId":"transaction-1",
                "index":0,
                "expectedDraftRevision":0,
                "content":"PRIVATE_STAGED_BODY_CANARY"
            }),
            approval_status: AgentApprovalStatus::NotRequired,
            reason: None,
        };
        let staged_projection = ApplyPatchTool.event_call_projection(&staged);
        assert_eq!(staged_projection.args["contentBytes"], 26);
        assert!(staged_projection.args.get("contentDigest").is_some());
        assert!(!staged_projection
            .args
            .to_string()
            .contains("PRIVATE_STAGED_BODY_CANARY"));
    }

    #[test]
    fn camel_case_create_update_delete_form_frozen_transactions() {
        let workspace = TestWorkspace::new();
        workspace.write("existing.txt", "alpha\nbeta\n");
        let context = workspace.context();

        let missing = observe(&context, "created.txt");
        let create = proposal(
            &context,
            json!({"action":"apply","operation":"create","filePath":"created.txt","observationId":missing,"content":"created\n"}),
        )
        .unwrap();
        assert_eq!(create.operation, AgentFileChangeOperation::Create);
        assert_eq!(
            create.execution.target_content.as_deref(),
            Some("created\n")
        );
        create.execution.validate().unwrap();
        create.validate().unwrap();
        let mut illegal_approval = create.clone();
        illegal_approval.approval_status = AgentApprovalStatus::NotRequired;
        assert!(illegal_approval.validate().is_err());
        illegal_approval.approval_status = AgentApprovalStatus::Rejected;
        assert!(illegal_approval.validate().is_err());

        let existing = observe(&context, "existing.txt");
        let update = proposal(
            &context,
            json!({"action":"apply","operation":"update","filePath":"existing.txt","observationId":existing,"edits":[{"kind":"replace","oldText":"beta","newText":"gamma"}]}),
        )
        .unwrap();
        assert_eq!(update.operation, AgentFileChangeOperation::Update);
        assert_eq!(
            update.execution.target_content.as_deref(),
            Some("alpha\ngamma\n")
        );
        update.execution.validate().unwrap();
        update.validate().unwrap();

        let existing = observe(&context, "existing.txt");
        let delete = proposal(
            &context,
            json!({"action":"apply","operation":"delete","filePath":"existing.txt","observationId":existing}),
        )
        .unwrap();
        assert_eq!(delete.operation, AgentFileChangeOperation::Delete);
        assert!(delete.execution.target_content.is_none());
        delete.execution.validate().unwrap();
        delete.validate().unwrap();
    }

    #[test]
    fn observation_must_be_exact_and_current() {
        let workspace = TestWorkspace::new();
        workspace.write("a.txt", "before\n");
        workspace.write("b.txt", "other\n");
        let context = workspace.context();
        let observation = observe(&context, "a.txt");
        let missing = proposal(
            &context,
            json!({"action":"apply","operation":"update","filePath":"a.txt","observationId":"missing","content":"after\n"}),
        )
        .unwrap_err();
        assert_eq!(
            missing.code(),
            Some("agent.apply_patch.observation_required")
        );
        let wrong_path = proposal(
            &context,
            json!({"action":"apply","operation":"update","filePath":"b.txt","observationId":observation,"content":"after\n"}),
        )
        .unwrap_err();
        assert_eq!(
            wrong_path.code(),
            Some("agent.apply_patch.observation_path_mismatch")
        );
        workspace.write("a.txt", "changed concurrently\n");
        let stale = proposal(
            &context,
            json!({"action":"apply","operation":"update","filePath":"a.txt","observationId":observation,"content":"after\n"}),
        )
        .unwrap_err();
        assert_eq!(stale.code(), Some("agent.apply_patch.observation_stale"));
    }

    #[test]
    fn malformed_proposal_does_not_burn_observation_but_valid_proposal_claims_it_once() {
        let workspace = TestWorkspace::new();
        workspace.write("once.txt", "before\n");
        let context = workspace.context();
        let observation = observe(&context, "once.txt");
        let malformed = proposal(
            &context,
            json!({
                "action":"apply",
                "operation":"update",
                "filePath":"once.txt",
                "observationId":observation,
                "edits":[{"kind":"replace","oldText":"missing","newText":"after"}]
            }),
        )
        .unwrap_err();
        assert_eq!(malformed.code(), Some("agent.apply_patch.match_not_found"));

        proposal(
            &context,
            json!({
                "action":"apply",
                "operation":"update",
                "filePath":"once.txt",
                "observationId":observation,
                "content":"after\n"
            }),
        )
        .unwrap();
        let replay = proposal(
            &context,
            json!({
                "action":"apply",
                "operation":"update",
                "filePath":"once.txt",
                "observationId":observation,
                "content":"another\n"
            }),
        )
        .unwrap_err();
        assert_eq!(
            replay.code(),
            Some("agent.apply_patch.observation_required")
        );
    }

    #[test]
    fn create_existing_returns_safe_file_exists() {
        let workspace = TestWorkspace::new();
        workspace.write("exists.txt", "keep\n");
        let context = workspace.context();
        let observation = observe(&context, "exists.txt");
        let error = proposal(
            &context,
            json!({"action":"apply","operation":"create","filePath":"exists.txt","observationId":observation,"content":"replace\n"}),
        )
        .unwrap_err();
        assert_eq!(error.code(), Some("agent.apply_patch.file_exists"));
        assert_eq!(error.to_string(), "文件已存在。");
    }

    #[test]
    fn no_workspace_absolute_succeeds_and_relative_fails_before_effects() {
        let workspace = TestWorkspace::new();
        let context = workspace.context_without_workspace();
        let absolute = workspace.root.canonicalize().unwrap().join("absolute.txt");
        let observation = observe(&context, absolute.to_str().unwrap());
        assert!(proposal(
            &context,
            json!({"action":"apply","operation":"create","filePath":absolute,"observationId":observation,"content":"ok\n"})
        )
        .is_ok());

        let alias = format!(
            "@home/.mycopilot-file-observation-test-{}-{}",
            std::process::id(),
            TEST_COUNTER.fetch_add(1, Ordering::Relaxed)
        );
        let alias_observation = observe(&context, &alias);
        let alias_direct = proposal(
            &context,
            json!({
                "action":"apply",
                "operation":"create",
                "filePath":alias,
                "observationId":alias_observation,
                "content":"not published by proposal construction\n"
            }),
        )
        .unwrap();
        assert_eq!(alias_direct.operation, AgentFileChangeOperation::Create);
        assert!(!std::path::Path::new(&alias_direct.execution.canonical_target).exists());

        let relative = proposal(
            &context,
            json!({"action":"apply","operation":"create","filePath":"relative.txt","observationId":"fobs_missing","content":"no\n"}),
        )
        .unwrap_err();
        assert_eq!(
            relative.code(),
            Some("agent.apply_patch.workspace_required")
        );
    }

    #[cfg(unix)]
    #[test]
    fn proposal_freeze_cannot_follow_a_swapped_ancestor_or_leaf() {
        let workspace = TestWorkspace::new();
        workspace.write("parent/target.txt", "authorized base\n");
        workspace.write("outside/target.txt", "outside secret\n");
        let context = workspace.context();
        let target = FileChangePathPolicy::new(Some(&workspace.root), true)
            .resolve("parent/target.txt")
            .unwrap();
        let observation_id = observe(&context, "parent/target.txt");
        let observation = context
            .file_observations()
            .validate(
                &observation_id,
                context.conversation_id().unwrap(),
                context.run_id().unwrap(),
                target.absolute_path(),
            )
            .unwrap();
        let original_parent = workspace.root.join("parent");
        let displaced_parent = workspace.root.join("displaced");
        let outside_parent = workspace.root.join("outside");
        let error = freeze_observed_base_with_hook(&context, &target, &observation, || {
            fs::rename(&original_parent, &displaced_parent).unwrap();
            symlink(&outside_parent, &original_parent).unwrap();
        })
        .unwrap_err();
        assert!(matches!(
            error.code(),
            FileChangeErrorCode::SymlinkForbidden | FileChangeErrorCode::Conflict
        ));
        assert_eq!(
            fs::read_to_string(outside_parent.join("target.txt")).unwrap(),
            "outside secret\n"
        );

        let workspace = TestWorkspace::new();
        workspace.write("target.txt", "authorized base\n");
        workspace.write("outside.txt", "outside secret\n");
        let context = workspace.context();
        let target = FileChangePathPolicy::new(Some(&workspace.root), true)
            .resolve("target.txt")
            .unwrap();
        let observation_id = observe(&context, "target.txt");
        let observation = context
            .file_observations()
            .validate(
                &observation_id,
                context.conversation_id().unwrap(),
                context.run_id().unwrap(),
                target.absolute_path(),
            )
            .unwrap();
        let leaf = workspace.root.join("target.txt");
        let outside = workspace.root.join("outside.txt");
        let error = freeze_observed_base_with_hook(&context, &target, &observation, || {
            fs::remove_file(&leaf).unwrap();
            symlink(&outside, &leaf).unwrap();
        })
        .unwrap_err();
        assert_eq!(error.code(), FileChangeErrorCode::SymlinkForbidden);
        assert_eq!(fs::read_to_string(outside).unwrap(), "outside secret\n");
    }

    #[cfg(unix)]
    #[test]
    fn proposal_freeze_rejects_a_raced_hard_link_before_building_a_diff() {
        let workspace = TestWorkspace::new();
        workspace.write("target.txt", "authorized base\n");
        workspace.write("outside.txt", "outside secret\n");
        let context = workspace.context();
        let target = FileChangePathPolicy::new(Some(&workspace.root), true)
            .resolve("target.txt")
            .unwrap();
        let observation_id = observe(&context, "target.txt");
        let observation = context
            .file_observations()
            .validate(
                &observation_id,
                context.conversation_id().unwrap(),
                context.run_id().unwrap(),
                target.absolute_path(),
            )
            .unwrap();
        let leaf = workspace.root.join("target.txt");
        let outside = workspace.root.join("outside.txt");
        let error = freeze_observed_base_with_hook(&context, &target, &observation, || {
            fs::remove_file(&leaf).unwrap();
            fs::hard_link(&outside, &leaf).unwrap();
        })
        .unwrap_err();
        assert_eq!(error.code(), FileChangeErrorCode::HardLinkForbidden);
        assert_eq!(fs::read_to_string(outside).unwrap(), "outside secret\n");
    }

    fn observe(context: &ToolExecutionContext, path: &str) -> String {
        let result = ToolRegistry::defaults_with_search(None).execute(
            context,
            &AgentToolCall {
                id: format!("read-{}", Uuid::new_v4()),
                tool: "read_file".to_string(),
                args: json!({"path":path}),
                approval_status: AgentApprovalStatus::Approved,
                reason: None,
            },
        );
        assert!(result.ok, "{}", result.error.unwrap_or_default());
        result.result.unwrap()["observationId"]
            .as_str()
            .unwrap()
            .to_string()
    }

    fn proposal(
        context: &ToolExecutionContext,
        args: Value,
    ) -> AgentResult<AgentFileChangeProposal> {
        let call_id = format!("apply-{}", Uuid::new_v4());
        let bound_context = context.clone().with_tool_call_id(call_id.clone());
        direct_proposal_from_call(
            &bound_context,
            &AgentToolCall {
                id: call_id,
                tool: "apply_patch".to_string(),
                args,
                approval_status: AgentApprovalStatus::Required,
                reason: None,
            },
        )
    }

    struct TestWorkspace {
        root: std::path::PathBuf,
    }

    impl TestWorkspace {
        fn new() -> Self {
            let root = std::env::temp_dir().join(format!(
                "mycopilot-direct-file-change-{}",
                TEST_COUNTER.fetch_add(1, Ordering::Relaxed)
            ));
            let _ = fs::remove_dir_all(&root);
            fs::create_dir_all(&root).unwrap();
            Self { root }
        }

        fn write(&self, path: &str, content: &str) {
            let path = self.root.join(path);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, content).unwrap();
        }

        fn context(&self) -> ToolExecutionContext {
            self.context_with_workspace(Some(self.root.clone()))
        }

        fn context_without_workspace(&self) -> ToolExecutionContext {
            self.context_with_workspace(None)
        }

        fn context_with_workspace(&self, root: Option<std::path::PathBuf>) -> ToolExecutionContext {
            ToolExecutionContext::from_run_context(Some(&AgentRunContext {
                collaboration_identity: None,
                conversation_id: Some("direct-test-conversation".to_string()),
                project_id: None,
                workspace: root.map(|root| AgentWorkspaceContext {
                    project_id: None,
                    display_name: Some("test".to_string()),
                    root_path: Some(root.to_string_lossy().to_string()),
                }),
                attachment_library: None,
                permissions: AgentPermissions {
                    read: AgentReadPermission::All,
                    write: AgentWritePermission::All,
                    command: AgentCommandPermission::RequireApproval,
                    command_safety: Default::default(),
                    patch: Default::default(),
                    builtin_execution: Default::default(),
                },
            }))
            .with_runtime_services("direct-test-run".to_string(), None)
            .with_file_change_tool_set_revision("tool-set-test-v1".to_string())
        }
    }

    impl Drop for TestWorkspace {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }
}
