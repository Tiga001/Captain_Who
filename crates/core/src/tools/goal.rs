use super::{AgentTool, ToolExecutionContext};
use crate::protocol::{
    AgentError, AgentResult, AgentToolApprovalMode, AgentToolDefinition, AgentToolSafety,
};
use crate::ConversationGoalStatus;
use serde::Deserialize;
use serde_json::{json, Value};

pub(super) struct GetGoalTool;
pub(super) struct CreateGoalTool;
pub(super) struct UpdateGoalTool;

impl AgentTool for GetGoalTool {
    fn exposure(&self) -> super::AgentToolExposure {
        super::AgentToolExposure::Stable
    }

    fn permission_policy(&self) -> super::AgentToolPermissionPolicy {
        super::AgentToolPermissionPolicy::Default
    }

    fn definition(&self) -> AgentToolDefinition {
        definition(
            "get_goal",
            "Get the optional persistent goal for the current conversation. Ordinary conversations have no goal.",
            json!({
                "type": "object",
                "properties": {},
                "additionalProperties": false
            }),
        )
    }

    fn execute(&self, context: &ToolExecutionContext, args: Value) -> AgentResult<Value> {
        serde_json::from_value::<EmptyGoalArgs>(args)
            .map_err(|error| AgentError::new(format!("get_goal 参数无效：{error}")))?;
        context.check_cancelled()?;
        let goal = context
            .storage()?
            .load_conversation_goal(context.conversation_id()?)
            .map_err(AgentError::new)?;
        Ok(match goal {
            Some(goal) => json!({
                "goal": {
                    "objective": goal.objective,
                    "status": goal.status
                }
            }),
            None => json!({ "goal": null }),
        })
    }

    fn archives_result(&self) -> bool {
        false
    }

    fn trace_projection(
        &self,
        result: &crate::protocol::AgentToolResult,
    ) -> crate::protocol::AgentToolResult {
        project_goal_result(result)
    }
}

impl AgentTool for CreateGoalTool {
    fn exposure(&self) -> super::AgentToolExposure {
        super::AgentToolExposure::Stable
    }

    fn permission_policy(&self) -> super::AgentToolPermissionPolicy {
        super::AgentToolPermissionPolicy::Default
    }

    fn definition(&self) -> AgentToolDefinition {
        definition(
            "create_goal",
            "Create an optional persistent goal only when the user explicitly asks to track a long-running objective. Do not infer a goal from an ordinary request. Fails while an unfinished goal exists.",
            json!({
                "type": "object",
                "properties": {
                    "objective": {
                        "type": "string",
                        "description": "The explicit long-running objective requested by the user.",
                        "maxLength": crate::MAX_CONVERSATION_GOAL_OBJECTIVE_CHARS
                    }
                },
                "required": ["objective"],
                "additionalProperties": false
            }),
        )
    }

    fn execute(&self, context: &ToolExecutionContext, args: Value) -> AgentResult<Value> {
        let args = serde_json::from_value::<CreateGoalArgs>(args)
            .map_err(|error| AgentError::new(format!("create_goal 参数无效：{error}")))?;
        context.check_cancelled()?;
        let goal = context
            .storage()?
            .create_conversation_goal(
                crate::ConversationGoalMutationActor::Model,
                context.conversation_id()?,
                &args.objective,
                crate::storage::now_ms(),
            )
            .map_err(AgentError::new)?;
        Ok(json!({
            "created": true,
            "status": goal.status
        }))
    }

    fn archives_result(&self) -> bool {
        false
    }

    fn trace_projection(
        &self,
        result: &crate::protocol::AgentToolResult,
    ) -> crate::protocol::AgentToolResult {
        project_goal_result(result)
    }
}

impl AgentTool for UpdateGoalTool {
    fn exposure(&self) -> super::AgentToolExposure {
        super::AgentToolExposure::Stable
    }

    fn permission_policy(&self) -> super::AgentToolPermissionPolicy {
        super::AgentToolPermissionPolicy::Default
    }

    fn definition(&self) -> AgentToolDefinition {
        definition(
            "update_goal",
            "Explicitly change the current Goal to active, completed, blocked, or cancelled. Use active only when the user resumes a blocked Goal; use cancelled only when the user explicitly abandons it. Never infer either from run or approval lifecycle. Completion is rejected while this run has unfinished Todo items.",
            json!({
                "type": "object",
                "properties": {
                    "status": {
                        "type": "string",
                        "enum": ["active", "completed", "blocked", "cancelled"]
                    }
                },
                "required": ["status"],
                "additionalProperties": false
            }),
        )
    }

