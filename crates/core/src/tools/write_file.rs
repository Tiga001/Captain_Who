use super::apply_patch_paths::sanitize_file_path;
use super::write_file_stream::WriteFileInputStreamObserver;
use super::{
    AgentTool, AgentToolPermissionPolicy, FileWriteToolAccess, ToolExecutionContext,
    ToolInputStreamObserver,
};
use crate::content_revision;
use crate::protocol::{
    AgentApprovalStatus, AgentError, AgentFileDraftSnapshot, AgentFileDraftStatus,
    AgentFileWriteMode, AgentFileWriteProposal, AgentProposedAction, AgentResult, AgentToolCall,
    AgentToolDefinition, AgentToolResult, AgentToolSafety,
};
use crate::storage::models::{
    AgentFileDraftChunkRecord, AgentFileDraftOperationRecord, AgentFileDraftRecord,
};
use crate::storage::now_ms;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use similar::{ChangeTag, TextDiff};
use std::fs;
use std::path::{Path, PathBuf};
use uuid::Uuid;

const MAX_DRAFT_BYTES: usize = 4 * 1024 * 1024;
const MAX_EDITS: usize = 128;
const MAX_SUMMARY_CHARS: usize = 2_000;
const RESULT_TAIL_CHARS: usize = 1_000;
const DRAFT_TTL_MS: i64 = 7 * 24 * 60 * 60 * 1_000;

pub(super) struct WriteFileTool;

impl AgentTool for WriteFileTool {
    fn exposure(&self) -> super::AgentToolExposure {
        super::AgentToolExposure::Stable
    }

    fn definition(&self) -> AgentToolDefinition {
        AgentToolDefinition {
            name: "write_file".to_string(),
            description: "Create and update UTF-8 text files through a persistent transaction draft. Use phase=begin once, then phase=append for generated content or phase=edit for structured draft edits, and phase=finish once to request approval and apply the real workspace write. After begin/append/edit, user-visible text is forbidden until every dirty draft has a finish or abort result. A finish result is returned only after automatic or manual approval completes.".to_string(),
            input_schema: input_schema(),
            safety: AgentToolSafety::RequiresApproval,
            requires_workspace: false,
            requires_approval: true,
            approval_mode: crate::protocol::AgentToolApprovalMode::Dynamic,
        }
    }

    fn execute(&self, context: &ToolExecutionContext, args: Value) -> AgentResult<Value> {
        let args = parse_args(args)?;
        match args.phase {
            WriteFilePhase::Begin => begin_draft(context, args),
            WriteFilePhase::Append => append_draft(context, args),
            WriteFilePhase::Edit => edit_draft(context, args),
            WriteFilePhase::Status => status_draft(context, args),
            WriteFilePhase::Abort => abort_draft(context, args),
            WriteFilePhase::Finish => Err(AgentError::new(
                "write_file finish 必须通过文件写入提案执行。",
            )),
        }
    }

    fn permission_policy(&self) -> AgentToolPermissionPolicy {
        AgentToolPermissionPolicy::FileWrite(FileWriteToolAccess::WriteOnly)
    }

    fn proposed_action(
        &self,
        context: &ToolExecutionContext,
        call: &AgentToolCall,
    ) -> AgentResult<AgentProposedAction> {
        let args = parse_args(call.args.clone())?;
        if args.phase != WriteFilePhase::Finish {
            return Err(AgentError::new(
                "只有 write_file phase=finish 可以产生审批提案。",
            ));
        }
        Ok(AgentProposedAction::FileWrite {
            file_write: finish_draft(context, call, args)?,
        })
    }

    fn requires_approval_for_call(&self, args: &Value) -> bool {
        args.get("phase").and_then(Value::as_str) == Some("finish")
    }

    fn input_stream_observer(
        &self,
        context: ToolExecutionContext,
    ) -> Option<Box<dyn ToolInputStreamObserver>> {
        Some(Box::new(WriteFileInputStreamObserver::new(context)))
    }

    fn event_call_projection(&self, call: &AgentToolCall) -> AgentToolCall {
        let mut projection = call.clone();
        let Some(args) = projection.args.as_object_mut() else {
            return projection;
        };
        if let Some(content) = args.get("content").and_then(Value::as_str) {
            let content_bytes = content.len() as u64;
            args.insert("content".to_string(), json!("[stored in private draft]"));
            args.insert("contentBytes".to_string(), json!(content_bytes));
        }
        projection
    }

