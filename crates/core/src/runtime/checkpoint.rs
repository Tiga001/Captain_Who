//! Durable agent-loop checkpoints used at approval boundaries.
//!
//! A checkpoint owns every piece of in-memory state needed to resume the same logical run. The
//! pending tool call already appears as the final, unresolved exchange in `context`; queued calls
//! belong to the same model response but have not started yet. On resume, the approved/rejected
//! result closes the pending exchange before queued calls continue.

use super::tool_failure_guard::semantic_tool_call_fingerprint;
use super::tool_flow::build_tool_observation_message;
use crate::context::{ContextFrame, ContextGroup};
use crate::conversation_trace::{
    canonical_tool_result_for_context, ConversationTraceRecorder, ConversationTraceSnapshot,
    ConversationTurnTraceItem,
};
use crate::llm::{validate_model_tool_call_id, LlmToolCall};
use crate::protocol::{
    AgentContextCheckpointItem, AgentContextCheckpointToolCall, AgentError, AgentExtensionSnapshot,
    AgentQueuedToolCallCheckpoint, AgentResult, AgentRunCheckpoint, AgentToolContinuation,
    AGENT_RUN_CHECKPOINT_SCHEMA_VERSION,
};
use std::collections::{BTreeSet, VecDeque};

#[derive(Debug, Clone)]
pub(super) struct QueuedToolCall {
    pub(super) call: LlmToolCall,
    pub(super) assistant_content: String,
    pub(super) group_id: String,
}

impl QueuedToolCall {
    pub(super) fn context_group(&self) -> ContextGroup {
        ContextGroup::tool_exchange(self.group_id.clone())
    }
}

#[derive(Debug, Clone, Default)]
pub(super) struct ToolCallBatch {
    queue: VecDeque<QueuedToolCall>,
    suppressed_narration: bool,
    /// Semantic calls already accepted from this one model response.
    ///
    /// This is not a global result cache. It only prevents duplicate side effects inside one
    /// provider response. Approval restore reconstructs it from the checkpoint's durable tool
    /// exchange groups, so pausing cannot make a queued duplicate executable again.
    seen_semantic_fingerprints: BTreeSet<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum ToolCallBatchClaim {
    Execute,
    Duplicate { semantic_fingerprint: String },
}

impl ToolCallBatch {
    pub(super) fn from_model_response(
        run_id: &str,
        model_request_index: usize,
        assistant_content: String,
        calls: Vec<LlmToolCall>,
        suppressed_narration: bool,
    ) -> Self {
        let queue = calls
            .into_iter()
            .enumerate()
            .map(|(tool_index, call)| QueuedToolCall {
                call,
                assistant_content: if tool_index == 0 {
                    assistant_content.clone()
                } else {
                    String::new()
                },
                group_id: format!(
                    "run:{run_id}:tool-exchange:{}:{}",
                    model_request_index + 1,
                    tool_index + 1
                ),
            })
            .collect();
        Self {
            queue,
            suppressed_narration,
            seen_semantic_fingerprints: BTreeSet::new(),
        }
    }

    pub(super) fn claim(&mut self, call: &LlmToolCall) -> ToolCallBatchClaim {
        let semantic_fingerprint = semantic_tool_call_fingerprint(&call.name, &call.args);
        if self
            .seen_semantic_fingerprints
            .insert(semantic_fingerprint.clone())
        {
            ToolCallBatchClaim::Execute
        } else {
            ToolCallBatchClaim::Duplicate {
                semantic_fingerprint,
            }
        }
    }

    pub(super) fn pop_front(&mut self) -> Option<QueuedToolCall> {
        self.queue.pop_front()
    }

    pub(super) fn is_empty(&self) -> bool {
        self.queue.is_empty()
    }

    pub(super) fn take_suppressed_narration(&mut self) -> bool {
        if self.queue.is_empty() && self.suppressed_narration {
            self.suppressed_narration = false;
            true
        } else {
            false
        }
    }
}

pub(super) struct RestoredRunCheckpoint {
    pub(super) context: ContextFrame,
    pub(super) next_model_request_index: usize,
    pub(super) tool_batch: ToolCallBatch,
    pub(super) extension_snapshots: Vec<AgentExtensionSnapshot>,
    pub(super) conversation_trace: ConversationTraceRecorder,
    pub(super) visible_trace_item_count: usize,
}

pub(super) struct RunCheckpointState<'a> {
    pub(super) context: &'a ContextFrame,
    pub(super) next_model_request_index: usize,
    pub(super) tool_batch: &'a ToolCallBatch,
    pub(super) extension_snapshots: Vec<AgentExtensionSnapshot>,
    pub(super) model_visible_trace_item_count: usize,
    pub(super) pending_tool_call_id: &'a str,
    pub(super) conversation_trace: &'a ConversationTraceRecorder,
}

