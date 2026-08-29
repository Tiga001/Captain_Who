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
    FILE_CHANGE_DIRECT_BINDING_SCHEMA_VERSION, FILE_CHANGE_SCHEMA_VERSION,
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
const APPLY_PATCH_ACTIONS: &[&str] = &[
    "apply", "begin", "append", "edit", "commit", "status", "abort",
];
const APPLY_PATCH_OPERATIONS: &[&str] = &["create", "update", "delete"];
const STAGED_UPDATE_STRATEGIES: &[&str] = &["modify", "rewrite"];

pub(super) struct ApplyPatchTool;

impl AgentTool for ApplyPatchTool {
    fn exposure(&self) -> super::AgentToolExposure {
        super::AgentToolExposure::Stable
    }

    fn definition(&self) -> AgentToolDefinition {
        AgentToolDefinition {
            name: "apply_patch".to_string(),
            description: "Create, update, or delete one UTF-8 text file through one strict FileChange protocol. Put exactly one operation in the required request object; use only the fields listed by its matching schema branch. Before request.action=apply or begin, call read_file on the exact target path and copy fileChangeTarget.filePath and observationId. Direct shape example: {\"request\":{\"action\":\"apply\",\"operation\":\"create\",\"filePath\":\"notes.txt\",\"observationId\":\"fobs_example\",\"content\":\"hello\\n\"}}. Examples show shape only: never copy fobs_example, example transaction IDs, or example cursors; replace them with the exact values returned by this Host. Direct apply accepts at most 32 KiB of complete content; use begin/append/edit/commit for larger content, with append chunks up to 1 MiB and a 4 MiB transaction total. After begin, copy the Host-returned transactionId, nextIndex, and draftRevision exactly. Staged shape example: {\"request\":{\"action\":\"append\",\"transactionId\":\"file-change-staged-v1:example\",\"index\":0,\"expectedDraftRevision\":0,\"content\":\"next chunk\"}}. create is always no-clobber; Staged delete is unsupported. Settle every unfinished transaction with commit or abort before user-visible narration. Without a workspace, filePath must be an authorized absolute path or @home, @desktop, @documents, or @downloads. Never use run_command, redirection, or scripts to bypass file-change approval."
                .to_string(),
            input_schema: patch_input_schema(),
            safety: AgentToolSafety::RequiresApproval,
            requires_workspace: false,
            requires_approval: true,
            approval_mode: crate::protocol::AgentToolApprovalMode::Dynamic,
        }
    }

    fn execute(&self, context: &ToolExecutionContext, args: Value) -> AgentResult<Value> {
        validate_wire_shape(&args).map_err(|error| file_change_wire_error(error, &args))?;
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
        validate_wire_shape(&call.args)
            .map_err(|error| file_change_wire_error(error, &call.args))?;
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
        matches!(apply_patch_action(args), Some("apply" | "commit"))
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
        if validate_wire_shape(&call.args).is_err() {
            projection.args = json!({ "invalidRequest": true });
            return projection;
        }
        let Some(request) = apply_patch_request(&call.args) else {
            projection.args = json!({ "invalidRequest": true });
            return projection;
        };
        let mut safe = Map::new();
        safe.insert("validatedRequest".to_string(), Value::Bool(true));
        copy_bounded_enum(request, &mut safe, "action", APPLY_PATCH_ACTIONS);
        copy_bounded_enum(request, &mut safe, "operation", APPLY_PATCH_OPERATIONS);
        copy_bounded_enum(request, &mut safe, "strategy", STAGED_UPDATE_STRATEGIES);
        copy_bounded_string(request, &mut safe, "filePath", 4_096);
        copy_bounded_string(request, &mut safe, "transactionId", 256);
        copy_u64(request, &mut safe, "index");
        copy_u64(request, &mut safe, "expectedDraftRevision");
        if let Some(content) = request.get("content").and_then(Value::as_str) {
            safe.insert("contentBytes".to_string(), json!(content.len()));
            safe.insert(
                "contentDigest".to_string(),
                json!(crate::file_change::content_digest(content.as_bytes())),
            );
        }
        if let Some(edits) = request.get("edits").and_then(Value::as_array) {
            safe.insert("editCount".to_string(), json!(edits.len()));
            if let Ok(digest) = crate::file_change::proposal_digest(&Value::Array(edits.clone())) {
                safe.insert("editsDigest".to_string(), json!(digest));
            }
        }
        projection.args = json!({ "request": safe });
        projection
    }