    fn model_projection(&self, result: &AgentToolResult) -> AgentToolResult {
        write_file_model_projection(result)
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

fn input_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "phase": {
                "type": "string",
                "enum": ["begin", "append", "edit", "finish", "status", "abort"],
                "description": "Draft lifecycle phase. begin needs filePath/mode; append needs draftId/index/content; edit needs draftId/edits; finish/status/abort need draftId."
            },
            "draftId": { "type": "string", "description": "Draft id returned by phase=begin." },
            "filePath": { "type": "string", "description": "Workspace-relative or permitted absolute text-file path. Used only by phase=begin." },
            "mode": {
                "type": "string",
                "enum": ["create", "rewrite", "modify", "append", "upsert"],
                "description": "create requires a missing target; rewrite starts an empty replacement for an existing file; modify starts from existing content; append starts from existing content; upsert creates or rewrites."
            },
            "index": { "type": "integer", "minimum": 0, "description": "Zero-based append index. Use nextChunkIndex from the previous result." },
            "content": { "type": "string", "description": "UTF-8 content to append to the private draft during phase=append." },
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
                    "required": ["kind"]
                }
            },
            "summary": { "type": "string", "description": "Short description of the file being generated or edited." }
        },
        "required": ["phase"]
    })
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct WriteFileArgs {
    phase: WriteFilePhase,
    draft_id: Option<String>,
    file_path: Option<String>,
    mode: Option<AgentFileWriteMode>,
    index: Option<u64>,
    content: Option<String>,
    edits: Option<Vec<StructuredTextEdit>>,
    summary: Option<String>,
}

#[derive(Debug, Deserialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum WriteFilePhase {
    Begin,
    Append,
    Edit,
    Finish,
    Status,
    Abort,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct StructuredTextEdit {
    kind: TextEditKind,
    old_text: Option<String>,
    new_text: Option<String>,
    anchor: Option<String>,
    text: Option<String>,
    replace_all: Option<bool>,
}

#[derive(Debug, Deserialize, Serialize, Clone, Copy)]
#[serde(rename_all = "snake_case")]
enum TextEditKind {
    Replace,
    InsertBefore,
    InsertAfter,
    Append,
    Prepend,
}

fn parse_args(value: Value) -> AgentResult<WriteFileArgs> {
    serde_json::from_value(value)
        .map_err(|error| AgentError::new(format!("write_file 参数无效：{error}")))
}

fn begin_draft(context: &ToolExecutionContext, args: WriteFileArgs) -> AgentResult<Value> {
    let storage = context.storage()?;
    let raw_file_path = required_string(args.file_path, "begin 需要 filePath")?;
    let file_path = sanitize_file_path(&raw_file_path, context.permissions().write)?;
    let mode = args
        .mode
        .ok_or_else(|| AgentError::new("write_file begin 需要 mode。"))?;
    let target = resolve_target(context, &file_path)?;
    let target_exists = target.exists();
    let base_content = if target_exists {
        read_target_text(&target)?
    } else {
        String::new()
    };

    match mode {
        AgentFileWriteMode::Create if target_exists => {
            return Err(AgentError::new(
                "write_file create 要求目标文件当前不存在。",
            ));
        }
        AgentFileWriteMode::Rewrite | AgentFileWriteMode::Modify | AgentFileWriteMode::Append
            if !target_exists =>
        {
            return Err(AgentError::new("write_file 当前模式要求目标文件已经存在。"));
        }
        _ => {}
    }

    let content = match mode {
        AgentFileWriteMode::Modify | AgentFileWriteMode::Append => base_content.clone(),
        AgentFileWriteMode::Create | AgentFileWriteMode::Rewrite | AgentFileWriteMode::Upsert => {
            String::new()
        }
    };
    let (additions, deletions) = diff_counts(&base_content, &content);
    let now = now_ms();
    let draft = AgentFileDraftRecord {
        id: format!("draft-{}", Uuid::new_v4()),
        conversation_id: context.conversation_id()?.to_string(),
        project_id: context.project_id().map(ToString::to_string),
        run_id: context.run_id()?.to_string(),
        file_path,
        mode: mode_label(mode).to_string(),
        status: "drafting".to_string(),
        base_revision: target_exists.then(|| content_revision(base_content.as_bytes())),
        base_content,
        content,
        additions,
        deletions,
        line_count: 0,
        byte_count: 0,
        chunk_count: 0,
        next_chunk_index: 0,
        stats_final: false,
        summary: sanitize_summary(args.summary),
        final_action_id: None,
        created_at: now,
        updated_at: now,
        expires_at: now.saturating_add(DRAFT_TTL_MS),
    };
    let mut draft = refresh_metrics(draft);
    draft.stats_final = false;
    storage
        .create_agent_file_draft(draft.clone())
        .map_err(AgentError::new)?;
    Ok(draft_result(&draft))
}