pub(super) fn create_run_checkpoint(
    run_id: &str,
    state: RunCheckpointState<'_>,
) -> AgentResult<AgentRunCheckpoint> {
    let RunCheckpointState {
        context,
        next_model_request_index,
        tool_batch,
        extension_snapshots,
        model_visible_trace_item_count,
        pending_tool_call_id,
        conversation_trace,
    } = state;
    validate_model_tool_call_id(pending_tool_call_id)?;
    for queued in &tool_batch.queue {
        validate_model_tool_call_id(&queued.call.id)?;
    }
    context.validate_pending_tool_call(pending_tool_call_id)?;
    let (conversation_trace_items, next_conversation_trace_sequence, conversation_trace_truncated) =
        conversation_trace.checkpoint();
    validate_conversation_trace_tool_call_ids(&conversation_trace_items)?;
    let committed_item_count = conversation_trace.committed_item_count();
    if model_visible_trace_item_count > committed_item_count
        || model_visible_trace_item_count > 0
            && !conversation_trace_items[model_visible_trace_item_count - 1]
                .is_safe_compaction_boundary()
    {
        return Err(AgentError::new(
            "运行检查点的模型可见 trace 游标不是完整日志边界。",
        ));
    }
    let context_items = context.checkpoint_items()?;
    validate_context_checkpoint_tool_call_ids(&context_items)?;
    Ok(AgentRunCheckpoint {
        version: AGENT_RUN_CHECKPOINT_SCHEMA_VERSION,
        run_id: run_id.to_string(),
        context_items,
        next_model_request_index,
        queued_tool_calls: tool_batch
            .queue
            .iter()
            .map(queued_tool_call_checkpoint)
            .collect(),
        suppressed_narration: tool_batch.suppressed_narration,
        extension_snapshots,
        pending_tool_call_id: pending_tool_call_id.to_string(),
        conversation_trace_items,
        next_conversation_trace_sequence,
        conversation_trace_truncated,
        model_visible_trace_item_count,
    })
}

pub(super) fn restore_run_checkpoint(
    checkpoint: AgentRunCheckpoint,
    run_id: &str,
    continuation: &AgentToolContinuation,
) -> AgentResult<RestoredRunCheckpoint> {
    if checkpoint.version != AGENT_RUN_CHECKPOINT_SCHEMA_VERSION {
        return Err(AgentError::new(format!(
            "无法恢复运行检查点：不支持版本 {}，当前版本为 {AGENT_RUN_CHECKPOINT_SCHEMA_VERSION}。",
            checkpoint.version,
        )));
    }
    if checkpoint.run_id != run_id {
        return Err(AgentError::new(format!(
            "无法恢复运行检查点：检查点属于 `{}`，当前运行是 `{run_id}`。",
            checkpoint.run_id
        )));
    }
    validate_model_tool_call_id(&checkpoint.pending_tool_call_id)?;
    validate_model_tool_call_id(&continuation.call.id)?;
    validate_model_tool_call_id(&continuation.result.call_id)?;
    validate_context_checkpoint_tool_call_ids(&checkpoint.context_items)?;
    validate_queued_checkpoint_tool_call_ids(&checkpoint.queued_tool_calls)?;
    validate_conversation_trace_tool_call_ids(&checkpoint.conversation_trace_items)?;
    if checkpoint.pending_tool_call_id != continuation.call.id {
        return Err(AgentError::new(format!(
            "无法恢复运行检查点：待审批调用 `{}` 与续跑结果 `{}` 不一致。",
            checkpoint.pending_tool_call_id, continuation.call.id
        )));
    }
    if continuation.call.id != continuation.result.call_id {
        return Err(AgentError::new(format!(
            "无法恢复运行检查点：续跑调用 `{}` 与工具结果 `{}` 不一致。",
            continuation.call.id, continuation.result.call_id
        )));
    }
    if continuation.call.tool != continuation.result.tool {
        return Err(AgentError::new(format!(
            "无法恢复运行检查点：续跑调用工具 `{}` 与工具结果 `{}` 不一致。",
            continuation.call.tool, continuation.result.tool
        )));
    }
    if checkpoint.next_model_request_index == 0 {
        return Err(AgentError::new(
            "无法恢复运行检查点：下一次模型请求序号无效。",
        ));
    }

    let committed_trace_item_count = ConversationTraceSnapshot {
        items: checkpoint.conversation_trace_items.clone(),
        next_sequence: checkpoint.next_conversation_trace_sequence,
        truncated: checkpoint.conversation_trace_truncated,
    }
    .committed_prefix()
    .items
    .len();
    if checkpoint.model_visible_trace_item_count > committed_trace_item_count
        || checkpoint.model_visible_trace_item_count > 0
            && !checkpoint.conversation_trace_items[checkpoint.model_visible_trace_item_count - 1]
                .is_safe_compaction_boundary()
    {
        return Err(AgentError::new(
            "无法恢复运行检查点：模型可见 trace 游标不是完整日志边界。",
        ));
    }
    let visible_trace_item_count = checkpoint.model_visible_trace_item_count;
    let restored_batch_fingerprints =
        restore_batch_fingerprints(&checkpoint.context_items, &checkpoint.pending_tool_call_id)?;
    let mut conversation_trace = ConversationTraceRecorder::from_checkpoint(
        checkpoint.conversation_trace_items,
        checkpoint.next_conversation_trace_sequence,
        checkpoint.conversation_trace_truncated,
    );
    let mut context = ContextFrame::from_checkpoint_items(checkpoint.context_items)?;
    let checkpoint_call = context.validate_pending_tool_call(&checkpoint.pending_tool_call_id)?;
    if checkpoint_call.name != continuation.call.tool
        || checkpoint_call.args != continuation.call.args
    {
        return Err(AgentError::new(
            "无法恢复运行检查点：续跑调用的工具或参数与冻结的待审批调用不一致。",
        ));
    }
    let continuation_call = LlmToolCall {
        id: continuation.call.id.clone(),
        name: continuation.call.tool.clone(),
        args: continuation.call.args.clone(),
    };
    let llm_result = canonical_tool_result_for_context(&continuation.result);
    context.append_tool_continuation(
        &continuation_call,
        build_tool_observation_message(&llm_result),
        !continuation.result.ok,
    )?;
    conversation_trace.record_tool_call(&continuation.call);
    conversation_trace.record_tool_result(&continuation.call, &llm_result);

    let queue = restore_queued_tool_calls(
        checkpoint.queued_tool_calls,
        &continuation.call.id,
        &context,
    )?;
    Ok(RestoredRunCheckpoint {
        context,
        next_model_request_index: checkpoint.next_model_request_index,
        tool_batch: ToolCallBatch {
            queue,
            suppressed_narration: checkpoint.suppressed_narration,
            seen_semantic_fingerprints: restored_batch_fingerprints,
        },
        extension_snapshots: checkpoint.extension_snapshots,
        conversation_trace,
        visible_trace_item_count,
    })
}

