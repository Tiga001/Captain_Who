use super::{AgentTool, AgentToolPermissionPolicy, FileWriteToolAccess, ToolExecutionContext};
use crate::file_change::{
    BoundParent, FileChangeBase, FileChangeCommitter, FileChangeContentState,
    FileChangeDirectBinding, FileChangeEdit, FileChangeError, FileChangeErrorCode,
    FileChangeMutation, FileChangeOperation, FileChangeOutcome, FileChangePathPolicy,
    FileChangePlanRequest, FileChangePlanner, FileChangeProposal, FileChangeStatus,
    FileChangeTransaction, FileObservationIdentity, FileObservationState,
    FILE_CHANGE_SCHEMA_VERSION,
};
use crate::protocol::{
    AgentApprovalStatus, AgentDiffProposal, AgentError, AgentPatchOperation, AgentProposedAction,
    AgentResult, AgentToolCall, AgentToolDefinition, AgentToolSafety, AgentWritePermission,
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
            description: "Request one short, one-shot Direct create, update, or delete operation for a text/code/config file. Before every Direct call, use read_file on the exact target path—even when you expect it to be absent; a directory listing, search result, or earlier read is not a substitute. Copy the returned observationId into this call. create is no-clobber and is valid only with a missing-state observation; if the target exists, use update or delete instead. Prefer exact structured edits for updates. Use write_file for long generated content or a change that must be assembled in stages. Without a workspace, filePath must be an authorized absolute path or use @home, @desktop, @documents, or @downloads; relative paths are invalid. Never switch to run_command, shell redirection, or a script to bypass file-change approval. The same Tool Call remains active through approval and Host execution, and no file is changed before that boundary."
                .to_string(),
            input_schema: patch_input_schema(),
            safety: AgentToolSafety::RequiresApproval,
            requires_workspace: false,
            requires_approval: true,
            approval_mode: crate::protocol::AgentToolApprovalMode::Always,
        }
    }

    fn execute(&self, _context: &ToolExecutionContext, _args: Value) -> AgentResult<Value> {
        Err(AgentError::new(
            "apply_patch 需要用户审批和 Host 执行层，不能由 Agent Runtime 直接应用。",
        ))
    }

    fn permission_policy(&self) -> AgentToolPermissionPolicy {
        AgentToolPermissionPolicy::FileWrite(FileWriteToolAccess::WriteOnly)
    }

    fn proposed_action(
        &self,
        context: &ToolExecutionContext,
        call: &AgentToolCall,
    ) -> AgentResult<AgentProposedAction> {
        Ok(AgentProposedAction::Diff {
            diff: direct_proposal_from_call(context, call)?,
        })
    }

    fn event_call_projection(&self, call: &AgentToolCall) -> AgentToolCall {
        let mut projection = call.clone();
        let Some(args) = projection.args.as_object_mut() else {
            return projection;
        };
        if let Some(content) = args
            .remove("content")
            .and_then(|value| value.as_str().map(str::len))
        {
            args.insert(
                "content".to_string(),
                json!("[frozen in private FileChange transaction]"),
            );
            args.insert("contentBytes".to_string(), json!(content));
        }
        if let Some(edit_count) = args
            .remove("edits")
            .and_then(|value| value.as_array().map(Vec::len))
        {
            args.insert(
                "edits".to_string(),
                json!("[frozen in private FileChange transaction]"),
            );
            args.insert("editCount".to_string(), json!(edit_count));
        }
        args.remove("observationId");
        projection
    }
}