fn append_draft(context: &ToolExecutionContext, args: WriteFileArgs) -> AgentResult<Value> {
    let draft_id = required_string(args.draft_id, "append 需要 draftId")?;
    let index = args
        .index
        .ok_or_else(|| AgentError::new("write_file append 需要 index。"))?;
    let content = args
        .content
        .ok_or_else(|| AgentError::new("write_file append 需要 content。"))?;
    if content.is_empty() {
        return Err(AgentError::new("write_file append.content 不能为空。"));
    }
    reject_nul(&content)?;

    let storage = context.storage()?;
    let mut draft = load_owned_draft(context, &draft_id)?;
    ensure_mutable(&draft)?;
    let chunk_hash = content_revision(content.as_bytes());
    if index < draft.next_chunk_index {
        let existing_hash = storage
            .get_agent_file_draft_chunk_hash(&draft_id, index)
            .map_err(AgentError::new)?;
        if existing_hash.as_deref() == Some(chunk_hash.as_str()) {
            return Ok(draft_result(&draft));
        }
        return Err(AgentError::new(format!(
            "write_file chunk {index} 已存在，但内容不同；当前 nextChunkIndex={}。",
            draft.next_chunk_index
        )));
    }
    if index != draft.next_chunk_index {
        return Err(AgentError::new(format!(
            "write_file append.index 应为 {}，实际为 {index}。",
            draft.next_chunk_index
        )));
    }
    if draft.content.len().saturating_add(content.len()) > MAX_DRAFT_BYTES {
        return Err(AgentError::new(format!(
            "write_file 草稿超过 {} bytes 限制。",
            MAX_DRAFT_BYTES
        )));
    }

    draft.content.push_str(&content);
    draft.chunk_count = draft.chunk_count.saturating_add(1);
    draft.next_chunk_index = draft.next_chunk_index.saturating_add(1);
    draft.status = "drafting".to_string();
    draft.stats_final = false;
    draft.updated_at = now_ms();
    draft.expires_at = draft.updated_at.saturating_add(DRAFT_TTL_MS);
    draft = refresh_metrics(draft);
    let chunk = AgentFileDraftChunkRecord {
        draft_id: draft_id.clone(),
        chunk_index: index,
        content_hash: chunk_hash.clone(),
        byte_count: content.len() as u64,
        created_at: draft.updated_at,
    };
    let operation = AgentFileDraftOperationRecord {
        draft_id,
        sequence: storage
            .next_agent_file_draft_operation_sequence(&draft.id)
            .map_err(AgentError::new)?,
        operation: "append".to_string(),
        payload_hash: chunk_hash,
        created_at: draft.updated_at,
    };
    storage
        .save_agent_file_draft_progress(&draft, Some(&chunk), Some(&operation))
        .map_err(AgentError::new)?;
    Ok(draft_result(&draft))
}

fn edit_draft(context: &ToolExecutionContext, args: WriteFileArgs) -> AgentResult<Value> {
    let draft_id = required_string(args.draft_id, "edit 需要 draftId")?;
    let edits = args
        .edits
        .filter(|edits| !edits.is_empty())
        .ok_or_else(|| AgentError::new("write_file edit 需要至少一个 edit。"))?;
    if edits.len() > MAX_EDITS {
        return Err(AgentError::new(format!(
            "write_file edit 一次最多接受 {MAX_EDITS} 个编辑。"
        )));
    }
    let storage = context.storage()?;
    let mut draft = load_owned_draft(context, &draft_id)?;
    ensure_mutable(&draft)?;
    let payload = serde_json::to_vec(&edits)
        .map_err(|error| AgentError::new(format!("write_file edits 序列化失败：{error}")))?;
    let updated = apply_structured_edits(&draft.content, &edits)?;
    if updated.len() > MAX_DRAFT_BYTES {
        return Err(AgentError::new(format!(
            "write_file 草稿超过 {} bytes 限制。",
            MAX_DRAFT_BYTES
        )));
    }
    draft.content = updated;
    draft.status = "drafting".to_string();
    draft.stats_final = false;
    draft.updated_at = now_ms();
    draft.expires_at = draft.updated_at.saturating_add(DRAFT_TTL_MS);
    draft = refresh_metrics(draft);
    let operation = AgentFileDraftOperationRecord {
        draft_id: draft.id.clone(),
        sequence: storage
            .next_agent_file_draft_operation_sequence(&draft.id)
            .map_err(AgentError::new)?,
        operation: "edit".to_string(),
        payload_hash: content_revision(&payload),
        created_at: draft.updated_at,
    };
    storage
        .save_agent_file_draft_progress(&draft, None, Some(&operation))
        .map_err(AgentError::new)?;
    Ok(draft_result(&draft))
}

fn status_draft(context: &ToolExecutionContext, args: WriteFileArgs) -> AgentResult<Value> {
    let draft_id = required_string(args.draft_id, "status 需要 draftId")?;
    Ok(draft_result(&load_owned_draft(context, &draft_id)?))
}