fn restore_batch_fingerprints(
    context_items: &[AgentContextCheckpointItem],
    pending_tool_call_id: &str,
) -> AgentResult<BTreeSet<String>> {
    let pending_item = context_items
        .iter()
        .find(|item| {
            item.tool_calls
                .iter()
                .any(|call| call.id == pending_tool_call_id)
        })
        .ok_or_else(|| AgentError::new("运行检查点缺少冻结的待审批工具调用。"))?;
    let pending_call = pending_item
        .tool_calls
        .iter()
        .find(|call| call.id == pending_tool_call_id)
        .expect("pending item was selected by this call");
    let pending_group = pending_item
        .group
        .as_ref()
        .ok_or_else(|| AgentError::new("运行检查点中的待审批工具调用缺少交换分组。"))?;
    let response_group_prefix = tool_exchange_response_prefix(&pending_group.id);

    let mut fingerprints = BTreeSet::new();
    // Always seed the pending call. This is sufficient for the most important approval boundary:
    // a duplicate queued after the pending action can never execute after resume.
    fingerprints.insert(semantic_tool_call_fingerprint(
        &pending_call.name,
        &pending_call.args,
    ));

    if let Some(prefix) = response_group_prefix {
        for item in context_items {
            let belongs_to_same_response = item
                .group
                .as_ref()
                .and_then(|group| tool_exchange_response_prefix(&group.id))
                .is_some_and(|candidate| candidate == prefix);
            if !belongs_to_same_response {
                continue;
            }
            for call in &item.tool_calls {
                fingerprints.insert(semantic_tool_call_fingerprint(&call.name, &call.args));
            }
        }
    }

    Ok(fingerprints)
}

fn tool_exchange_response_prefix(group_id: &str) -> Option<&str> {
    let (prefix, tool_index) = group_id.rsplit_once(':')?;
    tool_index.parse::<usize>().ok()?;
    prefix
        .starts_with("run:")
        .then_some(prefix)
        .filter(|prefix| prefix.contains(":tool-exchange:"))
}

fn queued_tool_call_checkpoint(call: &QueuedToolCall) -> AgentQueuedToolCallCheckpoint {
    AgentQueuedToolCallCheckpoint {
        call: AgentContextCheckpointToolCall {
            id: call.call.id.clone(),
            name: call.call.name.clone(),
            args: call.call.args.clone(),
        },
        assistant_content: call.assistant_content.clone(),
        group_id: call.group_id.clone(),
    }
}

fn validate_context_checkpoint_tool_call_ids(
    items: &[AgentContextCheckpointItem],
) -> AgentResult<()> {
    for item in items {
        if let Some(call_id) = &item.tool_call_id {
            validate_model_tool_call_id(call_id)?;
        }
        for call in &item.tool_calls {
            validate_model_tool_call_id(&call.id)?;
        }
    }
    Ok(())
}

fn validate_queued_checkpoint_tool_call_ids(
    calls: &[AgentQueuedToolCallCheckpoint],
) -> AgentResult<()> {
    for queued in calls {
        validate_model_tool_call_id(&queued.call.id)?;
    }
    Ok(())
}

fn validate_conversation_trace_tool_call_ids(
    items: &[ConversationTurnTraceItem],
) -> AgentResult<()> {
    for item in items {
        match item {
            ConversationTurnTraceItem::ToolCall { call_id, .. }
            | ConversationTurnTraceItem::ToolResult { call_id, .. } => {
                validate_model_tool_call_id(call_id)?;
            }
            ConversationTurnTraceItem::AssistantNarration { .. } => {}
        }
    }
    Ok(())
}

