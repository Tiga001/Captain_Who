use super::{AgentTool, ToolExecutionContext};
use crate::protocol::{
    AgentError, AgentResult, AgentToolApprovalMode, AgentToolDefinition, AgentToolSafety,
};
use crate::{TaskStatePatchOperation, TaskStateSnapshot};
use serde::Deserialize;
use serde_json::{json, Value};

const MAX_OPERATIONS: usize = 20;

pub(super) struct TaskStatePatchTool;

impl AgentTool for TaskStatePatchTool {
    fn exposure(&self) -> super::AgentToolExposure {
        super::AgentToolExposure::Stable
    }

    fn permission_policy(&self) -> super::AgentToolPermissionPolicy {
        super::AgentToolPermissionPolicy::Default
    }

    fn definition(&self) -> AgentToolDefinition {
        AgentToolDefinition {
            name: "task_state_patch".to_string(),
            description: "Atomically update the backend-owned Task Continuation State for the current conversation. Use the taskId and revision shown in Task Continuation State. Every write is compare-and-set; preserve confirmed state, attach authoritative history refs to completed work, and use supersede only when the latest user request replaces the prior objective. Exact historical bodies belong in conversation_history, not this state.".to_string(),
            input_schema: task_state_patch_schema(),
            safety: AgentToolSafety::ReadOnly,
            requires_workspace: false,
            requires_approval: false,
            approval_mode: AgentToolApprovalMode::Never,
        }
    }

    fn execute(&self, context: &ToolExecutionContext, args: Value) -> AgentResult<Value> {
        let args = serde_json::from_value::<TaskStatePatchArgs>(args)
            .map_err(|error| AgentError::new(format!("task_state_patch 参数无效：{error}")))?;
        if args.operations.is_empty() || args.operations.len() > MAX_OPERATIONS {
            return Err(AgentError::new(format!(
                "task_state_patch operations 必须包含 1 到 {MAX_OPERATIONS} 项。"
            )));
        }
        if args
            .operations
            .iter()
            .any(|operation| matches!(operation, TaskStatePatchOperation::ReplaceWorkItems { .. }))
        {
            return Err(AgentError::new(
                "workItems 是 todo_update 的持久化投影；请通过 todo_update 修改，不能直接 task_state_patch。",
            ));
        }
        context.check_cancelled()?;
        let snapshot = context
            .storage()?
            .patch_task_state(
                context.conversation_id()?,
                args.task_id.trim(),
                args.expected_revision,
                &args.operations,
                Some(context.run_id()?),
                context.tool_call_id()?,
                crate::storage::now_ms(),
            )
            .map_err(AgentError::new)?;
        snapshot.validate()?;
        serde_json::to_value(snapshot)
            .map_err(|error| AgentError::new(format!("无法序列化 Task State：{error}")))
    }

    fn archives_result(&self) -> bool {
        false
    }

    fn trace_projection(
        &self,
        result: &crate::protocol::AgentToolResult,
    ) -> crate::protocol::AgentToolResult {
        let mut projected = result.clone();
        projected.result = result.result.as_ref().and_then(project_task_state_result);
        crate::conversation_trace::canonical_tool_result_for_context(&projected)
    }
}

