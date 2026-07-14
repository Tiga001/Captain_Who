//! Durable agent-loop checkpoints used at approval boundaries.
//!
//! A checkpoint owns every piece of in-memory state needed to resume the same logical run. The
//! pending tool call already appears as the final, unresolved exchange in `context`; queued calls
//! belong to the same model response but have not started yet. On resume, the approved/rejected
//! result closes the pending exchange before queued calls continue.

use super::tool_flow::build_tool_observation_message;
use crate::context::{ContextFrame, ContextGroup};
use crate::conversation_trace::{
    canonical_tool_result_for_context, ConversationTraceRecorder, ConversationTraceSnapshot,
};
use crate::llm::LlmToolCall;
use crate::protocol::{
    AgentContextCheckpointToolCall, AgentError, AgentExtensionSnapshot,
    AgentQueuedToolCallCheckpoint, AgentResult, AgentRunCheckpoint, AgentToolContinuation,
};
use std::collections::{BTreeSet, VecDeque};

const RUN_CHECKPOINT_VERSION: u32 = 2;

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
    context.validate_pending_tool_call(pending_tool_call_id)?;
    let (conversation_trace_items, next_conversation_trace_sequence, conversation_trace_truncated) =
        conversation_trace.checkpoint();
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
    Ok(AgentRunCheckpoint {
        version: RUN_CHECKPOINT_VERSION,
        run_id: run_id.to_string(),
        context_items: context.checkpoint_items()?,
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
    if checkpoint.version != RUN_CHECKPOINT_VERSION {
        return Err(AgentError::new(format!(
            "无法恢复运行检查点：不支持版本 {}，当前版本为 {RUN_CHECKPOINT_VERSION}。",
            checkpoint.version
        )));
    }
    if checkpoint.run_id != run_id {
        return Err(AgentError::new(format!(
            "无法恢复运行检查点：检查点属于 `{}`，当前运行是 `{run_id}`。",
            checkpoint.run_id
        )));
    }
    if checkpoint.pending_tool_call_id != continuation.call.id {
        return Err(AgentError::new(format!(
            "无法恢复运行检查点：待审批调用 `{}` 与续跑结果 `{}` 不一致。",
            checkpoint.pending_tool_call_id, continuation.call.id
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
    let mut conversation_trace = ConversationTraceRecorder::from_checkpoint(
        checkpoint.conversation_trace_items,
        checkpoint.next_conversation_trace_sequence,
        checkpoint.conversation_trace_truncated,
    );
    let mut context = ContextFrame::from_checkpoint_items(checkpoint.context_items)?;
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
        },
        extension_snapshots: checkpoint.extension_snapshots,
        conversation_trace,
        visible_trace_item_count,
    })
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
            if queued.call.id.trim().is_empty() || queued.call.name.trim().is_empty() {
                return Err(AgentError::new(
                    "运行检查点中的待执行工具调用缺少 id 或名称。",
                ));
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
    use crate::llm::LlmMessageRole;
    use crate::protocol::{AgentApiStyle, AgentApprovalStatus, AgentToolCall, AgentToolResult};
    use serde_json::json;

    #[test]
    fn checkpoint_round_trip_closes_pending_exchange_and_restores_queue() {
        let pending = LlmToolCall {
            id: "write-1".to_string(),
            name: "write_file".to_string(),
            args: json!({ "phase": "finish" }),
        };
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
                id: "read-2".to_string(),
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
                pending_tool_call_id: "write-1",
                conversation_trace: &trace,
            },
        )
        .unwrap();
        let continuation = AgentToolContinuation {
            call: AgentToolCall {
                id: "write-1".to_string(),
                tool: "write_file".to_string(),
                args: json!({ "phase": "finish" }),
                approval_status: AgentApprovalStatus::Approved,
                reason: None,
            },
            result: AgentToolResult {
                call_id: "write-1".to_string(),
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
        assert_eq!(queued.call.id, "read-2");
        assert!(restored.tool_batch.take_suppressed_narration());
    }

    #[test]
    fn approval_resume_preserves_compacted_context_without_restoring_raw_history() {
        let pending = LlmToolCall {
            id: "write-after-compaction".to_string(),
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
            id: "read-before-approval".to_string(),
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
            id: "write-needs-approval".to_string(),
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
        let continuation = AgentToolContinuation {
            call: AgentToolCall {
                approval_status: AgentApprovalStatus::Approved,
                ..pending_call
            },
            result: AgentToolResult {
                call_id: "write-needs-approval".to_string(),
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