fn abort_draft(context: &ToolExecutionContext, args: WriteFileArgs) -> AgentResult<Value> {
    let draft_id = required_string(args.draft_id, "abort 需要 draftId")?;
    let storage = context.storage()?;
    let mut draft = load_owned_draft(context, &draft_id)?;
    ensure_mutable(&draft)?;
    draft.status = "aborted".to_string();
    draft.updated_at = now_ms();
    storage
        .update_agent_file_draft(&draft)
        .map_err(AgentError::new)?;
    Ok(draft_result(&draft))
}

fn finish_draft(
    context: &ToolExecutionContext,
    call: &AgentToolCall,
    args: WriteFileArgs,
) -> AgentResult<AgentFileWriteProposal> {
    let draft_id = required_string(args.draft_id, "finish 需要 draftId")?;
    let storage = context.storage()?;
    let mut draft = load_owned_draft(context, &draft_id)?;
    ensure_mutable(&draft)?;
    draft.status = "waiting_approval".to_string();
    draft.stats_final = true;
    draft.final_action_id = Some(call.id.clone());
    if let Some(summary) = sanitize_summary(args.summary) {
        draft.summary = Some(summary);
    }
    draft.updated_at = now_ms();
    draft = refresh_metrics(draft);
    storage
        .update_agent_file_draft(&draft)
        .map_err(AgentError::new)?;
    Ok(AgentFileWriteProposal {
        id: call.id.clone(),
        draft_id: draft.id.clone(),
        mode: parse_mode(&draft.mode)?,
        file_path: draft.file_path.clone(),
        base_revision: draft.base_revision.clone(),
        summary: draft.summary.clone(),
        additions: draft.additions,
        deletions: draft.deletions,
        line_count: draft.line_count,
        byte_count: draft.byte_count,
        approval_status: AgentApprovalStatus::Required,
    })
}

fn load_owned_draft(
    context: &ToolExecutionContext,
    draft_id: &str,
) -> AgentResult<AgentFileDraftRecord> {
    let draft = context
        .storage()?
        .get_agent_file_draft(draft_id)
        .map_err(AgentError::new)?
        .ok_or_else(|| AgentError::new(format!("未找到 write_file 草稿：{draft_id}")))?;
    if draft.conversation_id != context.conversation_id()? {
        return Err(AgentError::new("write_file 草稿不属于当前对话。"));
    }
    Ok(draft)
}

fn ensure_mutable(draft: &AgentFileDraftRecord) -> AgentResult<()> {
    match draft.status.as_str() {
        "drafting" | "ready" => Ok(()),
        "waiting_approval" => Err(AgentError::new("草稿正在等待审批，不能继续修改。")),
        "applying" => Err(AgentError::new("草稿正在应用。")),
        "applied" | "rejected" | "conflict" | "failed" | "aborted" | "expired" => {
            Err(AgentError::new(
                "write_file 草稿已经结算，不能继续修改或重复提交；后续写入请重新 phase=begin。",
            ))
        }
        _ => Err(AgentError::new("草稿状态不可修改。")),
    }
}

fn resolve_target(context: &ToolExecutionContext, file_path: &str) -> AgentResult<PathBuf> {
    let path = Path::new(file_path);
    let target = if path.is_absolute() {
        path.to_path_buf()
    } else {
        context.workspace_root()?.join(path)
    };
    let parent = target
        .parent()
        .ok_or_else(|| AgentError::new("write_file 目标路径缺少父目录。"))?;
    let canonical_parent = parent
        .canonicalize()
        .map_err(|error| AgentError::new(format!("write_file 父目录不可访问：{error}")))?;
    if !path.is_absolute() {
        let root = context.workspace_root()?;
        if !canonical_parent.starts_with(&root) {
            return Err(AgentError::new("write_file 目标必须位于 workspace 内。"));
        }
    }
    let resolved = canonical_parent.join(
        target
            .file_name()
            .ok_or_else(|| AgentError::new("write_file 目标缺少文件名。"))?,
    );
    if let Ok(metadata) = fs::symlink_metadata(&resolved) {
        if metadata.file_type().is_symlink() {
            return Err(AgentError::new("write_file 不允许写入符号链接目标。"));
        }
        if !metadata.is_file() {
            return Err(AgentError::new("write_file 目标不是普通文件。"));
        }
    }
    Ok(resolved)
}

fn read_target_text(path: &Path) -> AgentResult<String> {
    fs::read_to_string(path)
        .map_err(|error| AgentError::new(format!("读取 write_file 目标失败：{error}")))
}

fn refresh_metrics(mut draft: AgentFileDraftRecord) -> AgentFileDraftRecord {
    let (additions, deletions) = diff_counts(&draft.base_content, &draft.content);
    draft.additions = additions;
    draft.deletions = deletions;
    draft.line_count = text_line_count(&draft.content);
    draft.byte_count = draft.content.len() as u64;
    draft
}

