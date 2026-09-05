//! Reviewed model-facing contracts for human input and assistance.
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
            concat!(
                "Request human input or assistance when further progress requires waiting for the human. ",
                "Use it to clarify information, learn preferences, request judgment or decisions, collect feedback, ",
                "or request actions that require the human's personal participation. Items may be questions or concrete requests for assistance.\n\n",
                "Each call creates one batch and pauses the current run until the human submits the entire batch. ",
                "Responses, including explicitly skipped items, return through this call's single tool result.\n\n",
                "Each item should clearly state what participation is needed. Suggested responses are optional and should fit the situation; ",
                "the interface always supports custom text or skipping. This tool does not replace the application's execution permission approval."
            ),
        ),
        (
            HUMAN_INTERACTION_TOOL_NAMES[1],
            concat!(
                "Request human input or assistance while there is still work to advance independently of the response. ",
                "Use it to clarify information, learn preferences, request judgment or decisions, collect feedback, ",
                "or request actions that require the human's personal participation.\n\n",
                "Each call creates one batch and returns immediately after it is recorded; the current run continues. ",
                "accepted/requestId only means the request was recorded: it contains no human response and does not establish that any requested event has occurred. ",
                "Submitted responses arrive later as human input, never as a second tool result of this call.\n\n",
                "While waiting, advance only work that does not depend on the response. Do not poll or duplicate the same request. ",
                "The human may minimize the panel, skip items, or ignore the entire batch; ignoring itself does not trigger a new run. ",
                "Suggested responses should fit the situation, and the interface also supports custom text. ",
                "This tool does not replace the application's execution permission approval."
            ),
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
                    "description": "A batch of questions or requests for human participation, submitted together.",
                    "minItems": 1,
                    "items": {
                        "type": "object",
                        "properties": {
                            "title": {
                                "type": "string",
                                "description": "Clearly state the question or assistance requested, with enough context for the human to respond.",
                                "minLength": 1
                            },
                            "options": {
                                "type": "array",
                                "description": "Optional, context-specific suggested responses. Use meaningful alternatives without a fixed response template; omit for open-ended input. The interface also supports custom text and skipping.",
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
