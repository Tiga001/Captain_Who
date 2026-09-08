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
    AgentFileChangeResult, AgentFileChangeResultStatus, AgentGitDiffSnapshot, AgentProposedAction,
    AgentResult, AgentToolCall, AgentToolDefinition, AgentToolResult, AgentToolSafety,
    AgentWritePermission, AGENT_FILE_CHANGE_PROTOCOL_SCHEMA_VERSION,
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

mod request;
mod result;
mod schema;
mod support;

use request::*;
pub(crate) use request::{apply_patch_action, apply_patch_request, apply_patch_wire_is_valid};
use result::*;
pub(crate) use result::{
    attach_successor_observation_to_model_result,
    attach_successor_observation_to_model_result_with_predecessor,
    attach_successor_observation_to_model_result_with_proposal,
    copy_successor_observation_projection,
};
use schema::patch_input_schema;
use support::*;
pub(super) use support::{
    capture_missing_base_observation, file_change_agent_error, file_change_agent_error_for_path,
    file_change_agent_error_for_transaction, freeze_staged_observed_base, patch_operation,
    sanitize_summary, state_revision, validate_observation_operation,
};

pub(super) struct ApplyPatchTool;

impl AgentTool for ApplyPatchTool {
    fn exposure(&self) -> super::AgentToolExposure {
        super::AgentToolExposure::Stable
    }

    fn definition(&self) -> AgentToolDefinition {
        AgentToolDefinition {
            name: "apply_patch".to_string(),
            description: "Create, update, or delete one UTF-8 text file. Put one operation in request using only its schema branch. create (apply or begin) omits observationId and needs no prior read: the Host verifies the exact target is missing and creates it with atomic no-clobber; success returns its first fileChangeTarget. For update/delete or begin/update, copy the exact target's fileChangeTarget.filePath and observationId from read_file or a successful apply/commit in this run; read the target first if no reusable observation is available. Within the current run, a successful update/delete or Staged update commit renews the same observationId to the verified post-write state; reuse it only after receiving the successful Tool Result, never for multiple writes in the same Provider Tool Call batch. Do not reread solely for another ID. If fileChangeTarget is absent, observationRefreshRequired=true, current contents are unknown, or a match/file conflict occurs, follow continueWith and read_file again before correcting the change. Failure, rejection, cancellation, conflict and outcome_unknown do not renew observations. Direct create: {\"request\":{\"action\":\"apply\",\"operation\":\"create\",\"filePath\":\"notes.txt\",\"content\":\"hello\\n\"}}. For larger content or multi-step assembly, use begin, append/edit, then commit. Copy the latest Host transactionId, nextIndex and draftRevision exactly; never invent them or replay persisted chunks. Use status for authoritative cursors and follow allowedNextActions. append/edit changes only the draft; only a successful commit issues or renews fileChangeTarget. Staged delete is unsupported. Settle every unfinished transaction with commit or abort before user-visible narration. Without a workspace, filePath must be an authorized absolute path or @home/@desktop/@documents/@downloads. Never bypass file-change approval with run_command, redirection or scripts."
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
        non_model_file_change_result_projection(result)
    }

    fn checkpoint_projection(&self, result: &AgentToolResult) -> AgentToolResult {
        file_change_staged::public_result_projection(result)
    }

    fn trace_projection(&self, result: &AgentToolResult) -> AgentToolResult {
        without_successor_observation_projection(result)
    }

    fn archive_projection(&self, result: &AgentToolResult) -> AgentToolResult {
        without_successor_observation_projection(result)
    }
}

#[cfg(test)]
mod tests;
