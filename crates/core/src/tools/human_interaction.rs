//! Reviewed model-facing contracts for human questions.
//!
//! Synchronous execution yields an internal suspension which only the runtime driver may settle.
//! Asynchronous execution returns only after Host admission, then continues through normal tool
//! settlement. Both tools require the corresponding Host execution capability.

use crate::protocol::{AgentToolApprovalMode, AgentToolDefinition, AgentToolSafety};
use serde_json::json;

pub(crate) const HUMAN_INTERACTION_CAPABILITY: &str = "human.interaction";
pub(crate) const HUMAN_INTERACTION_ASYNC_CAPABILITY: &str = "human.interaction.async";
pub(crate) const HUMAN_INTERACTION_TOOL_NAMES: [&str; 2] =
    ["request_user_input", "request_user_input_async"];

pub(crate) struct RequestUserInputTool;
pub(crate) struct RequestUserInputAsyncTool;

/// A suspension is not a tool result. No pending acknowledgement enters model context.
pub(crate) enum HumanInteractionToolOutcome {
    Suspended(crate::human_interaction::HumanInteractionToolInput),
}

pub(crate) fn prepare_user_input_suspension(
    args: &serde_json::Value,
) -> crate::AgentResult<HumanInteractionToolOutcome> {
    Ok(HumanInteractionToolOutcome::Suspended(
        parse_human_interaction_input(args)?,
    ))
}

pub(crate) fn parse_human_interaction_input(
    args: &serde_json::Value,
) -> crate::AgentResult<crate::human_interaction::HumanInteractionToolInput> {
    let input = serde_json::from_value(args.clone())
        .map_err(|_| crate::AgentError::new("The human interaction input is invalid."))?;
    crate::human_interaction::validate_human_interaction_tool_input(&input)
        .map_err(|_| crate::AgentError::new("The human interaction input is invalid."))?;
    Ok(input)
}

impl super::AgentTool for RequestUserInputAsyncTool {
    fn definition(&self) -> AgentToolDefinition {
        human_interaction_tool_definitions().remove(1)
    }

    fn execute(
        &self,
        _context: &super::ToolExecutionContext,
        _args: serde_json::Value,
    ) -> crate::AgentResult<serde_json::Value> {
        Err(crate::AgentError::new(
            "Asynchronous questions require the trusted Host admission port.",
        ))
    }

    fn permission_policy(&self) -> super::AgentToolPermissionPolicy {
        super::AgentToolPermissionPolicy::Default
    }

    fn cancellation_settlement(&self) -> super::AgentToolCancellationSettlement {
        // Once admission commits, Stop cannot replace the known accepted fact with a synthetic
        // cancellation result. The run still stops after recording this one authoritative result.
        super::AgentToolCancellationSettlement::Authoritative
    }

    fn exposure(&self) -> super::AgentToolExposure {
        super::AgentToolExposure::RequiresCapability(super::ToolCapabilityId::application_owned(
            HUMAN_INTERACTION_ASYNC_CAPABILITY,
        ))
    }
}

impl super::AgentTool for RequestUserInputTool {
    fn definition(&self) -> AgentToolDefinition {
        human_interaction_tool_definitions().remove(0)
    }

    fn execute(
        &self,
        _context: &super::ToolExecutionContext,
        _args: serde_json::Value,
    ) -> crate::AgentResult<serde_json::Value> {
        Err(crate::AgentError::new(
            "Human input must be suspended by the runtime driver.",
        ))
    }

    fn permission_policy(&self) -> super::AgentToolPermissionPolicy {
        super::AgentToolPermissionPolicy::Default
    }

    fn exposure(&self) -> super::AgentToolExposure {
        super::AgentToolExposure::RequiresCapability(super::ToolCapabilityId::application_owned(
            HUMAN_INTERACTION_CAPABILITY,
        ))
    }
}

pub(crate) fn human_interaction_tool_definitions() -> Vec<AgentToolDefinition> {
    [
        (
            HUMAN_INTERACTION_TOOL_NAMES[0],
            "Ask the human a batch of questions when further useful work depends on the answers. This pauses the current run until the entire batch is submitted; its answers, including explicitly skipped questions, return as this tool's single result. Each question has a title and optional suggested options; the interface also supports custom text or skipping a question. Use only for information or preferences, never permission or tool approval. Do not split one decision across duplicate requests.",
        ),
        (
            HUMAN_INTERACTION_TOOL_NAMES[1],
            "Ask the human a batch of questions while continuing independent work. An accepted result with requestId means only that the questions were recorded, not that the human answered. This tool does not wait or pause the run. Submitted answers arrive later as authenticated human input, never a second result of this call. Continue work that does not depend on the answers; do not poll, repeat questions, guess answers, or treat silence, skipping, or ignoring as agreement. The human may minimize or ignore the entire batch; ignoring does not start another run. Each question has a title and optional suggested options, with custom text and skipping provided by the interface. Never use this for permission or tool approval.",
        ),
    ]
    .into_iter()
    .map(|(name, description)| AgentToolDefinition {
        name: name.to_string(),
        description: description.to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "questions": {
                    "type": "array",
                    "minItems": 1,
                    "items": {
                        "type": "object",
                        "properties": {
                            "title": { "type": "string", "minLength": 1 },
                            "options": {
                                "type": "array",
                                "minItems": 1,
                                "items": { "type": "string", "minLength": 1 }
                            }
                        },
                        "required": ["title"],
                        "additionalProperties": false
                    }
                }
            },
            "required": ["questions"],
            "additionalProperties": false
        }),
        safety: AgentToolSafety::ReadOnly,
        requires_workspace: false,
        requires_approval: false,
        approval_mode: AgentToolApprovalMode::Never,
    })
    .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn human_interaction_specs_are_portable_and_have_no_product_question_limit() {
        let definitions = human_interaction_tool_definitions();
        assert_eq!(definitions.len(), 2);
        for definition in definitions {
            crate::tools::schema::validate_portable_tool_input_schema(
                &definition.name,
                &definition.input_schema,
            )
            .unwrap();
            assert!(definition.input_schema["properties"]["questions"]
                .get("maxItems")
                .is_none());
            assert_eq!(definition.input_schema["additionalProperties"], false);
            assert!(!definition.requires_approval);
            assert_eq!(definition.approval_mode, AgentToolApprovalMode::Never);
            let fields = definition.input_schema["properties"].as_object().unwrap();
            assert_eq!(fields.keys().collect::<Vec<_>>(), vec!["questions"]);
        }
    }
}