fn diff_counts(base: &str, current: &str) -> (u64, u64) {
    let diff = TextDiff::from_lines(base, current);
    let mut additions = 0_u64;
    let mut deletions = 0_u64;
    for change in diff.iter_all_changes() {
        match change.tag() {
            ChangeTag::Insert => additions = additions.saturating_add(1),
            ChangeTag::Delete => deletions = deletions.saturating_add(1),
            ChangeTag::Equal => {}
        }
    }
    (additions, deletions)
}

fn text_line_count(content: &str) -> u64 {
    if content.is_empty() {
        0
    } else {
        content.lines().count() as u64
    }
}

fn draft_result(draft: &AgentFileDraftRecord) -> Value {
    let snapshot = snapshot_from_record(draft).unwrap_or_else(|_| AgentFileDraftSnapshot {
        draft_id: draft.id.clone(),
        conversation_id: draft.conversation_id.clone(),
        project_id: draft.project_id.clone(),
        file_path: draft.file_path.clone(),
        mode: AgentFileWriteMode::Upsert,
        status: AgentFileDraftStatus::Failed,
        base_revision: draft.base_revision.clone(),
        additions: draft.additions,
        deletions: draft.deletions,
        line_count: draft.line_count,
        byte_count: draft.byte_count,
        chunk_count: draft.chunk_count,
        next_chunk_index: draft.next_chunk_index,
        stats_final: draft.stats_final,
        summary: draft.summary.clone(),
        created_at: draft.created_at,
        updated_at: draft.updated_at,
    });
    let (tail, total_chars, tail_start) = tail_preview(&draft.content, RESULT_TAIL_CHARS);
    json!({
        "draft": snapshot,
        "tail": tail,
        "totalChars": total_chars,
        "tailStart": tail_start,
        "tailTruncated": tail_start > 0,
        "maxDraftBytes": MAX_DRAFT_BYTES,
        "transactionState": if matches!(draft.status.as_str(), "drafting" | "ready") { "dirty" } else { "settled" },
        "requiresFinishBeforeResponse": matches!(draft.status.as_str(), "drafting" | "ready"),
        "nextAction": if matches!(draft.status.as_str(), "drafting" | "ready") {
            "Continue write_file tool calls, then call phase=finish or phase=abort before emitting user-visible text."
        } else {
            "Do not modify this settled draft again. Call phase=begin for any later file transaction."
        }
    })
}

pub(crate) fn snapshot_from_record(
    draft: &AgentFileDraftRecord,
) -> AgentResult<AgentFileDraftSnapshot> {
    Ok(AgentFileDraftSnapshot {
        draft_id: draft.id.clone(),
        conversation_id: draft.conversation_id.clone(),
        project_id: draft.project_id.clone(),
        file_path: draft.file_path.clone(),
        mode: parse_mode(&draft.mode)?,
        status: parse_status(&draft.status)?,
        base_revision: draft.base_revision.clone(),
        additions: draft.additions,
        deletions: draft.deletions,
        line_count: draft.line_count,
        byte_count: draft.byte_count,
        chunk_count: draft.chunk_count,
        next_chunk_index: draft.next_chunk_index,
        stats_final: draft.stats_final,
        summary: draft.summary.clone(),
        created_at: draft.created_at,
        updated_at: draft.updated_at,
    })
}

fn parse_mode(value: &str) -> AgentResult<AgentFileWriteMode> {
    match value {
        "create" => Ok(AgentFileWriteMode::Create),
        "rewrite" => Ok(AgentFileWriteMode::Rewrite),
        "modify" => Ok(AgentFileWriteMode::Modify),
        "append" => Ok(AgentFileWriteMode::Append),
        "upsert" => Ok(AgentFileWriteMode::Upsert),
        _ => Err(AgentError::new(format!("未知 write_file mode：{value}"))),
    }
}

fn mode_label(mode: AgentFileWriteMode) -> &'static str {
    match mode {
        AgentFileWriteMode::Create => "create",
        AgentFileWriteMode::Rewrite => "rewrite",
        AgentFileWriteMode::Modify => "modify",
        AgentFileWriteMode::Append => "append",
        AgentFileWriteMode::Upsert => "upsert",
    }
}

fn parse_status(value: &str) -> AgentResult<AgentFileDraftStatus> {
    match value {
        "drafting" => Ok(AgentFileDraftStatus::Drafting),
        "ready" => Ok(AgentFileDraftStatus::Ready),
        "waiting_approval" => Ok(AgentFileDraftStatus::WaitingApproval),
        "applying" => Ok(AgentFileDraftStatus::Applying),
        "applied" => Ok(AgentFileDraftStatus::Applied),
        "rejected" => Ok(AgentFileDraftStatus::Rejected),
        "conflict" => Ok(AgentFileDraftStatus::Conflict),
        "failed" => Ok(AgentFileDraftStatus::Failed),
        "aborted" => Ok(AgentFileDraftStatus::Aborted),
        "expired" => Ok(AgentFileDraftStatus::Expired),
        _ => Err(AgentError::new(format!("未知文件草稿状态：{value}"))),
    }
}