fn restore_queued_tool_calls(
    calls: Vec<AgentQueuedToolCallCheckpoint>,
    pending_tool_call_id: &str,
    context: &ContextFrame,
) -> AgentResult<VecDeque<QueuedToolCall>> {
    let mut ids = BTreeSet::new();
    ids.insert(pending_tool_call_id.to_string());
    let mut group_ids = BTreeSet::new();
    calls
        .into_iter()
        .map(|queued| {
            validate_model_tool_call_id(&queued.call.id)?;
            if queued.call.name.trim().is_empty() {
                return Err(AgentError::new("运行检查点中的待执行工具调用缺少名称。"));
            }
            if !ids.insert(queued.call.id.clone()) {
                return Err(AgentError::new(format!(
                    "运行检查点中的工具调用 id `{}` 重复。",
                    queued.call.id
                )));
            }
            if context.contains_tool_call_id(&queued.call.id) {
                return Err(AgentError::new(format!(
                    "运行检查点中的待执行工具调用 id `{}` 已在上下文中使用。",
                    queued.call.id
                )));
            }
            if queued.group_id.trim().is_empty()
                || context.contains_group_id(&queued.group_id)
                || !group_ids.insert(queued.group_id.clone())
            {
                return Err(AgentError::new("运行检查点中的工具交换分组为空或重复。"));
            }
            Ok(QueuedToolCall {
                call: LlmToolCall {
                    id: queued.call.id,
                    name: queued.call.name,
                    args: queued.call.args,
                },
                assistant_content: queued.assistant_content,
                group_id: queued.group_id,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::{
        ContextCapacityDetector, ContextItem, ContextMetadata, ContextRetention, ContextScope,
        ContextSource,
    };
    use crate::llm::{model_response_tool_call_id, LlmMessageRole};
    use crate::protocol::{AgentApiStyle, AgentApprovalStatus, AgentToolCall, AgentToolResult};
    use serde_json::json;

    fn canonical_test_call_id(tool_index: usize, provider_call_id: &str) -> String {
        model_response_tool_call_id("checkpoint-validation-run", 0, tool_index, provider_call_id)
    }

    fn restorable_checkpoint_fixture() -> (AgentRunCheckpoint, AgentToolContinuation) {
        let pending = LlmToolCall {
            id: canonical_test_call_id(0, "provider-pending"),
            name: "write_file".to_string(),
            args: json!({ "path": "report.txt" }),
        };
        let context = ContextFrame::new(vec![ContextItem::assistant(
            "",
            vec![pending.clone()],
            ContextMetadata::new(
                ContextSource::ModelResponse,
                ContextScope::Run,
                ContextRetention::Retained,
            )
            .with_group(ContextGroup::tool_exchange("validation-pending")),
        )]);
        let batch = ToolCallBatch::from_model_response(
            "checkpoint-validation-run",
            0,
            String::new(),
            vec![LlmToolCall {
                id: canonical_test_call_id(1, "provider-queued"),
                name: "read_file".to_string(),
                args: json!({ "path": "report.txt" }),
            }],
            false,
        );
        let trace = ConversationTraceRecorder::default();
        let checkpoint = create_run_checkpoint(
            "checkpoint-validation-run",
            RunCheckpointState {
                context: &context,
                next_model_request_index: 1,
                tool_batch: &batch,
                extension_snapshots: Vec::new(),
                model_visible_trace_item_count: 0,
                pending_tool_call_id: &pending.id,
                conversation_trace: &trace,
            },
        )
        .unwrap();
        let continuation = AgentToolContinuation {
            call: AgentToolCall {
                id: pending.id.clone(),
                tool: pending.name.clone(),
                args: pending.args.clone(),
                approval_status: AgentApprovalStatus::Approved,
                reason: None,
            },
            result: AgentToolResult {
                call_id: pending.id,
                tool: pending.name,
                ok: true,
                result: Some(json!({ "status": "applied" })),
                error: None,
            },
        };
        (checkpoint, continuation)
    }

    fn assert_invalid_tool_call_id(error: AgentError) {
        assert_eq!(error.code(), Some("agent.invalid_model_tool_call_id"));
    }

    fn restore_error(result: AgentResult<RestoredRunCheckpoint>) -> AgentError {
        result.err().expect("checkpoint restore should fail")
    }

    #[test]
    fn checkpoint_v2_is_rejected_without_attempting_identity_migration() {
        let (mut checkpoint, continuation) = restorable_checkpoint_fixture();
        checkpoint.version = 2;
        checkpoint.pending_tool_call_id = "legacy/provider/call".to_string();

        let error = restore_error(restore_run_checkpoint(
            checkpoint,
            "checkpoint-validation-run",
            &continuation,
        ));

        assert!(error.to_string().contains("不支持版本 2"));
        assert!(error.to_string().contains("当前版本为 3"));
    }

    #[test]
    fn one_model_response_claims_reason_only_duplicates_once() {
        let mut batch = ToolCallBatch::from_model_response(
            "claim-run",
            0,
            String::new(),
            vec![
                LlmToolCall {
                    id: canonical_test_call_id(0, "claim-first"),
                    name: "write_file".to_string(),
                    args: json!({
                        "filePath": "report.txt",
                        "content": "same",
                        "reason": "Create the report"
                    }),
                },
                LlmToolCall {
                    id: canonical_test_call_id(1, "claim-duplicate"),
                    name: "write_file".to_string(),
                    args: json!({
                        "reason": "Write the requested file",
                        "content": "same",
                        "filePath": "report.txt"
                    }),
                },
            ],
            false,
        );
        let first = batch.pop_front().unwrap();
        let duplicate = batch.pop_front().unwrap();

        assert_eq!(batch.claim(&first.call), ToolCallBatchClaim::Execute);
        assert!(matches!(
            batch.claim(&duplicate.call),
            ToolCallBatchClaim::Duplicate { .. }
        ));
    }

    #[test]
    fn approval_restore_reconstructs_all_seen_calls_from_the_same_model_response() {
        let first = LlmToolCall {
            id: canonical_test_call_id(0, "restore-first"),
            name: "read_file".to_string(),
            args: json!({ "path": "source.txt", "reason": "Read source" }),
        };
        let pending = LlmToolCall {
            id: canonical_test_call_id(1, "restore-pending"),
            name: "write_file".to_string(),
            args: json!({
                "filePath": "report.txt",
                "content": "draft",
                "reason": "Write report"
            }),
        };
        let duplicate_first = LlmToolCall {
            id: canonical_test_call_id(2, "restore-duplicate"),
            name: "read_file".to_string(),
            args: json!({ "reason": "Read it again", "path": "source.txt" }),
        };
        let mut batch = ToolCallBatch::from_model_response(
            "checkpoint-validation-run",
            0,
            String::new(),
            vec![first.clone(), pending.clone(), duplicate_first],
            false,
        );
        let first = batch.pop_front().unwrap();
        assert_eq!(batch.claim(&first.call), ToolCallBatchClaim::Execute);
        let pending_queued = batch.pop_front().unwrap();
        assert_eq!(
            batch.claim(&pending_queued.call),
            ToolCallBatchClaim::Execute
        );

        let first_group = first.context_group();
        let pending_group = pending_queued.context_group();
        let context = ContextFrame::new(vec![
            ContextItem::assistant(
                "",
                vec![first.call.clone()],
                ContextMetadata::new(
                    ContextSource::ModelResponse,
                    ContextScope::Run,
                    ContextRetention::Retained,
                )
                .with_group(first_group.clone()),
            ),
            ContextItem::tool_result(
                first.call.id.clone(),
                "{}",
                false,
                ContextMetadata::new(
                    ContextSource::ToolResult,
                    ContextScope::Run,
                    ContextRetention::Retained,
                )
                .with_group(first_group),
            ),
            ContextItem::assistant(
                "",
                vec![pending_queued.call.clone()],
                ContextMetadata::new(
                    ContextSource::ModelResponse,
                    ContextScope::Run,
                    ContextRetention::Retained,
                )
                .with_group(pending_group),
            ),
        ]);
        let checkpoint = create_run_checkpoint(
            "checkpoint-validation-run",
            RunCheckpointState {
                context: &context,
                next_model_request_index: 1,
                tool_batch: &batch,
                extension_snapshots: Vec::new(),
                model_visible_trace_item_count: 0,
                pending_tool_call_id: &pending.id,
                conversation_trace: &ConversationTraceRecorder::default(),
            },
        )
        .unwrap();
        let continuation = AgentToolContinuation {
            call: AgentToolCall {
                id: pending.id.clone(),
                tool: pending.name.clone(),
                args: pending.args.clone(),
                approval_status: AgentApprovalStatus::Approved,
                reason: None,
            },
            result: AgentToolResult {
                call_id: pending.id,
                tool: pending.name,
                ok: true,
                result: Some(json!({ "status": "written" })),
                error: None,
            },
        };

        let mut restored =
            restore_run_checkpoint(checkpoint, "checkpoint-validation-run", &continuation).unwrap();
        let queued_duplicate = restored.tool_batch.pop_front().unwrap();
        assert!(matches!(
            restored.tool_batch.claim(&queued_duplicate.call),
            ToolCallBatchClaim::Duplicate { .. }
        ));
    }

    #[test]
    fn checkpoint_creation_rejects_invalid_pending_queued_context_and_trace_ids() {
        let invalid_id = "legacy/provider/call".to_string();
        let pending = LlmToolCall {
            id: invalid_id.clone(),
            name: "write_file".to_string(),
            args: json!({ "path": "report.txt" }),
        };
        let context = ContextFrame::new(vec![ContextItem::assistant(
            "",
            vec![pending.clone()],
            ContextMetadata::new(
                ContextSource::ModelResponse,
                ContextScope::Run,
                ContextRetention::Retained,
            )
            .with_group(ContextGroup::tool_exchange("invalid-pending")),
        )]);
        let batch = ToolCallBatch::default();
        let trace = ConversationTraceRecorder::default();
        let error = create_run_checkpoint(
            "create-invalid-pending",
            RunCheckpointState {
                context: &context,
                next_model_request_index: 1,
                tool_batch: &batch,
                extension_snapshots: Vec::new(),
                model_visible_trace_item_count: 0,
                pending_tool_call_id: &pending.id,
                conversation_trace: &trace,
            },
        )
        .unwrap_err();
        assert_invalid_tool_call_id(error);

        let valid_pending = LlmToolCall {
            id: canonical_test_call_id(0, "create-valid-pending"),
            name: "write_file".to_string(),
            args: json!({ "path": "report.txt" }),
        };
        let valid_pending_context = ContextFrame::new(vec![ContextItem::assistant(
            "",
            vec![valid_pending.clone()],
            ContextMetadata::new(
                ContextSource::ModelResponse,
                ContextScope::Run,
                ContextRetention::Retained,
            )
            .with_group(ContextGroup::tool_exchange("valid-pending")),
        )]);
        let invalid_queue = ToolCallBatch::from_model_response(
            "create-invalid-queue",
            0,
            String::new(),
            vec![LlmToolCall {
                id: invalid_id.clone(),
                name: "read_file".to_string(),
                args: json!({ "path": "report.txt" }),
            }],
            false,
        );
        let error = create_run_checkpoint(
            "create-invalid-queue",
            RunCheckpointState {
                context: &valid_pending_context,
                next_model_request_index: 1,
                tool_batch: &invalid_queue,
                extension_snapshots: Vec::new(),
                model_visible_trace_item_count: 0,
                pending_tool_call_id: &valid_pending.id,
                conversation_trace: &trace,
            },
        )
        .unwrap_err();
        assert_invalid_tool_call_id(error);

        let historical_group = ContextGroup::tool_exchange("invalid-history");
        let invalid_context = ContextFrame::new(vec![
            ContextItem::assistant(
                "",
                vec![LlmToolCall {
                    id: invalid_id.clone(),
                    name: "read_file".to_string(),
                    args: json!({ "path": "old.txt" }),
                }],
                ContextMetadata::new(
                    ContextSource::ModelResponse,
                    ContextScope::Run,
                    ContextRetention::Retained,
                )
                .with_group(historical_group.clone()),
            ),
            ContextItem::tool_result(
                invalid_id.clone(),
                "{}",
                false,
                ContextMetadata::new(
                    ContextSource::ToolResult,
                    ContextScope::Run,
                    ContextRetention::Retained,
                )
                .with_group(historical_group),
            ),
            ContextItem::assistant(
                "",
                vec![valid_pending.clone()],
                ContextMetadata::new(
                    ContextSource::ModelResponse,
                    ContextScope::Run,
                    ContextRetention::Retained,
                )
                .with_group(ContextGroup::tool_exchange("valid-context-pending")),
            ),
        ]);
        let error = create_run_checkpoint(
            "create-invalid-context",
            RunCheckpointState {
                context: &invalid_context,
                next_model_request_index: 1,
                tool_batch: &batch,
                extension_snapshots: Vec::new(),
                model_visible_trace_item_count: 0,
                pending_tool_call_id: &valid_pending.id,
                conversation_trace: &trace,
            },
        )
        .unwrap_err();
        assert_invalid_tool_call_id(error);

        let mut invalid_trace = ConversationTraceRecorder::default();
        invalid_trace.record_tool_call(&AgentToolCall {
            id: invalid_id,
            tool: "read_file".to_string(),
            args: json!({ "path": "old.txt" }),
            approval_status: AgentApprovalStatus::NotRequired,
            reason: None,
        });
        let error = create_run_checkpoint(
            "create-invalid-trace",
            RunCheckpointState {
                context: &valid_pending_context,
                next_model_request_index: 1,
                tool_batch: &batch,
                extension_snapshots: Vec::new(),
                model_visible_trace_item_count: 0,
                pending_tool_call_id: &valid_pending.id,
                conversation_trace: &invalid_trace,
            },
        )
        .unwrap_err();
        assert_invalid_tool_call_id(error);
    }

    #[test]
    fn checkpoint_restore_validates_every_persisted_and_continuation_id() {
        let (checkpoint, continuation) = restorable_checkpoint_fixture();

        let mut invalid_pending = checkpoint.clone();
        invalid_pending.pending_tool_call_id = "legacy/provider/pending".to_string();
        assert_invalid_tool_call_id(restore_error(restore_run_checkpoint(
            invalid_pending,
            "checkpoint-validation-run",
            &continuation,
        )));

        let mut invalid_context = checkpoint.clone();
        invalid_context.context_items[0].tool_calls[0].id = "legacy/provider/context".to_string();
        assert_invalid_tool_call_id(restore_error(restore_run_checkpoint(
            invalid_context,
            "checkpoint-validation-run",
            &continuation,
        )));

        let mut invalid_queue = checkpoint.clone();
        invalid_queue.queued_tool_calls[0].call.id = "legacy/provider/queued".to_string();
        assert_invalid_tool_call_id(restore_error(restore_run_checkpoint(
            invalid_queue,
            "checkpoint-validation-run",
            &continuation,
        )));

        let mut invalid_trace = checkpoint.clone();
        invalid_trace
            .conversation_trace_items
            .push(ConversationTurnTraceItem::ToolCall {
                sequence: 0,
                call_id: "legacy/provider/trace".to_string(),
                tool: "read_file".to_string(),
                operation: json!({ "path": "old.txt" }),
                approval_status: AgentApprovalStatus::NotRequired,
                truncated: false,
            });
        assert_invalid_tool_call_id(restore_error(restore_run_checkpoint(
            invalid_trace,
            "checkpoint-validation-run",
            &continuation,
        )));

        let mut invalid_continuation_call = continuation.clone();
        invalid_continuation_call.call.id = "legacy/provider/continuation".to_string();
        assert_invalid_tool_call_id(restore_error(restore_run_checkpoint(
            checkpoint.clone(),
            "checkpoint-validation-run",
            &invalid_continuation_call,
        )));

        let mut invalid_continuation_result = continuation.clone();
        invalid_continuation_result.result.call_id = "legacy/provider/result".to_string();
        assert_invalid_tool_call_id(restore_error(restore_run_checkpoint(
            checkpoint,
            "checkpoint-validation-run",
            &invalid_continuation_result,
        )));
    }

    #[test]
    fn checkpoint_restore_rejects_a_valid_but_mismatched_continuation_result_id() {
        let (checkpoint, mut continuation) = restorable_checkpoint_fixture();
        continuation.result.call_id = canonical_test_call_id(9, "different-result");

        let error = restore_error(restore_run_checkpoint(
            checkpoint,
            "checkpoint-validation-run",
            &continuation,
        ));

        assert!(error.to_string().contains("续跑调用"));
        assert!(error.to_string().contains("工具结果"));
        assert!(error.to_string().contains("不一致"));
    }

    #[test]
    fn checkpoint_restore_rejects_changed_call_tool_args_and_result_tool() {
        let (checkpoint, continuation) = restorable_checkpoint_fixture();

        let mut changed_tool = continuation.clone();
        changed_tool.call.tool = "run_command".to_string();
        changed_tool.result.tool = "run_command".to_string();
        let error = restore_error(restore_run_checkpoint(
            checkpoint.clone(),
            "checkpoint-validation-run",
            &changed_tool,
        ));
        assert!(error.to_string().contains("工具或参数"));

        let mut changed_args = continuation.clone();
        changed_args.call.args = json!({ "path": "different.txt" });
        let error = restore_error(restore_run_checkpoint(
            checkpoint.clone(),
            "checkpoint-validation-run",
            &changed_args,
        ));
        assert!(error.to_string().contains("工具或参数"));

        let mut changed_result_tool = continuation;
        changed_result_tool.result.tool = "run_command".to_string();
        let error = restore_error(restore_run_checkpoint(
            checkpoint,
            "checkpoint-validation-run",
            &changed_result_tool,
        ));
        assert!(error.to_string().contains("续跑调用工具"));
        assert!(error.to_string().contains("工具结果"));
    }

    #[test]
    fn checkpoint_round_trip_closes_pending_exchange_and_restores_queue() {
        let pending = LlmToolCall {
            id: canonical_test_call_id(0, "round-trip-pending"),
            name: "write_file".to_string(),
            args: json!({ "phase": "finish" }),
        };
        let queued_call_id = canonical_test_call_id(1, "round-trip-queued");
        let group = ContextGroup::tool_exchange("exchange-1");
        let context = ContextFrame::new(vec![
            ContextItem::text(
                LlmMessageRole::System,
                "rules",
                ContextSource::BackendSystemPrompt,
                ContextScope::Run,
                ContextRetention::Retained,
            ),
            ContextItem::assistant(
                "",
                vec![pending.clone()],
                ContextMetadata::new(
                    ContextSource::ModelResponse,
                    ContextScope::Run,
                    ContextRetention::Retained,
                )
                .with_group(group),
            ),
        ]);
        let batch = ToolCallBatch::from_model_response(
            "run-1",
            0,
            String::new(),
            vec![LlmToolCall {
                id: queued_call_id.clone(),
                name: "read_file".to_string(),
                args: json!({ "path": "report.txt" }),
            }],
            true,
        );
        let trace = ConversationTraceRecorder::default();
        let checkpoint = create_run_checkpoint(
            "run-1",
            RunCheckpointState {
                context: &context,
                next_model_request_index: 1,
                tool_batch: &batch,
                extension_snapshots: Vec::new(),
                model_visible_trace_item_count: 0,
                pending_tool_call_id: &pending.id,
                conversation_trace: &trace,
            },
        )
        .unwrap();
        let continuation = AgentToolContinuation {
            call: AgentToolCall {
                id: pending.id.clone(),
                tool: "write_file".to_string(),
                args: json!({ "phase": "finish" }),
                approval_status: AgentApprovalStatus::Approved,
                reason: None,
            },
            result: AgentToolResult {
                call_id: pending.id,
                tool: "write_file".to_string(),
                ok: true,
                result: Some(json!({ "status": "applied" })),
                error: None,
            },
        };

        let mut restored = restore_run_checkpoint(checkpoint, "run-1", &continuation).unwrap();

        restored.context.validate_complete_tool_protocol().unwrap();
        assert_eq!(restored.next_model_request_index, 1);
        assert!(!restored.tool_batch.take_suppressed_narration());
        let queued = restored.tool_batch.pop_front().unwrap();
        assert_eq!(queued.call.id, queued_call_id);
        assert!(restored.tool_batch.take_suppressed_narration());
    }

    #[test]
    fn approval_resume_preserves_failed_command_observation_for_the_model() {
        let pending = LlmToolCall {
            id: canonical_test_call_id(0, "failed-command-pending"),
            name: "run_command".to_string(),
            args: json!({ "command": "python3 -c 'import openpyxl'" }),
        };
        let context = ContextFrame::new(vec![ContextItem::assistant(
            "",
            vec![pending.clone()],
            ContextMetadata::new(
                ContextSource::ModelResponse,
                ContextScope::Run,
                ContextRetention::Retained,
            )
            .with_group(ContextGroup::tool_exchange("command-exchange")),
        )]);
        let batch = ToolCallBatch::default();
        let trace = ConversationTraceRecorder::default();
        let checkpoint = create_run_checkpoint(
            "run-command",
            RunCheckpointState {
                context: &context,
                next_model_request_index: 1,
                tool_batch: &batch,
                extension_snapshots: Vec::new(),
                model_visible_trace_item_count: 0,
                pending_tool_call_id: &pending.id,
                conversation_trace: &trace,
            },
        )
        .unwrap();
        let continuation = AgentToolContinuation {
            call: AgentToolCall {
                id: pending.id.clone(),
                tool: pending.name.clone(),
                args: pending.args.clone(),
                approval_status: AgentApprovalStatus::Approved,
                reason: None,
            },
            result: AgentToolResult {
                call_id: pending.id,
                tool: pending.name,
                ok: false,
                result: Some(json!({
                    "exitCode": 1,
                    "stdout": "dependency check started",
                    "stderr": "ModuleNotFoundError: No module named 'openpyxl'",
                    "timedOut": false,
                    "cancelled": false,
                    "stdoutTruncated": false,
                    "stderrTruncated": false,
                })),
                error: Some("命令执行失败。".to_string()),
            },
        };

        let restored = restore_run_checkpoint(checkpoint, "run-command", &continuation).unwrap();

        restored.context.validate_complete_tool_protocol().unwrap();
        let messages = restored.context.to_messages();
        let observation = messages.last().expect("restored tool observation");
        assert_eq!(observation.role, LlmMessageRole::Tool);
        assert!(observation.is_error);
        assert!(observation.content.contains("\"exitCode\": 1"));
        assert!(observation.content.contains("dependency check started"));
        assert!(observation.content.contains("ModuleNotFoundError"));
        assert!(observation.content.contains("\"stdoutTruncated\": false"));
        assert!(observation.content.contains("\"stderrTruncated\": false"));
    }

    #[test]
    fn approval_resume_preserves_compacted_context_without_restoring_raw_history() {
        let pending = LlmToolCall {
            id: canonical_test_call_id(0, "compaction-pending"),
            name: "write_file".to_string(),
            args: json!({ "phase": "finish", "path": "report.txt" }),
        };
        let active = ContextFrame::new(vec![
            ContextItem::text(
                LlmMessageRole::System,
                "system rules",
                ContextSource::BackendSystemPrompt,
                ContextScope::Run,
                ContextRetention::Retained,
            ),
            ContextItem::text(
                LlmMessageRole::User,
                "RAW_HISTORY_MUST_NOT_RETURN",
                ContextSource::ConversationHistory,
                ContextScope::Conversation,
                ContextRetention::Retained,
            ),
            ContextItem::text(
                LlmMessageRole::Assistant,
                "old answer",
                ContextSource::ConversationHistory,
                ContextScope::Conversation,
                ContextRetention::Retained,
            ),
            ContextItem::text(
                LlmMessageRole::User,
                "current request",
                ContextSource::ConversationHistory,
                ContextScope::Conversation,
                ContextRetention::Retained,
            ),
            ContextItem::assistant(
                "I will write the report.",
                vec![pending.clone()],
                ContextMetadata::new(
                    ContextSource::ModelResponse,
                    ContextScope::Run,
                    ContextRetention::Retained,
                )
                .with_group(ContextGroup::tool_exchange("compacted-write-exchange")),
            ),
        ]);
        let mut compacted = ContextFrame::new(vec![
            ContextItem::text(
                LlmMessageRole::System,
                "system rules",
                ContextSource::BackendSystemPrompt,
                ContextScope::Run,
                ContextRetention::Retained,
            ),
            ContextItem::text(
                LlmMessageRole::Assistant,
                "COMPACTED_HISTORY_SURVIVES_RESUME",
                ContextSource::ConversationSummary,
                ContextScope::Conversation,
                ContextRetention::Retained,
            ),
            ContextItem::text(
                LlmMessageRole::User,
                "current request",
                ContextSource::ConversationHistory,
                ContextScope::Conversation,
                ContextRetention::Retained,
            ),
        ]);
        let detector =
            ContextCapacityDetector::for_model("test-model", AgentApiStyle::OpenAiCompatible, &[]);
        detector.prepare_frame(&mut compacted);
        let compacted_baseline = compacted.share_measured_persistent_baseline().unwrap();
        let active = active.replace_persistent_baseline(compacted_baseline);
        let tool_batch = ToolCallBatch::default();
        let conversation_trace = ConversationTraceRecorder::default();
        let checkpoint = create_run_checkpoint(
            "run-compacted",
            RunCheckpointState {
                context: &active,
                next_model_request_index: 2,
                tool_batch: &tool_batch,
                extension_snapshots: Vec::new(),
                model_visible_trace_item_count: 0,
                pending_tool_call_id: &pending.id,
                conversation_trace: &conversation_trace,
            },
        )
        .unwrap();
        let continuation = AgentToolContinuation {
            call: AgentToolCall {
                id: pending.id.clone(),
                tool: pending.name.clone(),
                args: pending.args.clone(),
                approval_status: AgentApprovalStatus::Approved,
                reason: None,
            },
            result: AgentToolResult {
                call_id: pending.id,
                tool: pending.name,
                ok: true,
                result: Some(json!({ "status": "applied" })),
                error: None,
            },
        };

        let restored = restore_run_checkpoint(checkpoint, "run-compacted", &continuation).unwrap();
        restored.context.validate_complete_tool_protocol().unwrap();
        let combined_content = restored
            .context
            .to_messages()
            .into_iter()
            .map(|message| message.content)
            .collect::<Vec<_>>()
            .join("\n");

        assert!(combined_content.contains("COMPACTED_HISTORY_SURVIVES_RESUME"));
        assert!(combined_content.contains("current request"));
        assert!(combined_content.contains("applied"));
        assert!(!combined_content.contains("RAW_HISTORY_MUST_NOT_RETURN"));
    }

    #[test]
    fn approval_resume_preserves_the_actual_model_visible_trace_cursor() {
        let completed_call = AgentToolCall {
            id: canonical_test_call_id(0, "visible-trace-completed"),
            tool: "read_file".to_string(),
            args: json!({ "path": "notes.txt" }),
            approval_status: AgentApprovalStatus::NotRequired,
            reason: None,
        };
        let completed_result = AgentToolResult {
            call_id: completed_call.id.clone(),
            tool: completed_call.tool.clone(),
            ok: true,
            result: Some(json!({ "content": "unseen result" })),
            error: None,
        };
        let pending_call = AgentToolCall {
            id: canonical_test_call_id(1, "visible-trace-pending"),
            tool: "write_file".to_string(),
            args: json!({ "phase": "finish" }),
            approval_status: AgentApprovalStatus::Required,
            reason: None,
        };
        let first_group = ContextGroup::tool_exchange("first-exchange");
        let pending_group = ContextGroup::tool_exchange("pending-exchange");
        let context = ContextFrame::new(vec![
            ContextItem::text(
                LlmMessageRole::System,
                "rules",
                ContextSource::BackendSystemPrompt,
                ContextScope::Run,
                ContextRetention::Retained,
            ),
            ContextItem::assistant(
                "",
                vec![LlmToolCall {
                    id: completed_call.id.clone(),
                    name: completed_call.tool.clone(),
                    args: completed_call.args.clone(),
                }],
                ContextMetadata::new(
                    ContextSource::ModelResponse,
                    ContextScope::Run,
                    ContextRetention::Retained,
                )
                .with_group(first_group.clone()),
            ),
            ContextItem::tool_result(
                completed_call.id.clone(),
                build_tool_observation_message(&completed_result),
                false,
                ContextMetadata::new(
                    ContextSource::ToolResult,
                    ContextScope::Run,
                    ContextRetention::Retained,
                )
                .with_group(first_group),
            ),
            ContextItem::assistant(
                "",
                vec![LlmToolCall {
                    id: pending_call.id.clone(),
                    name: pending_call.tool.clone(),
                    args: pending_call.args.clone(),
                }],
                ContextMetadata::new(
                    ContextSource::ModelResponse,
                    ContextScope::Run,
                    ContextRetention::Retained,
                )
                .with_group(pending_group),
            ),
        ]);
        let mut trace = ConversationTraceRecorder::default();
        trace.record_tool_call(&completed_call);
        trace.record_tool_result(&completed_call, &completed_result);
        trace.record_tool_call(&pending_call);
        let tool_batch = ToolCallBatch::default();
        let checkpoint = create_run_checkpoint(
            "run-multi-tool",
            RunCheckpointState {
                context: &context,
                next_model_request_index: 1,
                tool_batch: &tool_batch,
                extension_snapshots: Vec::new(),
                model_visible_trace_item_count: 0,
                pending_tool_call_id: &pending_call.id,
                conversation_trace: &trace,
            },
        )
        .unwrap();
        let pending_call_id = pending_call.id.clone();
        let continuation = AgentToolContinuation {
            call: AgentToolCall {
                approval_status: AgentApprovalStatus::Approved,
                ..pending_call
            },
            result: AgentToolResult {
                call_id: pending_call_id,
                tool: "write_file".to_string(),
                ok: true,
                result: Some(json!({ "status": "applied" })),
                error: None,
            },
        };

        let restored = restore_run_checkpoint(checkpoint, "run-multi-tool", &continuation).unwrap();

        assert_eq!(restored.visible_trace_item_count, 0);
        assert_eq!(restored.conversation_trace.committed_item_count(), 4);
    }
}
