//! Temporary round-3 compatibility entry for the already-published `write_file` Tool.
//!
//! This module owns no draft model, repository, or recovery state machine. Every phase is translated
//! into the canonical FileChange staged transaction implemented by `file_change_staged`; round 4
//! removes this public adapter after provider/prompt migration is complete.

use super::apply_patch::{domain_edit, file_change_agent_error, StructuredTextEdit};
use super::write_file_stream::FileChangeInputStreamObserver;
use super::{
    file_change_staged, AgentTool, AgentToolPermissionPolicy, FileWriteToolAccess,
    ToolExecutionContext, ToolInputStreamObserver,
};
use crate::file_change::{FileChangeError, FileChangeErrorCode};
use crate::protocol::{
    AgentError, AgentFileWriteMode, AgentProposedAction, AgentResult, AgentToolCall,
    AgentToolDefinition, AgentToolResult, AgentToolSafety,
};
use serde::Deserialize;
use serde_json::{json, Value};

pub(super) struct WriteFileTool;

impl AgentTool for WriteFileTool {
    fn exposure(&self) -> super::AgentToolExposure {
        super::AgentToolExposure::Stable
    }

    fn definition(&self) -> AgentToolDefinition {
        AgentToolDefinition {
            name: "write_file".to_string(),
            description: "Temporary compatibility entry backed by the same canonical FileChange transaction/store as apply_patch. Do not start new work with this tool; use apply_patch action=begin/append/edit/commit/status/abort. Existing in-flight write_file transactions may still be continued and settled through phase=append/edit/finish/status/abort until the next upgrade round."
                .to_string(),
            input_schema: input_schema(),
            safety: AgentToolSafety::RequiresApproval,
            requires_workspace: false,
            requires_approval: true,
            approval_mode: crate::protocol::AgentToolApprovalMode::Dynamic,
        }
    }

    fn execute(&self, context: &ToolExecutionContext, args: Value) -> AgentResult<Value> {
        let source_args_digest = crate::file_change::proposal_digest(&args).map_err(|error| {
            file_change_agent_error(FileChangeError::with_diagnostic(
                FileChangeErrorCode::Failed,
                error.to_string(),
            ))
        })?;
        match parse_args(args)? {
            WriteFileArgs::Begin {
                file_path,
                mode,
                summary,
            } => {
                let result = file_change_staged::begin_write_file_bridge(
                    context,
                    mode,
                    file_path,
                    source_args_digest,
                    summary,
                )?;
                bridge_result_from_value(context, result)
            }
            WriteFileArgs::Append {
                draft_id,
                index,
                content,
            } => {
                // Canonical transactions start at revision/index zero and advance both exactly
                // once per mixed append/edit mutation. The legacy wire exposes only append.index.
                file_change_staged::append(
                    context,
                    "write_file",
                    draft_id.clone(),
                    index,
                    index,
                    content,
                    source_args_digest,
                )?;
                file_change_staged::write_file_bridge_result(context, &draft_id)
            }
            WriteFileArgs::Edit { draft_id, edits } => {
                let (revision, index) =
                    file_change_staged::write_file_bridge_cursor(context, &draft_id)?;
                let edits = edits
                    .into_iter()
                    .map(domain_edit)
                    .collect::<Result<Vec<_>, _>>()
                    .map_err(file_change_agent_error)?;
                file_change_staged::edit(
                    context,
                    "write_file",
                    draft_id.clone(),
                    index,
                    revision,
                    edits,
                    source_args_digest,
                )?;
                file_change_staged::write_file_bridge_result(context, &draft_id)
            }
            WriteFileArgs::Status { draft_id } => {
                file_change_staged::status(context, "write_file", draft_id.clone())?;
                file_change_staged::write_file_bridge_result(context, &draft_id)
            }
            WriteFileArgs::Abort { draft_id } => {
                file_change_staged::abort(context, "write_file", draft_id.clone())?;
                file_change_staged::write_file_bridge_result(context, &draft_id)
            }
            WriteFileArgs::Finish { .. } => Err(AgentError::new(
                "write_file phase=finish 需要通过 FileChange 审批边界。",
            )),
        }
    }

    fn permission_policy(&self) -> AgentToolPermissionPolicy {
        AgentToolPermissionPolicy::FileWrite(FileWriteToolAccess::ReadWrite)
    }

    fn proposed_action(
        &self,
        context: &ToolExecutionContext,
        call: &AgentToolCall,
    ) -> AgentResult<AgentProposedAction> {
        let WriteFileArgs::Finish { draft_id, summary } = parse_args(call.args.clone())? else {
            return Err(AgentError::new(
                "只有 write_file phase=finish 可以产生 FileChange 审批提案。",
            ));
        };
        let (revision, _) = file_change_staged::write_file_bridge_cursor(context, &draft_id)?;
        Ok(AgentProposedAction::FileWrite {
            file_write: file_change_staged::commit(
                context,
                call,
                "write_file",
                draft_id,
                revision,
                summary,
            )?,
        })
    }