fn task_state_patch_schema() -> Value {
    serde_json::from_str(
        r#"{
          "type": "object",
          "properties": {
            "taskId": { "type": "string" },
            "expectedRevision": { "type": "integer", "minimum": 1 },
            "operations": {
              "type": "array",
              "minItems": 1,
              "maxItems": 20,
              "items": {
                "type": "object",
                "properties": {
                  "op": {
                    "type": "string",
                    "enum": [
                      "set_objective", "set_status", "set_acceptance_criteria",
                      "set_current_phase", "set_next_actions",
                      "set_blockers", "set_confirmed_decisions", "set_artifact_refs",
                      "set_unresolved_questions", "set_history_evidence_refs", "supersede"
                    ]
                  },
                  "objective": { "type": "string" },
                  "status": {
                    "type": "string",
                    "enum": ["active", "waiting_user", "blocked", "completed", "cancelled"]
                  },
                  "stoppedReason": { "type": ["string", "null"] },
                  "value": { "type": ["string", "null"] },
                  "items": {
                    "type": "array",
                    "maxItems": 40,
                    "items": {
                      "type": ["object", "string"],
                      "properties": {
                        "id": { "type": "string" },
                        "title": { "type": "string" },
                        "status": {
                          "type": "string",
                          "enum": ["pending", "in_progress", "completed", "blocked", "cancelled"]
                        },
                        "note": { "type": ["string", "null"] },
                        "summary": { "type": "string" },
                        "label": { "type": "string" },
                        "evidenceRefs": {
                          "type": "array",
                          "items": {
                            "type": "object",
                            "properties": {
                              "kind": { "type": "string", "enum": ["message", "trace_item", "archive"] },
                              "messageId": { "type": "string" },
                              "assistantMessageId": { "type": "string" },
                              "sequence": { "type": "integer", "minimum": 0 },
                              "archiveRef": { "type": "string" }
                            },
                            "required": ["kind"]
                          }
                        },
                        "ref": {
                          "type": "object",
                          "properties": {
                            "kind": { "type": "string", "enum": ["message", "trace_item", "archive"] },
                            "messageId": { "type": "string" },
                            "assistantMessageId": { "type": "string" },
                            "sequence": { "type": "integer", "minimum": 0 },
                            "archiveRef": { "type": "string" }
                          },
                          "required": ["kind"]
                        }
                      }
                    }
                  },
                  "refs": {
                    "type": "array",
                    "maxItems": 40,
                    "items": {
                      "type": "object",
                      "properties": {
                        "kind": { "type": "string", "enum": ["message", "trace_item", "archive"] },
                        "messageId": { "type": "string" },
                        "assistantMessageId": { "type": "string" },
                        "sequence": { "type": "integer", "minimum": 0 },
                        "archiveRef": { "type": "string" }
                      },
                      "required": ["kind"]
                    }
                  }
                },
                "required": ["op"]
              }
            }
          },
          "required": ["taskId", "expectedRevision", "operations"]
        }"#,
    )
    .expect("application-owned task_state_patch schema must be valid JSON")
}

fn project_task_state_result(value: &Value) -> Option<Value> {
    let snapshot = serde_json::from_value::<TaskStateSnapshot>(value.clone()).ok()?;
    Some(json!({
        "taskId": snapshot.control.task_id,
        "conversationId": snapshot.control.conversation_id,
        "revision": snapshot.control.revision,
        "status": snapshot.control.status,
        "updatedAt": snapshot.control.updated_at,
        "taskStatePersisted": true
    }))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TaskStatePatchArgs {
    task_id: String,
    expected_revision: u64,
    operations: Vec<TaskStatePatchOperation>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trace_projection_keeps_only_task_identity_and_revision() {
        let value = json!({
            "control": {
                "schemaVersion": 1,
                "taskId": "task-1",
                "conversationId": "conversation-1",
                "objective": "Sensitive exact objective text",
                "sourceMessageId": "message-1",
                "status": "active",
                "revision": 2,
                "currentRunId": "run-1",
                "createdAt": 1,
                "updatedAt": 2
            },
            "checkpoint": {
                "nextActions": ["Sensitive exact next action"]
            }
        });
        let projected = project_task_state_result(&value).unwrap();
        assert_eq!(projected["revision"], 2);
        assert!(projected.get("objective").is_none());
        assert!(!projected.to_string().contains("Sensitive"));
    }

    #[test]
    fn input_schema_is_provider_portable() {
        crate::tools::schema::validate_portable_tool_input_schema(
            "task_state_patch",
            &task_state_patch_schema(),
        )
        .unwrap();
    }
}