fn patch_input_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "action": {
                "type": "string",
                "enum": ["apply"],
                "description": "Direct mode for one short, complete file change."
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
            "content": {
                "type": "string",
                "maxLength": 32768,
                "description": "Complete UTF-8 content for a short create or update. Use write_file for long or staged content."
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
        "required": ["action", "operation", "filePath", "observationId"],
        "additionalProperties": false
    })
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ApplyPatchArgs {
    action: DirectAction,
    operation: FileChangeOperation,
    file_path: String,
    observation_id: String,
    content: Option<String>,
    edits: Option<Vec<StructuredTextEdit>>,
    summary: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
enum DirectAction {
    Apply,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct StructuredTextEdit {
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

fn direct_proposal_from_call(
    context: &ToolExecutionContext,
    call: &AgentToolCall,
) -> AgentResult<AgentDiffProposal> {
    validate_wire_shape(&call.args).map_err(file_change_agent_error)?;
    let args: ApplyPatchArgs = serde_json::from_value(call.args.clone()).map_err(|_| {
        file_change_agent_error(FileChangeError::new(FileChangeErrorCode::InvalidArguments))
    })?;
    let DirectAction::Apply = args.action;
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
    .resolve(&args.file_path)
    .map_err(file_change_agent_error)?;
    let observation = context
        .file_observations()
        .validate(
            &args.observation_id,
            context.conversation_id()?,
            context.run_id()?,
            target.absolute_path(),
        )
        .map_err(file_change_agent_error)?;
    validate_observation_operation(args.operation, observation.state())
        .map_err(file_change_agent_error)?;
    let frozen_base =
        freeze_observed_base(context, &target, &observation).map_err(file_change_agent_error)?;
    let mutation = mutation_from_args(args.operation, args.content, args.edits)
        .map_err(file_change_agent_error)?;
    let plan = FileChangePlanner
        .plan(FileChangePlanRequest {
            operation: args.operation,
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
        observation_id: args.observation_id,
        observation: observation.checkpoint(),
        source_call_id: call.id.clone(),
        source_args_digest: crate::file_change::proposal_digest(&call.args).map_err(|error| {
            file_change_agent_error(FileChangeError::with_diagnostic(
                FileChangeErrorCode::Failed,
                error.to_string(),
            ))
        })?,
        conversation_id: context.conversation_id()?.to_string(),
        run_id: context.run_id()?.to_string(),
        canonical_target: target.absolute_path().to_string_lossy().into_owned(),
        base_content: plan.base_content.clone(),
        target_content: plan.target_content.clone(),
        delete_journal,
        receipt: None,
        permission_revision: context.file_change_permission_revision().to_string(),
        tool_set_revision: context.file_change_tool_set_revision().to_string(),
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
    Ok(AgentDiffProposal {
        id: call.id.clone(),
        operation: patch_operation(plan.operation),
        file_path: plan.file_path,
        patch: plan.diff,
        base_revision: state_revision(&plan.base).map(str::to_string),
        summary: sanitize_summary(args.summary),
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
            "content",
            "edits",
            "summary",
        ],
    )?;
    for required in ["action", "operation", "filePath", "observationId"] {
        if object.get(required).is_none_or(Value::is_null) {
            return Err(FileChangeError::new(FileChangeErrorCode::InvalidArguments));
        }
    }
    for optional in ["content", "edits", "summary"] {
        if object.get(optional).is_some_and(Value::is_null) {
            return Err(FileChangeError::new(FileChangeErrorCode::InvalidArguments));
        }
    }
    if object.get("action").and_then(Value::as_str) != Some("apply") {
        return Err(FileChangeError::new(FileChangeErrorCode::InvalidArguments));
    }
    let operation = object
        .get("operation")
        .and_then(Value::as_str)
        .ok_or_else(|| FileChangeError::new(FileChangeErrorCode::InvalidArguments))?;
    let has_content = object.contains_key("content");
    let has_edits = object.contains_key("edits");
    let valid = match operation {
        "create" => has_content && !has_edits,
        "update" => has_content ^ has_edits,
        "delete" => !has_content && !has_edits,
        _ => false,
    };
    if !valid {
        Err(FileChangeError::new(
            FileChangeErrorCode::IllegalFieldCombination,
        ))
    } else if let Some(edits) = object.get("edits") {
        validate_edit_wire_shapes(edits)
    } else {
        Ok(())
    }
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

fn domain_edit(edit: StructuredTextEdit) -> Result<FileChangeEdit, FileChangeError> {
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

fn validate_observation_operation(
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
struct FrozenBase {
    content: Option<String>,
    revision: Option<String>,
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
    freeze_observed_base_with_hook(context, target, observation, || {})
}

fn freeze_observed_base_with_hook(
    context: &ToolExecutionContext,
    target: &crate::file_change::ResolvedFileChangeTarget,
    observation: &crate::file_change::FileObservation,
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
            if current.bytes.len() > MAX_EDIT_CONTENT_BYTES {
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

fn patch_operation(operation: FileChangeOperation) -> AgentPatchOperation {
    match operation {
        FileChangeOperation::Create => AgentPatchOperation::Create,
        FileChangeOperation::Update => AgentPatchOperation::Update,
        FileChangeOperation::Delete => AgentPatchOperation::Delete,
    }
}

fn state_revision(state: &FileChangeContentState) -> Option<&str> {
    match state {
        FileChangeContentState::Missing => None,
        FileChangeContentState::Present { revision, .. } => Some(revision),
    }
}

fn sanitize_summary(summary: Option<String>) -> Option<String> {
    summary
        .map(|summary| summary.trim().chars().take(MAX_SUMMARY_CHARS).collect())
        .filter(|summary: &String| !summary.is_empty())
}

fn file_change_agent_error(error: FileChangeError) -> AgentError {
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
    fn direct_schema_is_portable_strict_and_has_no_raw_patch_or_revision() {
        let schema = patch_input_schema();
        assert_eq!(schema["type"], "object");
        assert_eq!(schema["additionalProperties"], false);
        assert_eq!(
            schema["required"],
            json!(["action", "operation", "filePath", "observationId"])
        );
        assert!(schema.get("oneOf").is_none());
        assert!(schema.get("anyOf").is_none());
        assert!(schema.get("allOf").is_none());
        let properties = schema["properties"].as_object().unwrap();
        assert!(!properties.contains_key("patch"));
        assert!(!properties.contains_key("expectedRevision"));
        assert_eq!(properties["edits"]["items"]["additionalProperties"], false);
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
        assert_eq!(create.operation, AgentPatchOperation::Create);
        assert_eq!(
            create.execution.target_content.as_deref(),
            Some("created\n")
        );
        create.execution.validate().unwrap();

        let existing = observe(&context, "existing.txt");
        let update = proposal(
            &context,
            json!({"action":"apply","operation":"update","filePath":"existing.txt","observationId":existing,"edits":[{"kind":"replace","oldText":"beta","newText":"gamma"}]}),
        )
        .unwrap();
        assert_eq!(update.operation, AgentPatchOperation::Update);
        assert_eq!(
            update.execution.target_content.as_deref(),
            Some("alpha\ngamma\n")
        );
        update.execution.validate().unwrap();

        let existing = observe(&context, "existing.txt");
        let delete = proposal(
            &context,
            json!({"action":"apply","operation":"delete","filePath":"existing.txt","observationId":existing}),
        )
        .unwrap();
        assert_eq!(delete.operation, AgentPatchOperation::Delete);
        assert!(delete.execution.target_content.is_none());
        delete.execution.validate().unwrap();
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
        assert_eq!(alias_direct.operation, AgentPatchOperation::Create);
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

    fn proposal(context: &ToolExecutionContext, args: Value) -> AgentResult<AgentDiffProposal> {
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