    fn execute(&self, context: &ToolExecutionContext, args: Value) -> AgentResult<Value> {
        let args = serde_json::from_value::<UpdateGoalArgs>(args)
            .map_err(|error| AgentError::new(format!("update_goal 参数无效：{error}")))?;
        context.check_cancelled()?;
        let runtime = context.goal_runtime_state()?;
        if args.status == ConversationGoalStatus::Completed && runtime.unfinished_todo_items > 0 {
            return Err(AgentError::new(format!(
                "当前 Run 仍有 {} 个未完成 Todo，Goal 不能标记为 completed。",
                runtime.unfinished_todo_items
            )));
        }
        let stopped_reason = match args.status {
            ConversationGoalStatus::Blocked => Some(
                runtime
                    .blocked_reason
                    .unwrap_or_else(|| "The current run reported a genuine blocker.".to_string()),
            ),
            ConversationGoalStatus::Active
            | ConversationGoalStatus::Completed
            | ConversationGoalStatus::Cancelled => None,
        };
        let goal = context
            .storage()?
            .update_conversation_goal_status(
                crate::ConversationGoalMutationActor::Model,
                context.conversation_id()?,
                args.status,
                stopped_reason.as_deref(),
                crate::storage::now_ms(),
            )
            .map_err(AgentError::new)?;
        Ok(json!({
            "updated": true,
            "status": goal.status
        }))
    }

    fn archives_result(&self) -> bool {
        false
    }

    fn trace_projection(
        &self,
        result: &crate::protocol::AgentToolResult,
    ) -> crate::protocol::AgentToolResult {
        project_goal_result(result)
    }
}

fn definition(name: &str, description: &str, input_schema: Value) -> AgentToolDefinition {
    AgentToolDefinition {
        name: name.to_string(),
        description: description.to_string(),
        input_schema,
        safety: AgentToolSafety::ReadOnly,
        requires_workspace: false,
        requires_approval: false,
        approval_mode: AgentToolApprovalMode::Never,
    }
}