fn apply_structured_edits(current: &str, edits: &[StructuredTextEdit]) -> AgentResult<String> {
    let mut updated = current.to_string();
    for edit in edits {
        updated = match edit.kind {
            TextEditKind::Replace => {
                let old_text = required_text(edit.old_text.as_deref(), "replace.oldText")?;
                let new_text = edit.new_text.as_deref().unwrap_or_default();
                let matches = updated.match_indices(old_text).count();
                if matches == 0 {
                    return Err(AgentError::new("write_file replace.oldText 未找到。"));
                }
                if edit.replace_all.unwrap_or(false) {
                    updated.replace(old_text, new_text)
                } else {
                    if matches != 1 {
                        return Err(AgentError::new(format!(
                            "write_file replace.oldText 出现 {matches} 次，要求唯一匹配。"
                        )));
                    }
                    updated.replacen(old_text, new_text, 1)
                }
            }
            TextEditKind::InsertBefore | TextEditKind::InsertAfter => {
                let anchor = required_text(edit.anchor.as_deref(), "edit.anchor")?;
                let text = edit.text.as_deref().unwrap_or_default();
                let matches = updated.match_indices(anchor).count();
                if matches != 1 {
                    return Err(AgentError::new(format!(
                        "write_file anchor 出现 {matches} 次，要求唯一匹配。"
                    )));
                }
                if matches!(edit.kind, TextEditKind::InsertBefore) {
                    updated.replacen(anchor, &format!("{text}{anchor}"), 1)
                } else {
                    updated.replacen(anchor, &format!("{anchor}{text}"), 1)
                }
            }
            TextEditKind::Append => {
                format!("{}{}", updated, edit.text.as_deref().unwrap_or_default())
            }
            TextEditKind::Prepend => {
                format!("{}{}", edit.text.as_deref().unwrap_or_default(), updated)
            }
        };
    }
    Ok(updated)
}

fn required_string(value: Option<String>, message: &str) -> AgentResult<String> {
    value
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .ok_or_else(|| AgentError::new(message))
}

fn required_text<'a>(value: Option<&'a str>, field: &str) -> AgentResult<&'a str> {
    value
        .filter(|value| !value.is_empty())
        .ok_or_else(|| AgentError::new(format!("write_file {field} 不能为空。")))
}

fn sanitize_summary(summary: Option<String>) -> Option<String> {
    summary
        .map(|summary| {
            summary
                .trim()
                .chars()
                .take(MAX_SUMMARY_CHARS)
                .collect::<String>()
        })
        .filter(|summary| !summary.is_empty())
}

fn reject_nul(content: &str) -> AgentResult<()> {
    if content.contains('\0') {
        Err(AgentError::new("write_file 内容不能包含空字符。"))
    } else {
        Ok(())
    }
}