    fn trace_call_projection(&self, call: &AgentToolCall) -> AgentToolCall {
        let mut projection = call.clone();
        projection.args = crate::file_change_support::apply_patch_trace_operation(&call.args)
            .unwrap_or_else(|_| json!({ "request": {} }));
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
    let file_path = json!({
        "type": "string",
        "minLength": 1,
        "description": "Copy the exact fileChangeTarget.filePath returned by the immediately preceding read_file call. It may be workspace-relative, an authorized absolute local path, or a supported system alias."
    });
    let observation_id = json!({
        "type": "string",
        "minLength": 1,
        "description": "Copy the opaque observationId returned by that exact read_file call."
    });
    let transaction_id = json!({
        "type": "string",
        "minLength": 1,
        "description": "Copy the opaque transactionId returned by begin or status."
    });
    let index = json!({
        "type": "integer",
        "minimum": 0,
        "description": "Copy the exact nextIndex returned by the preceding successful mutation or status."
    });
    let draft_revision = json!({
        "type": "integer",
        "minimum": 0,
        "description": "Copy the exact draftRevision returned by the preceding successful mutation or status."
    });
    let summary = json!({
        "type": "string",
        "maxLength": MAX_SUMMARY_CHARS,
        "description": "Optional short human-readable summary. Allowed only for apply or commit."
    });
    let edits = structured_edits_schema();
    json!({
        "type": "object",
        "properties": {
            "request": {
                "description": "Exactly one strict FileChange request. Do not add fields from another branch.",
                "oneOf": [
                    {
                        "type": "object",
                        "properties": {
                            "action": { "type": "string", "enum": ["apply"] },
                            "operation": { "type": "string", "enum": ["create"] },
                            "filePath": file_path.clone(),
                            "observationId": observation_id.clone(),
                            "content": {
                                "type": "string",
                                "maxLength": MAX_INLINE_CONTENT_BYTES,
                                "description": "Complete Direct file content, at most 32 KiB of UTF-8 bytes. Empty content creates an empty file."
                            },
                            "summary": summary.clone()
                        },
                        "required": ["action", "operation", "filePath", "observationId", "content"],
                        "additionalProperties": false
                    },
                    {
                        "type": "object",
                        "properties": {
                            "action": { "type": "string", "enum": ["apply"] },
                            "operation": { "type": "string", "enum": ["update"] },
                            "filePath": file_path.clone(),
                            "observationId": observation_id.clone(),
                            "content": {
                                "type": "string",
                                "maxLength": MAX_INLINE_CONTENT_BYTES,
                                "description": "Complete replacement content, at most 32 KiB of UTF-8 bytes. For larger complete replacements use begin/update with strategy=rewrite."
                            },
                            "summary": summary.clone()
                        },
                        "required": ["action", "operation", "filePath", "observationId", "content"],
                        "additionalProperties": false
                    },
                    {
                        "type": "object",
                        "properties": {
                            "action": { "type": "string", "enum": ["apply"] },
                            "operation": { "type": "string", "enum": ["update"] },
                            "filePath": file_path.clone(),
                            "observationId": observation_id.clone(),
                            "edits": edits.clone(),
                            "summary": summary.clone()
                        },
                        "required": ["action", "operation", "filePath", "observationId", "edits"],
                        "additionalProperties": false
                    },
                    {
                        "type": "object",
                        "properties": {
                            "action": { "type": "string", "enum": ["apply"] },
                            "operation": { "type": "string", "enum": ["delete"] },
                            "filePath": file_path.clone(),
                            "observationId": observation_id.clone(),
                            "summary": summary.clone()
                        },
                        "required": ["action", "operation", "filePath", "observationId"],
                        "additionalProperties": false
                    },
                    {
                        "type": "object",
                        "properties": {
                            "action": { "type": "string", "enum": ["begin"] },
                            "operation": { "type": "string", "enum": ["create"] },
                            "filePath": file_path.clone(),
                            "observationId": observation_id.clone()
                        },
                        "required": ["action", "operation", "filePath", "observationId"],
                        "additionalProperties": false
                    },
                    {
                        "type": "object",
                        "properties": {
                            "action": { "type": "string", "enum": ["begin"] },
                            "operation": { "type": "string", "enum": ["update"] },
                            "filePath": file_path.clone(),
                            "observationId": observation_id.clone(),
                            "strategy": {
                                "type": "string",
                                "enum": ["modify", "rewrite"],
                                "description": "modify starts from observed content; rewrite starts from an empty draft."
                            }
                        },
                        "required": ["action", "operation", "filePath", "observationId", "strategy"],
                        "additionalProperties": false
                    },
                    {
                        "type": "object",
                        "properties": {
                            "action": { "type": "string", "enum": ["append"] },
                            "transactionId": transaction_id.clone(),
                            "index": index.clone(),
                            "expectedDraftRevision": draft_revision.clone(),
                            "content": {
                                "type": "string",
                                "minLength": 1,
                                "maxLength": file_change_staged::MAX_STAGED_CHUNK_BYTES,
                                "description": "One non-empty append chunk, at most 1 MiB of UTF-8 bytes. The complete transaction may not exceed 4 MiB."
                            }
                        },
                        "required": ["action", "transactionId", "index", "expectedDraftRevision", "content"],
                        "additionalProperties": false
                    },
                    {
                        "type": "object",
                        "properties": {
                            "action": { "type": "string", "enum": ["edit"] },
                            "transactionId": transaction_id.clone(),
                            "index": index,
                            "expectedDraftRevision": draft_revision.clone(),
                            "edits": edits
                        },
                        "required": ["action", "transactionId", "index", "expectedDraftRevision", "edits"],
                        "additionalProperties": false
                    },
                    {
                        "type": "object",
                        "properties": {
                            "action": { "type": "string", "enum": ["commit"] },
                            "transactionId": transaction_id.clone(),
                            "expectedDraftRevision": draft_revision,
                            "summary": summary
                        },
                        "required": ["action", "transactionId", "expectedDraftRevision"],
                        "additionalProperties": false
                    },
                    {
                        "type": "object",
                        "properties": {
                            "action": { "type": "string", "enum": ["status"] },
                            "transactionId": transaction_id.clone()
                        },
                        "required": ["action", "transactionId"],
                        "additionalProperties": false
                    },
                    {
                        "type": "object",
                        "properties": {
                            "action": { "type": "string", "enum": ["abort"] },
                            "transactionId": transaction_id
                        },
                        "required": ["action", "transactionId"],
                        "additionalProperties": false
                    }
                ]
            }
        },
        "required": ["request"],
        "additionalProperties": false
    })
}

fn structured_edits_schema() -> Value {
    json!({
        "type": "array",
        "minItems": 1,
        "maxItems": 128,
        "description": "One to 128 ordered exact text edits. Direct edits must leave a target no larger than 240,000 UTF-8 bytes; Staged edits must keep the transaction total at or below 4 MiB. replace matches exact bytes once unless replaceAll=true; no trimming, normalization, or fuzzy matching occurs.",
        "items": {
            "oneOf": [
                {
                    "type": "object",
                    "properties": {
                        "kind": { "type": "string", "enum": ["replace"] },
                        "oldText": { "type": "string", "minLength": 1 },
                        "newText": { "type": "string" },
                        "replaceAll": { "type": "boolean" }
                    },
                    "required": ["kind", "oldText", "newText"],
                    "additionalProperties": false
                },
                {
                    "type": "object",
                    "properties": {
                        "kind": { "type": "string", "enum": ["insert_before"] },
                        "anchor": { "type": "string", "minLength": 1 },
                        "text": { "type": "string" }
                    },
                    "required": ["kind", "anchor", "text"],
                    "additionalProperties": false
                },
                {
                    "type": "object",
                    "properties": {
                        "kind": { "type": "string", "enum": ["insert_after"] },
                        "anchor": { "type": "string", "minLength": 1 },
                        "text": { "type": "string" }
                    },
                    "required": ["kind", "anchor", "text"],
                    "additionalProperties": false
                },
                {
                    "type": "object",
                    "properties": {
                        "kind": { "type": "string", "enum": ["append"] },
                        "text": { "type": "string" }
                    },
                    "required": ["kind", "text"],
                    "additionalProperties": false
                },
                {
                    "type": "object",
                    "properties": {
                        "kind": { "type": "string", "enum": ["prepend"] },
                        "text": { "type": "string" }
                    },
                    "required": ["kind", "text"],
                    "additionalProperties": false
                }
            ]
        }
    })
}

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
fn direct_proposal_from_call(
    context: &ToolExecutionContext,
    call: &AgentToolCall,
) -> AgentResult<AgentFileChangeProposal> {
    validate_wire_shape(&call.args).map_err(|error| file_change_wire_error(error, &call.args))?;
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
    let observation = context
        .file_observations()
        .validate(
            &observation_id,
            context.conversation_id()?,
            context.run_id()?,
            target.absolute_path(),
        )
        .map_err(|error| {
            file_change_agent_error_for_direct(error, &file_path, &observation_id, staged_recovery)
        })?;
    validate_observation_operation(operation, observation.state()).map_err(|error| {
        file_change_agent_error_for_direct(error, &file_path, &observation_id, staged_recovery)
    })?;
    let frozen_base = freeze_observed_base(context, &target, &observation).map_err(|error| {
        file_change_agent_error_for_direct(error, &file_path, &observation_id, staged_recovery)
    })?;
    let mutation = mutation_from_args(operation, content, edits).map_err(|error| {
        file_change_agent_error_for_direct(error, &file_path, &observation_id, staged_recovery)
    })?;
    let plan = FileChangePlanner
        .plan(FileChangePlanRequest {
            operation,
            file_path: target.display_path(),
            base: frozen_base.as_plan_base(),
            mutation,
        })
        .map_err(|error| {
            file_change_agent_error_for_direct(error, &file_path, &observation_id, staged_recovery)
        })?;
    if plan
        .target_content
        .as_ref()
        .is_some_and(|content| content.len() > MAX_EDIT_CONTENT_BYTES)
    {
        return Err(file_change_agent_error_for_direct(
            FileChangeError::new(FileChangeErrorCode::ContentTooLarge),
            &file_path,
            &observation_id,
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
    context
        .file_observations()
        .claim(
            &execution.observation_id,
            context.conversation_id()?,
            context.run_id()?,
            target.absolute_path(),
        )
        .map_err(|error| file_change_agent_error_for_path(error, &file_path))?;
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

fn copy_bounded_enum(
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

fn copy_bounded_string(
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

fn copy_u64(source: &Map<String, Value>, target: &mut Map<String, Value>, key: &str) {
    if let Some(value) = source.get(key).and_then(Value::as_u64) {
        target.insert(key.to_string(), json!(value));
    }
}

fn validate_wire_shape(value: &Value) -> Result<(), FileChangeError> {
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
    file_change_agent_error_with_continuation(error, None, None)
}

#[derive(Debug, Clone, Copy)]
enum DirectStagedRecovery {
    Create,
    Modify,
    Rewrite,
}

fn file_change_wire_error(error: FileChangeError, value: &Value) -> AgentError {
    let continuation = wire_safe_continuation(error.code(), value);
    file_change_agent_error_with_continuation(error, continuation, Some(wire_expected_shape(value)))
}

fn wire_safe_continuation(code: FileChangeErrorCode, value: &Value) -> Option<Value> {
    let request = apply_patch_request(value)?;
    let action = request.get("action").and_then(Value::as_str)?;
    if matches!(action, "apply" | "begin") {
        let file_path = bounded_nonempty_string(request, "filePath", 4_096)?;
        let observation_is_invalid = request
            .get("observationId")
            .and_then(Value::as_str)
            .is_none_or(str::is_empty);
        if observation_is_invalid {
            return Some(read_file_continuation(file_path));
        }
        if code == FileChangeErrorCode::ContentTooLarge && action == "apply" {
            let observation_id = bounded_nonempty_string(request, "observationId", 256)?;
            let staged_recovery = match request.get("operation").and_then(Value::as_str) {
                Some("create") if request.contains_key("content") => {
                    Some(DirectStagedRecovery::Create)
                }
                Some("update") if request.contains_key("content") => {
                    Some(DirectStagedRecovery::Rewrite)
                }
                Some("update") if request.contains_key("edits") => {
                    Some(DirectStagedRecovery::Modify)
                }
                _ => None,
            };
            return staged_recovery
                .map(|recovery| direct_staged_continuation(file_path, observation_id, recovery));
        }
        return None;
    }
    if matches!(action, "append" | "edit" | "commit") {
        let transaction_id = bounded_nonempty_string(request, "transactionId", 256)?;
        if request_has_invalid_cursor(action, request) {
            return Some(staged_status_continuation(transaction_id));
        }
    }
    None
}

fn bounded_nonempty_string<'a>(
    request: &'a Map<String, Value>,
    key: &str,
    max_chars: usize,
) -> Option<&'a str> {
    request
        .get(key)?
        .as_str()
        .filter(|value| !value.is_empty() && value.chars().count() <= max_chars)
}

fn request_has_invalid_cursor(action: &str, request: &Map<String, Value>) -> bool {
    let expected_revision_invalid = request
        .get("expectedDraftRevision")
        .is_none_or(|value| value.as_u64().is_none());
    let index_invalid = matches!(action, "append" | "edit")
        && request
            .get("index")
            .is_none_or(|value| value.as_u64().is_none());
    let forbidden_cursor_field = ["nextIndex", "draftRevision", "expected_draft_revision"]
        .iter()
        .any(|key| request.contains_key(*key))
        || (action == "commit" && request.contains_key("index"));
    expected_revision_invalid || index_invalid || forbidden_cursor_field
}

fn file_change_agent_error_for_direct(
    error: FileChangeError,
    file_path: &str,
    observation_id: &str,
    staged_recovery: Option<DirectStagedRecovery>,
) -> AgentError {
    let continuation = if error.code() == FileChangeErrorCode::ContentTooLarge {
        staged_recovery
            .map(|recovery| direct_staged_continuation(file_path, observation_id, recovery))
    } else if error_requires_reread(error.code()) {
        Some(read_file_continuation(file_path))
    } else {
        None
    };
    file_change_agent_error_with_continuation(error, continuation, None)
}

pub(super) fn file_change_agent_error_for_path(
    error: FileChangeError,
    file_path: &str,
) -> AgentError {
    let continuation =
        error_requires_reread(error.code()).then(|| read_file_continuation(file_path));
    file_change_agent_error_with_continuation(error, continuation, None)
}

pub(super) fn file_change_agent_error_for_transaction(
    error: FileChangeError,
    transaction_id: &str,
) -> AgentError {
    let continuation = matches!(
        error.code(),
        FileChangeErrorCode::DraftRevisionConflict | FileChangeErrorCode::MutationOutOfOrder
    )
    .then(|| staged_status_continuation(transaction_id));
    file_change_agent_error_with_continuation(error, continuation, None)
}

fn error_requires_reread(code: FileChangeErrorCode) -> bool {
    matches!(
        code,
        FileChangeErrorCode::FileExists
            | FileChangeErrorCode::FileMissing
            | FileChangeErrorCode::RevisionConflict
            | FileChangeErrorCode::MatchNotFound
            | FileChangeErrorCode::AmbiguousMatch
            | FileChangeErrorCode::ObservationRequired
            | FileChangeErrorCode::ObservationExpired
            | FileChangeErrorCode::ObservationOwnerMismatch
            | FileChangeErrorCode::ObservationPathMismatch
            | FileChangeErrorCode::ObservationStale
    )
}

fn read_file_continuation(file_path: &str) -> Value {
    json!({
        "tool": "read_file",
        "args": { "path": file_path }
    })
}

fn direct_staged_continuation(
    file_path: &str,
    observation_id: &str,
    recovery: DirectStagedRecovery,
) -> Value {
    let mut request = Map::from_iter([
        ("action".to_string(), json!("begin")),
        (
            "operation".to_string(),
            json!(match recovery {
                DirectStagedRecovery::Create => "create",
                DirectStagedRecovery::Modify | DirectStagedRecovery::Rewrite => "update",
            }),
        ),
        ("filePath".to_string(), json!(file_path)),
        ("observationId".to_string(), json!(observation_id)),
    ]);
    match recovery {
        DirectStagedRecovery::Create => {}
        DirectStagedRecovery::Modify => {
            request.insert("strategy".to_string(), json!("modify"));
        }
        DirectStagedRecovery::Rewrite => {
            request.insert("strategy".to_string(), json!("rewrite"));
        }
    }
    json!({
        "tool": "apply_patch",
        "args": { "request": request }
    })
}

fn staged_status_continuation(transaction_id: &str) -> Value {
    json!({
        "tool": "apply_patch",
        "args": {
            "request": {
                "action": "status",
                "transactionId": transaction_id
            }
        }
    })
}

fn wire_expected_shape(value: &Value) -> Value {
    let root = json!({
        "requiredFields": ["request"],
        "allowedFields": ["request"]
    });
    let Some(request) = apply_patch_request(value) else {
        return json!({
            "root": root,
            "request": {
                "allowedActions": ["apply", "begin", "append", "edit", "commit", "status", "abort"]
            }
        });
    };
    // A malformed call may omit the discriminator while still making exactly one branch clear.
    // Infer only those unambiguous literals so the correction identifies the concrete current
    // shape without echoing private content/edits back through the error channel.
    let action = request
        .get("action")
        .and_then(Value::as_str)
        .or_else(|| inferred_action_for_expected_shape(request));
    let operation = request
        .get("operation")
        .and_then(Value::as_str)
        .or_else(|| inferred_operation_for_expected_shape(action, request));
    let (required, optional, constraints): (Vec<&str>, Vec<&str>, Vec<&str>) = match (
        action, operation,
    ) {
        (Some("apply"), Some("create")) => (
            vec![
                "action",
                "operation",
                "filePath",
                "observationId",
                "content",
            ],
            vec!["summary"],
            vec!["content is complete UTF-8 text and at most 32 KiB"],
        ),
        (Some("apply"), Some("update")) if request.contains_key("edits") => (
            vec!["action", "operation", "filePath", "observationId", "edits"],
            vec!["summary"],
            vec!["do not include content; use exactly one edit shape"],
        ),
        (Some("apply"), Some("update")) => (
            vec![
                "action",
                "operation",
                "filePath",
                "observationId",
                "content",
            ],
            vec!["summary"],
            vec!["choose exactly one of content or edits; content is at most 32 KiB"],
        ),
        (Some("apply"), Some("delete")) => (
            vec!["action", "operation", "filePath", "observationId"],
            vec!["summary"],
            vec!["do not include content or edits"],
        ),
        (Some("begin"), Some("create")) => (
            vec!["action", "operation", "filePath", "observationId"],
            vec![],
            vec!["do not include strategy, content, edits, or summary"],
        ),
        (Some("begin"), Some("update")) => (
            vec![
                "action",
                "operation",
                "filePath",
                "observationId",
                "strategy",
            ],
            vec![],
            vec!["strategy is modify or rewrite; do not include content or edits"],
        ),
        (Some("append"), _) => (
            vec![
                "action",
                "transactionId",
                "index",
                "expectedDraftRevision",
                "content",
            ],
            vec![],
            vec!["content is non-empty and at most 1 MiB; transaction total is at most 4 MiB"],
        ),
        (Some("edit"), _) => (
            vec![
                "action",
                "transactionId",
                "index",
                "expectedDraftRevision",
                "edits",
            ],
            vec![],
            vec!["do not include content; every edit uses exactly one listed edit shape"],
        ),
        (Some("commit"), _) => (
            vec!["action", "transactionId", "expectedDraftRevision"],
            vec!["summary"],
            vec![],
        ),
        (Some("status" | "abort"), _) => (vec!["action", "transactionId"], vec![], vec![]),
        _ => {
            return json!({
                "root": root,
                "request": {
                    "allowedActions": ["apply", "begin", "append", "edit", "commit", "status", "abort"],
                    "applyOperations": ["create", "update", "delete"],
                    "beginOperations": ["create", "update"]
                }
            });
        }
    };
    let mut allowed = required.clone();
    allowed.extend(optional.iter().copied());
    json!({
        "root": root,
        "request": {
            "action": action,
            "operation": operation,
            "requiredFields": required,
            "optionalFields": optional,
            "allowedFields": allowed,
            "constraints": constraints,
            "editShapes": {
                "replace": { "requiredFields": ["kind", "oldText", "newText"], "optionalFields": ["replaceAll"] },
                "insert_before": { "requiredFields": ["kind", "anchor", "text"] },
                "insert_after": { "requiredFields": ["kind", "anchor", "text"] },
                "append": { "requiredFields": ["kind", "text"] },
                "prepend": { "requiredFields": ["kind", "text"] }
            }
        }
    })
}

fn inferred_action_for_expected_shape(request: &Map<String, Value>) -> Option<&'static str> {
    match request.get("operation").and_then(Value::as_str) {
        Some("delete") => Some("apply"),
        Some("create" | "update")
            if request.contains_key("content")
                || request.contains_key("edits")
                || request.contains_key("summary") =>
        {
            Some("apply")
        }
        Some("create" | "update") if request.contains_key("strategy") => Some("begin"),
        Some("create")
            if request.contains_key("filePath") && request.contains_key("observationId") =>
        {
            Some("begin")
        }
        _ => None,
    }
}

fn inferred_operation_for_expected_shape(
    action: Option<&str>,
    request: &Map<String, Value>,
) -> Option<&'static str> {
    match action {
        Some("apply") if request.contains_key("edits") => Some("update"),
        Some("apply")
            if !request.contains_key("content")
                && !request.contains_key("edits")
                && request.contains_key("filePath")
                && request.contains_key("observationId") =>
        {
            Some("delete")
        }
        Some("begin") if request.contains_key("strategy") => Some("update"),
        Some("begin")
            if request.contains_key("filePath") && request.contains_key("observationId") =>
        {
            Some("create")
        }
        _ => None,
    }
}

fn file_change_agent_error_with_continuation(
    error: FileChangeError,
    continuation: Option<Value>,
    expected_shape: Option<Value>,
) -> AgentError {
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
            "continueWith": continuation.unwrap_or(Value::Null),
            "expectedShape": expected_shape.unwrap_or(Value::Null)
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

    fn wire(request: Value) -> Value {
        json!({ "request": request })
    }

    #[test]
    fn unified_schema_is_portable_strict_and_has_no_raw_patch_or_revision() {
        let definition = ApplyPatchTool.definition();
        let schema = definition.input_schema;
        assert!(definition.description.contains("Examples show shape only"));
        assert!(definition
            .description
            .contains("replace them with the exact values returned by this Host"));
        assert_eq!(schema["type"], "object");
        assert_eq!(schema["additionalProperties"], false);
        assert_eq!(schema["required"], json!(["request"]));
        assert!(schema.get("oneOf").is_none());
        assert!(schema.get("anyOf").is_none());
        assert!(schema.get("allOf").is_none());
        let request = &schema["properties"]["request"];
        assert_eq!(request["oneOf"].as_array().unwrap().len(), 11);
        assert!(!schema.to_string().contains("\"patch\""));
        assert!(!schema.to_string().contains("expectedRevision"));
        let update_edits = &request["oneOf"][2];
        assert_eq!(
            update_edits["properties"]["edits"]["items"]["oneOf"]
                .as_array()
                .unwrap()
                .len(),
            5
        );
    }

    #[test]
    fn staged_wire_enforces_every_exact_action_matrix() {
        let valid = [
            wire(
                json!({"action":"begin","operation":"create","filePath":"a.txt","observationId":"fobs_x"}),
            ),
            wire(
                json!({"action":"begin","operation":"update","filePath":"a.txt","observationId":"fobs_x","strategy":"modify"}),
            ),
            wire(
                json!({"action":"begin","operation":"update","filePath":"a.txt","observationId":"fobs_x","strategy":"rewrite"}),
            ),
            wire(
                json!({"action":"append","transactionId":"file-change-staged-v1:x","index":0,"expectedDraftRevision":0,"content":"x"}),
            ),
            wire(
                json!({"action":"edit","transactionId":"file-change-staged-v1:x","index":1,"expectedDraftRevision":1,"edits":[{"kind":"append","text":"y"}]}),
            ),
            wire(
                json!({"action":"commit","transactionId":"file-change-staged-v1:x","expectedDraftRevision":2,"summary":"done"}),
            ),
            wire(json!({"action":"status","transactionId":"file-change-staged-v1:x"})),
            wire(json!({"action":"abort","transactionId":"file-change-staged-v1:x"})),
        ];
        for value in valid {
            validate_wire_shape(&value).unwrap_or_else(|error| panic!("rejected {value}: {error}"));
        }

        let invalid = [
            wire(
                json!({"action":"begin","operation":"delete","filePath":"a.txt","observationId":"fobs_x"}),
            ),
            wire(
                json!({"action":"begin","operation":"create","filePath":"a.txt","observationId":"fobs_x","strategy":"rewrite"}),
            ),
            wire(
                json!({"action":"begin","operation":"update","filePath":"a.txt","observationId":"fobs_x"}),
            ),
            wire(
                json!({"action":"append","transactionId":"x","index":0,"expectedDraftRevision":null,"content":"x"}),
            ),
            wire(
                json!({"action":"edit","transactionId":"x","index":0,"expectedDraftRevision":0,"edits":[],"content":"x"}),
            ),
            wire(json!({"action":"commit","transactionId":"x","expected_draft_revision":0})),
            wire(json!({"action":"status","transactionId":"x","summary":"extra"})),
            wire(json!({"action":"abort","transactionId":"x","extra":true})),
        ];
        for value in invalid {
            assert!(validate_wire_shape(&value).is_err(), "accepted {value}");
        }
    }

    #[test]
    fn strict_wire_rejects_missing_null_unknown_snake_case_raw_patch_and_bad_combinations() {
        let valid = wire(json!({
            "action": "apply", "operation": "delete", "filePath": "a.txt",
            "observationId": "fobs_example"
        }));
        let invalid = [
            json!({"action":"apply","operation":"delete","filePath":"a.txt","observationId":"fobs_example"}),
            json!({"operation":"delete","filePath":"a.txt","observationId":"fobs_example"}),
            wire(
                json!({"action":"apply","operation":"delete","filePath":"a.txt","observationId":null}),
            ),
            wire(
                json!({"action":"apply","operation":"delete","filePath":"a.txt","observationId":"fobs_example","extra":true}),
            ),
            wire(
                json!({"action":"apply","operation":"delete","file_path":"a.txt","observationId":"fobs_example"}),
            ),
            wire(
                json!({"action":"apply","operation":"update","filePath":"a.txt","observationId":"fobs_example","patch":"@@"}),
            ),
            wire(
                json!({"action":"apply","operation":"update","filePath":"a.txt","observationId":"fobs_example","content":"x","edits":[]}),
            ),
            wire(
                json!({"action":"apply","operation":"update","filePath":"a.txt","observationId":"fobs_example","edits":[{"kind":"replace","oldText":"a","newText":"b","replaceAll":null}]}),
            ),
            wire(
                json!({"action":"apply","operation":"update","filePath":"a.txt","observationId":"fobs_example","edits":[{"kind":"append","text":"x","extra":true}]}),
            ),
            wire(
                json!({"action":"apply","operation":"update","filePath":"a.txt","observationId":"fobs_example","edits":[{"kind":"insert_before","text":"x"}]}),
            ),
            wire(
                json!({"action":"apply","operation":"delete","filePath":"a.txt","observationId":"fobs_example","content":"x"}),
            ),
            wire(json!({
                "action":"apply",
                "operation":"delete",
                "filePath":"a.txt",
                "observationId":"fobs_example",
                "summary":"x".repeat(MAX_SUMMARY_CHARS + 1)
            })),
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
                wire(
                    json!({"action":1,"operation":"delete","filePath":"a.txt","observationId":"fobs_x"}),
                ),
                "agent.apply_patch.invalid_arguments",
            ),
            (
                wire(
                    json!({"action":"apply","operation":"delete","filePath":"","observationId":"fobs_x"}),
                ),
                "agent.apply_patch.invalid_arguments",
            ),
            (
                wire(
                    json!({"action":"apply","operation":"delete","filePath":"a.txt","observationId":""}),
                ),
                "agent.apply_patch.invalid_arguments",
            ),
            (
                wire(
                    json!({"action":"apply","operation":"update","filePath":"a.txt","observationId":"fobs_x","patch":"@@"}),
                ),
                "agent.apply_patch.unknown_field",
            ),
            (
                wire(
                    json!({"action":"apply","operation":"delete","filePath":"a.txt","observationId":"fobs_x","summary":null}),
                ),
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
    fn wire_errors_return_safe_exact_and_parseable_corrections() {
        let workspace = TestWorkspace::new();
        let context = workspace.context();
        let registry = ToolRegistry::defaults_with_search(None);

        let missing_observation = wire(json!({
            "action":"apply",
            "operation":"create",
            "filePath":"missing.txt",
            "content":"hello\n"
        }));
        let error = file_change_wire_error(
            validate_wire_shape(&missing_observation).unwrap_err(),
            &missing_observation,
        );
        let continuation = &error.details().unwrap()["continueWith"];
        assert_eq!(continuation["tool"], "read_file");
        assert_eq!(continuation["args"], json!({"path":"missing.txt"}));
        let read = registry.execute(
            &context,
            &AgentToolCall {
                id: "correct-read".to_string(),
                tool: "read_file".to_string(),
                args: continuation["args"].clone(),
                approval_status: AgentApprovalStatus::Approved,
                reason: None,
            },
        );
        assert!(read.ok, "{}", read.error.unwrap_or_default());

        let bad_cursor = wire(json!({
            "action":"append",
            "transactionId":"file-change-staged-v1:current",
            "content":"chunk"
        }));
        let error =
            file_change_wire_error(validate_wire_shape(&bad_cursor).unwrap_err(), &bad_cursor);
        let continuation = &error.details().unwrap()["continueWith"];
        assert_eq!(continuation["tool"], "apply_patch");
        assert_eq!(continuation["args"]["request"]["action"], "status");
        validate_wire_shape(&continuation["args"]).unwrap();
        parse_args(continuation["args"].clone()).unwrap();

        let observation_id = observe(&context, "large.txt");
        let oversized = wire(json!({
            "action":"apply",
            "operation":"create",
            "filePath":"large.txt",
            "observationId":observation_id,
            "content":"DIRECT_PRIVATE_CANARY".repeat(MAX_INLINE_CONTENT_BYTES)
        }));
        let error =
            file_change_wire_error(validate_wire_shape(&oversized).unwrap_err(), &oversized);
        let details = error.details().unwrap();
        let continuation = &details["continueWith"];
        assert_eq!(continuation["tool"], "apply_patch");
        assert_eq!(continuation["args"]["request"]["action"], "begin");
        assert_eq!(continuation["args"]["request"]["operation"], "create");
        validate_wire_shape(&continuation["args"]).unwrap();
        parse_args(continuation["args"].clone()).unwrap();
        assert!(!details.to_string().contains("DIRECT_PRIVATE_CANARY"));
        assert!(!details.to_string().contains("<exact filePath>"));
    }

    #[test]
    fn wire_errors_describe_the_exact_branch_without_echoing_body_fields() {
        let invalid_begin = wire(json!({
            "action":"begin",
            "operation":"create",
            "filePath":"new.txt",
            "observationId":"fobs_example",
            "strategy":"rewrite"
        }));
        let invalid = [
            invalid_begin.clone(),
            wire(json!({
                "action":"edit",
                "transactionId":"file-change-staged-v1:current",
                "index":0,
                "expectedDraftRevision":0,
                "edits":[{"kind":"append","text":"PRIVATE_EDIT_CANARY"}],
                "content":"PRIVATE_EXTRA_CANARY"
            })),
        ];
        for value in invalid {
            let error = file_change_wire_error(validate_wire_shape(&value).unwrap_err(), &value);
            let shape = &error.details().unwrap()["expectedShape"]["request"];
            assert!(shape["allowedFields"].as_array().is_some());
            assert!(!shape.to_string().contains("PRIVATE_EDIT_CANARY"));
            assert!(!shape.to_string().contains("PRIVATE_EXTRA_CANARY"));
        }
        let begin_shape = wire_expected_shape(&invalid_begin);
        assert!(!begin_shape["request"]["allowedFields"]
            .as_array()
            .unwrap()
            .contains(&json!("strategy")));
    }

    #[test]
    fn black_box_failure_shapes_correct_missing_discriminators_and_mixed_edit_fields() {
        // Regression for the manual A07 failure: the model supplied an otherwise exact update
        // edit but omitted `action`. The correction must name the apply/update branch instead of
        // returning only a low-information list of every action.
        let missing_action = wire(json!({
            "operation":"update",
            "filePath":"crlf.txt",
            "observationId":"fobs_example",
            "edits":[{"kind":"replace","oldText":"CRLF_TARGET","newText":"CRLF_REPLACED"}]
        }));
        let shape = wire_expected_shape(&missing_action);
        assert_eq!(shape["request"]["action"], "apply");
        assert_eq!(shape["request"]["operation"], "update");
        assert_eq!(
            shape["request"]["requiredFields"],
            json!(["action", "operation", "filePath", "observationId", "edits"])
        );

        // Regressions for A04/A11: every edit kind advertises its own exact keys, while
        // begin/create explicitly excludes the update-only strategy field.
        let mixed_edit = wire(json!({
            "action":"apply",
            "operation":"update",
            "filePath":"existing.txt",
            "observationId":"fobs_example",
            "edits":[{"kind":"insert_before","anchor":"TARGET_ONCE","newText":"PRIVATE"}]
        }));
        let details =
            file_change_wire_error(validate_wire_shape(&mixed_edit).unwrap_err(), &mixed_edit);
        let expected = &details.details().unwrap()["expectedShape"]["request"];
        assert_eq!(
            expected["editShapes"]["insert_before"]["requiredFields"],
            json!(["kind", "anchor", "text"])
        );
        assert!(!expected.to_string().contains("PRIVATE"));

        let create_with_strategy = wire(json!({
            "action":"begin",
            "operation":"create",
            "filePath":"long-staged.md",
            "observationId":"fobs_example",
            "strategy":"rewrite"
        }));
        let expected = wire_expected_shape(&create_with_strategy);
        assert_eq!(expected["request"]["action"], "begin");
        assert_eq!(expected["request"]["operation"], "create");
        assert!(!expected["request"]["allowedFields"]
            .as_array()
            .unwrap()
            .contains(&json!("strategy")));
    }

    #[test]
    fn renderer_event_projection_excludes_content_edits_and_observation_authority() {
        let call = AgentToolCall {
            id: "apply-1".to_string(),
            tool: "apply_patch".to_string(),
            args: wire(json!({
                "action":"apply",
                "operation":"update",
                "filePath":"a.txt",
                "observationId":"fobs_secret",
                "edits":[{"kind":"append","text":"secret content"}]
            })),
            approval_status: AgentApprovalStatus::Required,
            reason: None,
        };
        let projected = ApplyPatchTool.event_call_projection(&call);
        assert!(projected.args["request"].get("observationId").is_none());
        assert_eq!(projected.args["request"]["editCount"], 1);
        assert!(!projected.args.to_string().contains("secret content"));

        let staged = AgentToolCall {
            id: "append-1".to_string(),
            tool: "apply_patch".to_string(),
            args: wire(json!({
                "action":"append",
                "transactionId":"transaction-1",
                "index":0,
                "expectedDraftRevision":0,
                "content":"PRIVATE_STAGED_BODY_CANARY"
            })),
            approval_status: AgentApprovalStatus::NotRequired,
            reason: None,
        };
        let staged_projection = ApplyPatchTool.event_call_projection(&staged);
        assert_eq!(staged_projection.args["request"]["contentBytes"], 26);
        assert!(staged_projection.args["request"]
            .get("contentDigest")
            .is_some());
        assert!(!staged_projection
            .args
            .to_string()
            .contains("PRIVATE_STAGED_BODY_CANARY"));

        let flat_malformed = AgentToolCall {
            id: "flat-malformed".to_string(),
            tool: "apply_patch".to_string(),
            args: json!({"action":"apply","content":"PRIVATE_FLAT_CANARY"}),
            approval_status: AgentApprovalStatus::Required,
            reason: None,
        };
        let flat_projection = ApplyPatchTool.event_call_projection(&flat_malformed);
        assert_eq!(flat_projection.args, json!({"invalidRequest":true}));
        assert!(!flat_projection
            .args
            .to_string()
            .contains("PRIVATE_FLAT_CANARY"));

        let nested_malformed = AgentToolCall {
            id: "nested-malformed".to_string(),
            tool: "apply_patch".to_string(),
            args: wire(json!({
                "action":"apply",
                "operation":"delete",
                "filePath":"PRIVATE_PATH_CANARY",
                "body":"PRIVATE_NESTED_CANARY"
            })),
            approval_status: AgentApprovalStatus::Required,
            reason: None,
        };
        let nested_projection = ApplyPatchTool.event_call_projection(&nested_malformed);
        assert_eq!(nested_projection.args, json!({"invalidRequest":true}));
        assert!(!nested_projection
            .args
            .to_string()
            .contains("PRIVATE_NESTED_CANARY"));
        assert!(!nested_projection
            .args
            .to_string()
            .contains("PRIVATE_PATH_CANARY"));
    }

    #[test]
    fn durable_trace_projection_uses_the_authority_digest_shape_without_private_text() {
        let call = AgentToolCall {
            id: "trace-1".to_string(),
            tool: "apply_patch".to_string(),
            args: wire(json!({
                "action":"apply",
                "operation":"create",
                "filePath":"trace.txt",
                "observationId":"fobs_private",
                "content":"PRIVATE_DURABLE_TRACE_CANARY\n"
            })),
            approval_status: AgentApprovalStatus::Required,
            reason: None,
        };

        let projected = ApplyPatchTool.trace_call_projection(&call);
        assert_eq!(
            crate::file_change::proposal_digest(&projected.args).unwrap(),
            crate::file_change_support::apply_patch_trace_args_digest(&call.args).unwrap()
        );
        assert_eq!(projected.args["request"]["action"], "apply");
        assert_eq!(projected.args["request"]["changeRepresentation"], "content");
        assert!(projected.args["request"].get("contentDigest").is_some());
        assert!(projected.args["request"].get("observationId").is_none());
        assert!(!projected
            .args
            .to_string()
            .contains("PRIVATE_DURABLE_TRACE_CANARY"));

        let binary_call = AgentToolCall {
            args: wire(json!({
                "action":"apply",
                "operation":"create",
                "filePath":"binary.txt",
                "observationId":"fobs_binary",
                "content":"data:text/plain;base64,UFJJVkFURV9CSU5BUllfQ0FOQVJZ"
            })),
            ..call
        };
        let binary_projection = ApplyPatchTool.trace_call_projection(&binary_call);
        assert_eq!(
            crate::file_change::proposal_digest(&binary_projection.args).unwrap(),
            crate::file_change_support::apply_patch_trace_args_digest(&binary_call.args).unwrap()
        );
        assert!(!binary_projection
            .args
            .to_string()
            .contains("UFJJVkFURV9CSU5BUllfQ0FOQVJZ"));
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
                args: wire(args),
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
