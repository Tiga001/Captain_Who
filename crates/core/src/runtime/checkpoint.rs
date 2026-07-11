//! Durable agent-loop checkpoints used at approval boundaries.
//!
//! A checkpoint owns every piece of in-memory state needed to resume the same logical run. The
//! pending tool call already appears as the final, unresolved exchange in `context`; queued calls
//! belong to the same model response but have not started yet. On resume, the approved/rejected
//! result closes the pending exchange before queued calls continue.

use super::tool_flow::{build_tool_observation_message, redact_tool_result_for_llm};
use crate::context::{ContextFrame, ContextGroup};
use crate::llm::LlmToolCall;
use crate::protocol::{
    AgentContextCheckpointToolCall, AgentError, AgentExtensionSnapshot,
    AgentQueuedToolCallCheckpoint, AgentResult, AgentRunCheckpoint, AgentToolContinuation,
};
use std::collections::{BTreeSet, VecDeque};

const RUN_CHECKPOINT_VERSION: u32 = 1;

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
}

pub(super) fn create_run_checkpoint(
    run_id: &str,
    context: &ContextFrame,
    next_model_request_index: usize,
    tool_batch: &ToolCallBatch,
    extension_snapshots: Vec<AgentExtensionSnapshot>,
    pending_tool_call_id: &str,
) -> AgentResult<AgentRunCheckpoint> {
    context.validate_pending_tool_call(pending_tool_call_id)?;
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

    let mut context = ContextFrame::from_checkpoint_items(checkpoint.context_items)?;
    let continuation_call = LlmToolCall {
        id: continuation.call.id.clone(),
        name: continuation.call.tool.clone(),
        args: continuation.call.args.clone(),
    };
    let llm_result = redact_tool_result_for_llm(&continuation.result);
    context.append_tool_continuation(
        &continuation_call,
        build_tool_observation_message(&llm_result),
        !continuation.result.ok,
    )?;

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
        ContextItem, ContextMetadata, ContextRetention, ContextScope, ContextSource,
    };
    use crate::llm::LlmMessageRole;
    use crate::protocol::{AgentApprovalStatus, AgentToolCall, AgentToolResult};
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
        let checkpoint =
            create_run_checkpoint("run-1", &context, 1, &batch, Vec::new(), "write-1").unwrap();
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
}