fn tail_preview(content: &str, limit: usize) -> (String, usize, usize) {
    let total_chars = content.chars().count();
    let tail_start = total_chars.saturating_sub(limit);
    (
        content.chars().skip(tail_start).collect(),
        total_chars,
        tail_start,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{
        AgentCommandPermission, AgentPatchPermission, AgentPermissions, AgentReadPermission,
        AgentRunContext, AgentWorkspaceContext, AgentWritePermission,
    };
    use crate::storage::models::ChatConversationRecord;
    use crate::storage::service::StorageService;
    use std::sync::Arc;
    use tempfile::tempdir;

    #[test]
    fn diff_counts_create_and_replace() {
        assert_eq!(diff_counts("", "a\nb\n"), (2, 0));
        assert_eq!(diff_counts("a\nb\n", "a\nc\n"), (1, 1));
    }

    #[test]
    fn draft_tail_reports_the_exact_preview_range_to_every_result_consumer() {
        let content = "你".repeat(RESULT_TAIL_CHARS + 7);
        let draft = AgentFileDraftRecord {
            id: "draft-tail".to_string(),
            conversation_id: "conversation-1".to_string(),
            project_id: None,
            run_id: "run-1".to_string(),
            file_path: "report.md".to_string(),
            mode: "create".to_string(),
            status: "drafting".to_string(),
            base_revision: None,
            base_content: String::new(),
            content: content.clone(),
            additions: 1,
            deletions: 0,
            line_count: 1,
            byte_count: content.len() as u64,
            chunk_count: 1,
            next_chunk_index: 1,
            stats_final: true,
            summary: None,
            final_action_id: None,
            created_at: 1,
            updated_at: 2,
            expires_at: i64::MAX,
        };
        let value = draft_result(&draft);
        assert_eq!(value["totalChars"], RESULT_TAIL_CHARS + 7);
        assert_eq!(value["tailStart"], 7);
        assert_eq!(value["tailTruncated"], true);
        assert_eq!(
            value["tail"].as_str().unwrap().chars().count(),
            RESULT_TAIL_CHARS
        );

        let canonical = AgentToolResult {
            exact_archive_file: None,
            call_id: "write-tail".to_string(),
            tool: "write_file".to_string(),
            ok: true,
            result: Some(value.clone()),
            error: None,
        };
        let model = write_file_model_projection(&canonical);
        assert_eq!(
            model.result.as_ref().unwrap()["totalChars"],
            value["totalChars"]
        );
        assert_eq!(
            model.result.as_ref().unwrap()["tailStart"],
            value["tailStart"]
        );
        assert_eq!(
            model.result.as_ref().unwrap()["tailTruncated"],
            value["tailTruncated"]
        );
        for projection in [
            WriteFileTool.event_projection(&canonical),
            WriteFileTool.trace_projection(&canonical),
            WriteFileTool.archive_projection(&canonical),
            WriteFileTool.checkpoint_projection(&canonical),
        ] {
            assert_eq!(projection.result.as_ref(), Some(&value));
        }
    }

    #[test]
    fn event_call_projection_redacts_private_draft_without_changing_trace_or_execution() {
        let call = AgentToolCall {
            id: "write-observable".to_string(),
            tool: "write_file".to_string(),
            args: json!({
                "phase": "append",
                "draftId": "draft-1",
                "index": 0,
                "content": "secret draft content",
            }),
            approval_status: AgentApprovalStatus::NotRequired,
            reason: None,
        };

        let trace_projection = WriteFileTool.trace_call_projection(&call);
        let event_projection = WriteFileTool.event_call_projection(&call);

        assert_eq!(call.args["content"], "secret draft content");
        assert_eq!(trace_projection.args["content"], "secret draft content");
        assert!(trace_projection.args.get("contentBytes").is_none());
        assert_eq!(
            event_projection.args["content"],
            "[stored in private draft]"
        );
        assert_eq!(event_projection.args["contentBytes"], 20);
    }

    #[test]
    fn structured_edits_require_unique_anchor() {
        let edits = vec![StructuredTextEdit {
            kind: TextEditKind::Replace,
            old_text: Some("x".to_string()),
            new_text: Some("z".to_string()),
            anchor: None,
            text: None,
            replace_all: Some(false),
        }];
        assert!(apply_structured_edits("x x", &edits).is_err());
        assert_eq!(apply_structured_edits("x y", &edits).unwrap(), "z y");
    }

    #[test]
    fn begin_append_and_finish_share_one_persistent_draft() {
        let fixture = tempdir().unwrap();
        let workspace = fixture.path().join("workspace");
        fs::create_dir_all(&workspace).unwrap();
        let storage = Arc::new(StorageService::open(&fixture.path().join("app.db")).unwrap());
        storage
            .save_conversation(ChatConversationRecord {
                id: "conversation-1".to_string(),
                project_id: None,
                model_id: None,
                title: "Test".to_string(),
                messages: Vec::new(),
                created_at: 1,
                updated_at: 1,
                pinned_at: None,
                archived_at: None,
                unread_at: None,
            })
            .unwrap();
        let context = ToolExecutionContext::from_run_context(Some(&AgentRunContext {
            collaboration_identity: None,
            conversation_id: Some("conversation-1".to_string()),
            project_id: None,
            workspace: Some(AgentWorkspaceContext {
                project_id: None,
                display_name: Some("workspace".to_string()),
                root_path: Some(workspace.to_string_lossy().to_string()),
            }),
            attachment_library: None,
            permissions: AgentPermissions {
                read: AgentReadPermission::WorkspaceOnly,
                write: AgentWritePermission::WorkspaceOnly,
                command: AgentCommandPermission::RequireApproval,
                command_safety: Default::default(),
                patch: AgentPatchPermission::RequireApproval,
                builtin_execution: Default::default(),
            },
        }))
        .with_runtime_services("run-1".to_string(), Some(storage.clone()));
        let tool = WriteFileTool;
        let begin = tool
            .execute(
                &context,
                json!({
                    "phase": "begin",
                    "filePath": "report.md",
                    "mode": "create"
                }),
            )
            .unwrap();
        let draft_id = begin["draft"]["draftId"].as_str().unwrap().to_string();
        let content = format!("# Report\n{}\n", "x".repeat(16 * 1024));
        let appended = tool
            .execute(
                &context,
                json!({
                    "phase": "append",
                    "draftId": draft_id,
                    "index": 0,
                    "content": content
                }),
            )
            .unwrap();
        assert_eq!(appended["draft"]["nextChunkIndex"], 1);
        assert_eq!(appended["draft"]["byteCount"], content.len() as u64);
        assert_eq!(appended["draft"]["additions"], 2);

        let action = tool
            .proposed_action(
                &context,
                &AgentToolCall {
                    id: "call-finish".to_string(),
                    tool: "write_file".to_string(),
                    args: json!({ "phase": "finish", "draftId": draft_id }),
                    approval_status: AgentApprovalStatus::Required,
                    reason: None,
                },
            )
            .unwrap();
        let AgentProposedAction::FileWrite { file_write } = action else {
            panic!("expected file write proposal");
        };
        assert_eq!(file_write.file_path, "report.md");
        assert_eq!(file_write.additions, 2);
        assert_eq!(
            storage
                .get_agent_file_draft(&file_write.draft_id)
                .unwrap()
                .unwrap()
                .status,
            "waiting_approval"
        );
    }

    #[test]
    fn finish_enters_approval_even_when_target_changed_after_begin() {
        let fixture = tempdir().unwrap();
        let workspace = fixture.path().join("workspace");
        fs::create_dir_all(&workspace).unwrap();
        fs::write(workspace.join("report.md"), "before\n").unwrap();
        let storage = Arc::new(StorageService::open(&fixture.path().join("app.db")).unwrap());
        storage
            .save_conversation(ChatConversationRecord {
                id: "conversation-1".to_string(),
                project_id: None,
                model_id: None,
                title: "Test".to_string(),
                messages: Vec::new(),
                created_at: 1,
                updated_at: 1,
                pinned_at: None,
                archived_at: None,
                unread_at: None,
            })
            .unwrap();
        let context = ToolExecutionContext::from_run_context(Some(&AgentRunContext {
            collaboration_identity: None,
            conversation_id: Some("conversation-1".to_string()),
            project_id: None,
            workspace: Some(AgentWorkspaceContext {
                project_id: None,
                display_name: Some("workspace".to_string()),
                root_path: Some(workspace.to_string_lossy().to_string()),
            }),
            attachment_library: None,
            permissions: AgentPermissions {
                read: AgentReadPermission::WorkspaceOnly,
                write: AgentWritePermission::WorkspaceOnly,
                command: AgentCommandPermission::RequireApproval,
                command_safety: Default::default(),
                patch: AgentPatchPermission::RequireApproval,
                builtin_execution: Default::default(),
            },
        }))
        .with_runtime_services("run-1".to_string(), Some(storage.clone()));
        let tool = WriteFileTool;
        let begin = tool
            .execute(
                &context,
                json!({
                    "phase": "begin",
                    "filePath": "report.md",
                    "mode": "rewrite"
                }),
            )
            .unwrap();
        let draft_id = begin["draft"]["draftId"].as_str().unwrap().to_string();
        tool.execute(
            &context,
            json!({
                "phase": "append",
                "draftId": draft_id,
                "index": 0,
                "content": "after\n"
            }),
        )
        .unwrap();
        fs::write(workspace.join("report.md"), "changed outside\n").unwrap();

        let action = tool
            .proposed_action(
                &context,
                &AgentToolCall {
                    id: "call-finish".to_string(),
                    tool: "write_file".to_string(),
                    args: json!({ "phase": "finish", "draftId": draft_id }),
                    approval_status: AgentApprovalStatus::Required,
                    reason: None,
                },
            )
            .unwrap();

        assert!(matches!(action, AgentProposedAction::FileWrite { .. }));
        assert_eq!(
            storage
                .get_agent_file_draft(&draft_id)
                .unwrap()
                .unwrap()
                .status,
            "waiting_approval"
        );
    }

    #[test]
    fn settled_draft_requires_a_new_begin() {
        let draft = AgentFileDraftRecord {
            id: "draft-1".to_string(),
            conversation_id: "conversation-1".to_string(),
            project_id: None,
            run_id: "run-1".to_string(),
            file_path: "report.md".to_string(),
            mode: "create".to_string(),
            status: "rejected".to_string(),
            base_revision: None,
            base_content: String::new(),
            content: "draft".to_string(),
            additions: 1,
            deletions: 0,
            line_count: 1,
            byte_count: 5,
            chunk_count: 1,
            next_chunk_index: 1,
            stats_final: true,
            summary: None,
            final_action_id: Some("call-finish".to_string()),
            created_at: 1,
            updated_at: 2,
            expires_at: i64::MAX,
        };

        let error = ensure_mutable(&draft).unwrap_err().to_string();
        assert!(error.contains("phase=begin"));
    }
}
