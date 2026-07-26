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
            "Mark the current explicit goal completed or genuinely blocked. Do not use this for ordinary turn progress, objective edits, pause, resume, or cancellation.",
            json!({
                "type": "object",
                "properties": {
                    "status": {
                        "type": "string",
                        "enum": ["completed", "blocked"]
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
        let goal = context
            .storage()?
            .update_conversation_goal_status(
                context.conversation_id()?,
                args.status,
                None,
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
}