fn project_goal_result(
    result: &crate::protocol::AgentToolResult,
) -> crate::protocol::AgentToolResult {
    let mut projected = result.clone();
    projected.result = result.result.as_ref().map(|value| {
        json!({
            "goalPresent": value.get("goal").is_some_and(|goal| !goal.is_null()),
            "created": value.get("created").and_then(Value::as_bool).unwrap_or(false),
            "updated": value.get("updated").and_then(Value::as_bool).unwrap_or(false),
            "status": value.get("status").cloned().or_else(|| {
                value.get("goal").and_then(|goal| goal.get("status")).cloned()
            }),
            "goalStatePersisted": result.ok
        })
    });
    crate::conversation_trace::canonical_tool_result_for_context(&projected)
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct EmptyGoalArgs {}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CreateGoalArgs {
    objective: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct UpdateGoalArgs {
    status: ConversationGoalStatus,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{AgentPermissions, AgentRunContext};
    use crate::storage::models::{ChatConversationRecord, ChatMessageRecord};
    use crate::storage::service::StorageService;
    use crate::tools::{GoalRuntimeState, GoalRuntimeStateReader};
    use std::sync::Arc;
    use tempfile::tempdir;

    #[derive(Debug)]
    struct FixedGoalRuntimeState(GoalRuntimeState);

    impl GoalRuntimeStateReader for FixedGoalRuntimeState {
        fn goal_runtime_state(&self) -> GoalRuntimeState {
            self.0.clone()
        }
    }

    fn goal_test_context(
        storage: Arc<StorageService>,
        runtime: GoalRuntimeState,
    ) -> ToolExecutionContext {
        let run_context = AgentRunContext {
            conversation_id: Some("conversation-goal-tool".to_string()),
            project_id: None,
            workspace: None,
            attachment_library: None,
            permissions: AgentPermissions::default(),
        };
        ToolExecutionContext::from_run_context(Some(&run_context))
            .with_runtime_services("run-goal-tool".to_string(), Some(storage))
            .with_goal_runtime_state_reader(Some(Arc::new(FixedGoalRuntimeState(runtime))))
    }

    fn goal_test_storage() -> (tempfile::TempDir, Arc<StorageService>) {
        let root = tempdir().unwrap();
        let storage = Arc::new(StorageService::open(&root.path().join("storage.sqlite")).unwrap());
        storage
            .save_conversation(ChatConversationRecord {
                id: "conversation-goal-tool".to_string(),
                project_id: None,
                model_id: None,
                title: "Goal tool test".to_string(),
                messages: vec![ChatMessageRecord {
                    id: "goal-source".to_string(),
                    role: "user".to_string(),
                    content: "Track this goal.".to_string(),
                    created_at: 1,
                    status: Some("sent".to_string()),
                    attachments: Vec::new(),
                    agent_run_json: None,
                    ui_state_json: None,
                }],
                created_at: 1,
                updated_at: 1,
                pinned_at: None,
                archived_at: None,
                unread_at: None,
            })
            .unwrap();
        storage
            .create_conversation_goal(
                crate::ConversationGoalMutationActor::User,
                "conversation-goal-tool",
                "Finish the goal.",
                2,
            )
            .unwrap();
        (root, storage)
    }

    #[test]
    fn goal_tool_schemas_are_small_and_provider_portable() {
        for tool in [
            GetGoalTool.definition(),
            CreateGoalTool.definition(),
            UpdateGoalTool.definition(),
        ] {
            crate::tools::schema::validate_portable_tool_input_schema(
                &tool.name,
                &tool.input_schema,
            )
            .unwrap();
            assert!(tool.input_schema.to_string().len() < 600);
        }
    }

    #[test]
    fn durable_projection_does_not_copy_objective() {
        let result = crate::protocol::AgentToolResult {
            exact_archive_file: None,
            call_id: "call-1".to_string(),
            tool: "get_goal".to_string(),
            ok: true,
            result: Some(json!({
                "goal": {
                    "objective": "Sensitive long-running objective",
                    "status": "active"
                }
            })),
            error: None,
        };

        let projected = project_goal_result(&result);
        assert!(!projected.result.unwrap().to_string().contains("Sensitive"));
    }

    #[test]
    fn completion_is_rejected_until_current_run_todo_is_finished() {
        let (_root, storage) = goal_test_storage();
        let pending_context = goal_test_context(
            Arc::clone(&storage),
            GoalRuntimeState {
                unfinished_todo_items: 1,
                blocked_reason: None,
            },
        );

        let error = UpdateGoalTool
            .execute(&pending_context, json!({ "status": "completed" }))
            .unwrap_err()
            .to_string();
        assert!(error.contains("仍有 1 个未完成 Todo"));
        assert_eq!(
            storage
                .load_conversation_goal("conversation-goal-tool")
                .unwrap()
                .unwrap()
                .status,
            ConversationGoalStatus::Active
        );

        let complete_context = goal_test_context(storage.clone(), GoalRuntimeState::default());
        let result = UpdateGoalTool
            .execute(&complete_context, json!({ "status": "completed" }))
            .unwrap();
        assert_eq!(result["status"], "completed");
    }

    #[test]
    fn blocked_reason_is_derived_from_run_todo_instead_of_model_arguments() {
        let (_root, storage) = goal_test_storage();
        let context = goal_test_context(
            Arc::clone(&storage),
            GoalRuntimeState {
                unfinished_todo_items: 1,
                blocked_reason: Some("Need credentials: token missing".to_string()),
            },
        );

        let result = UpdateGoalTool
            .execute(&context, json!({ "status": "blocked" }))
            .unwrap();
        assert_eq!(result["status"], "blocked");
        let goal = storage
            .load_conversation_goal("conversation-goal-tool")
            .unwrap()
            .unwrap();
        assert_eq!(
            goal.stopped_reason.as_deref(),
            Some("Need credentials: token missing")
        );

        let resumed = UpdateGoalTool
            .execute(&context, json!({ "status": "active" }))
            .unwrap();
        assert_eq!(resumed["status"], "active");
        let goal = storage
            .load_conversation_goal("conversation-goal-tool")
            .unwrap()
            .unwrap();
        assert_eq!(goal.status, ConversationGoalStatus::Active);
        assert_eq!(goal.stopped_reason, None);
    }

    #[test]
    fn model_can_cancel_only_through_the_explicit_goal_tool_status() {
        let (_root, storage) = goal_test_storage();
        let context = goal_test_context(Arc::clone(&storage), GoalRuntimeState::default());

        let result = UpdateGoalTool
            .execute(&context, json!({ "status": "cancelled" }))
            .unwrap();
        assert_eq!(result["status"], "cancelled");
        let goal = storage
            .load_conversation_goal("conversation-goal-tool")
            .unwrap()
            .unwrap();
        assert_eq!(goal.status, ConversationGoalStatus::Cancelled);
        assert_eq!(goal.stopped_reason, None);
    }
}