    fn requires_approval_for_call(&self, args: &Value) -> bool {
        args.get("phase").and_then(Value::as_str) == Some("finish")
    }

    fn input_stream_observer(
        &self,
        context: ToolExecutionContext,
    ) -> Option<Box<dyn ToolInputStreamObserver>> {
        Some(Box::new(FileChangeInputStreamObserver::write_file(context)))
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
                (
                    edits.len(),
                    crate::file_change::proposal_digest(&Value::Array(edits.clone())).ok(),
                )
            })
        }) {
            args.insert("editCount".to_string(), json!(edits.0));
            if let Some(digest) = edits.1 {
                args.insert("editsDigest".to_string(), json!(digest));
            }
        }
        projection
    }

    fn model_projection(&self, result: &AgentToolResult) -> AgentToolResult {
        write_file_model_projection(result)
    }

    fn event_projection(&self, result: &AgentToolResult) -> AgentToolResult {
        file_change_staged::public_result_projection(result)
    }

    fn checkpoint_projection(&self, result: &AgentToolResult) -> AgentToolResult {
        file_change_staged::public_result_projection(result)
    }
}

pub(super) fn write_file_model_projection(result: &AgentToolResult) -> AgentToolResult {
    let projected = result.result.as_ref().and_then(|value| {
        if let Some(draft) = value.get("draft") {
            let mut output = serde_json::Map::new();
            if let Some(draft) = super::model_projection::retain_object_fields(
                draft,
                &[
                    "draftId",
                    "filePath",
                    "mode",
                    "status",
                    "lineCount",
                    "byteCount",
                    "chunkCount",
                    "nextChunkIndex",
                    "statsFinal",
                    "summary",
                ],
            ) {
                output.insert("draft".to_string(), draft);
            }
            for field in [
                "tail",
                "totalChars",
                "tailStart",
                "tailTruncated",
                "transactionState",
                "requiresFinishBeforeResponse",
                "nextAction",
            ] {
                super::model_projection::insert_field(&mut output, value, field);
            }
            return (!output.is_empty()).then_some(Value::Object(output));
        }
        super::model_projection::retain_object_fields(
            value,
            &[
                "status",
                "draftId",
                "filePath",
                "mode",
                "additions",
                "deletions",
                "lineCount",
                "byteCount",
                "error",
                "message",
            ],
        )
    });
    super::model_projection::compact_model_result(result, projected)
}

fn bridge_result_from_value(context: &ToolExecutionContext, value: Value) -> AgentResult<Value> {
    let transaction_id = value
        .get("transactionId")
        .and_then(Value::as_str)
        .ok_or_else(|| {
            file_change_agent_error(FileChangeError::new(FileChangeErrorCode::Failed))
        })?;
    file_change_staged::write_file_bridge_result(context, transaction_id)
}

#[derive(Debug, Deserialize)]
#[serde(
    tag = "phase",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
enum WriteFileArgs {
    Begin {
        file_path: String,
        mode: AgentFileWriteMode,
        summary: Option<String>,
    },
    Append {
        draft_id: String,
        index: u64,
        content: String,
    },
    Edit {
        draft_id: String,
        edits: Vec<StructuredTextEdit>,
    },
    Finish {
        draft_id: String,
        summary: Option<String>,
    },
    Status {
        draft_id: String,
    },
    Abort {
        draft_id: String,
    },
}

fn parse_args(value: Value) -> AgentResult<WriteFileArgs> {
    serde_json::from_value(value).map_err(|_| {
        file_change_agent_error(FileChangeError::new(FileChangeErrorCode::InvalidArguments))
    })
}

fn input_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "phase": { "type": "string", "enum": ["begin", "append", "edit", "finish", "status", "abort"] },
            "draftId": { "type": "string" },
            "filePath": { "type": "string" },
            "mode": { "type": "string", "enum": ["create", "rewrite", "modify", "append", "upsert"] },
            "index": { "type": "integer", "minimum": 0 },
            "content": { "type": "string", "maxLength": 1048576 },
            "edits": {
                "type": "array",
                "minItems": 1,
                "maxItems": 128,
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
            "summary": { "type": "string" }
        },
        "required": ["phase"],
        "additionalProperties": false
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bridge_wire_is_strict_and_does_not_add_new_aliases() {
        assert!(parse_args(json!({
            "phase": "append",
            "draftId": "file-change-staged-v1:test",
            "index": 0,
            "content": "x"
        }))
        .is_ok());
        for invalid in [
            json!({"phase":"append","draft_id":"x","index":0,"content":"x"}),
            json!({"phase":"append","draftId":"x","index":0,"content":"x","extra":true}),
            json!({"phase":"append","draftId":"x","index":null,"content":"x"}),
        ] {
            assert!(parse_args(invalid).is_err());
        }
    }

    #[test]
    fn bridge_schema_is_bounded_and_closed() {
        let schema = input_schema();
        assert_eq!(schema["additionalProperties"], false);
        assert_eq!(schema["properties"]["content"]["maxLength"], 1024 * 1024);
    }
}
