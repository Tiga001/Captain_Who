use super::{
    AgentTool, AgentToolCancellationSettlement, AgentToolExecutionValue, AgentToolExposure,
    AgentToolPermissionPolicy, AgentToolResultPersistence, AsyncAgentTool, BoxAgentToolFuture,
    ToolCapabilityId, ToolExecutionContext, AGENT_COLLABORATION_CAPABILITY,
};
use crate::protocol::{
    AgentError, AgentResult, AgentToolApprovalMode, AgentToolDefinition, AgentToolSafety,
};
use crate::{
    AgentCollaborationAction, AgentCollaborationExecutionControl,
    AgentCollaborationResultPersistence, AgentForkTurns, AgentMessageRequest, AgentSpawnRequest,
    AgentWaitRequest, ReasoningEffort, AGENT_COLLABORATION_DEFAULT_WAIT_MS,
    AGENT_COLLABORATION_MAX_AGENT_TYPE_BYTES, AGENT_COLLABORATION_MAX_MODEL_CONFIG_ID_BYTES,
    AGENT_COLLABORATION_MAX_TASK_NAME_BYTES, AGENT_COLLABORATION_MAX_WAIT_MS,
    AGENT_COLLABORATION_MAX_WAIT_TARGETS, ROOT_AGENT_TASK_NAME,
    ROOT_AGENT_TASK_NAME_RESERVED_MESSAGE,
};
use serde::Deserialize;
use serde_json::{json, Value};

pub(super) struct AgentCollaborationTool {
    kind: AgentCollaborationToolKind,
}

#[derive(Clone, Copy)]
pub(super) enum AgentCollaborationToolKind {
    Spawn,
    SendMessage,
    FollowupTask,
    Wait,
    List,
    Interrupt,
}

impl AgentCollaborationTool {
    pub(super) fn new(kind: AgentCollaborationToolKind) -> Self {
        Self { kind }
    }

    fn action(&self, args: Value) -> AgentResult<AgentCollaborationAction> {
        match self.kind {
            AgentCollaborationToolKind::Spawn => {
                let input: SpawnArgs = parse_args("spawn_agent", args)?;
                let task_name = nonempty(
                    "task_name",
                    input.task_name,
                    AGENT_COLLABORATION_MAX_TASK_NAME_BYTES,
                )?;
                if task_name == ROOT_AGENT_TASK_NAME {
                    return Err(argument_error(
                        "task_name",
                        ROOT_AGENT_TASK_NAME_RESERVED_MESSAGE,
                    ));
                }
                Ok(AgentCollaborationAction::Spawn(AgentSpawnRequest {
                    task_name,
                    message: nonempty("message", input.message, 64 * 1024)?,
                    agent_type: optional_selector(
                        "agent_type",
                        input.agent_type,
                        AGENT_COLLABORATION_MAX_AGENT_TYPE_BYTES,
                    )?,
                    model_config_id: optional_selector(
                        "model",
                        input.model,
                        AGENT_COLLABORATION_MAX_MODEL_CONFIG_ID_BYTES,
                    )?,
                    reasoning_effort: input.reasoning_effort,
                    fork_turns: parse_fork_turns(input.fork_turns)?,
                }))
            }
            AgentCollaborationToolKind::SendMessage => Ok(AgentCollaborationAction::SendMessage(
                parse_message_args("send_message", args)?,
            )),
            AgentCollaborationToolKind::FollowupTask => Ok(AgentCollaborationAction::FollowupTask(
                parse_message_args("followup_task", args)?,
            )),
            AgentCollaborationToolKind::Wait => {
                let input: WaitArgs = parse_args("wait_agent", args)?;
                if input.targets.is_empty()
                    || input.targets.len() > AGENT_COLLABORATION_MAX_WAIT_TARGETS
                {
                    return Err(argument_error(
                        "targets",
                        format!(
                            "must contain 1..={AGENT_COLLABORATION_MAX_WAIT_TARGETS} exact task names"
                        ),
                    ));
                }
                let mut targets = Vec::with_capacity(input.targets.len());
                for target in input.targets {
                    let target =
                        nonempty("targets", target, AGENT_COLLABORATION_MAX_TASK_NAME_BYTES)?;
                    if targets.contains(&target) {
                        return Err(argument_error("targets", "must not contain duplicates"));
                    }
                    targets.push(target);
                }
                let timeout_ms = input
                    .timeout_ms
                    .unwrap_or(AGENT_COLLABORATION_DEFAULT_WAIT_MS);
                if timeout_ms > AGENT_COLLABORATION_MAX_WAIT_MS {
                    return Err(argument_error(
                        "timeout_ms",
                        format!("must be <= {AGENT_COLLABORATION_MAX_WAIT_MS}"),
                    ));
                }
                Ok(AgentCollaborationAction::Wait(AgentWaitRequest {
                    target_task_names: targets,
                    timeout_ms,
                }))
            }
            AgentCollaborationToolKind::List => {
                parse_args::<EmptyArgs>("list_agents", args)?;
                Ok(AgentCollaborationAction::List)
            }
            AgentCollaborationToolKind::Interrupt => {
                let input: TargetArgs = parse_args("interrupt_agent", args)?;
                Ok(AgentCollaborationAction::Interrupt {
                    target_task_name: nonempty(
                        "target",
                        input.target,
                        AGENT_COLLABORATION_MAX_TASK_NAME_BYTES,
                    )?,
                })
            }
        }
    }
}

impl AgentTool for AgentCollaborationTool {
    fn definition(&self) -> AgentToolDefinition {
        let (name, description, input_schema) = match self.kind {
            AgentCollaborationToolKind::Spawn => (
                "spawn_agent",
                "Create one direct persistent child Agent and queue its initial task. Exact agent_type and model selectors must come from the collaboration directory. For visual work, select only a directory entry whose authoritative imageInput capability is true; never infer capability from a name.",
                json!({
                    "type": "object",
                    "properties": {
                        "task_name": { "type": "string", "minLength": 1, "maxLength": AGENT_COLLABORATION_MAX_TASK_NAME_BYTES, "description": "Short, meaningful task name unique in the current Agent tree, such as frontend-review. 主智能体 is reserved for the root Agent and cannot name a child. Do not use a conversation title or full instructions as the name. Use this exact name to address the Agent in every later collaboration call." },
                        "message": { "type": "string", "minLength": 1, "maxLength": 65536 },
                        "agent_type": { "type": "string", "minLength": 1, "maxLength": AGENT_COLLABORATION_MAX_AGENT_TYPE_BYTES, "description": "Optional exact template machine key." },
                        "model": { "type": "string", "minLength": 1, "maxLength": AGENT_COLLABORATION_MAX_MODEL_CONFIG_ID_BYTES, "description": "Optional exact model_config_id." },
                        "reasoning_effort": { "type": "string", "enum": ["high", "max"] },
                        "fork_turns": {
                            "description": "none, all, or a positive logical-turn count.",
                            "anyOf": [
                                { "type": "string", "enum": ["none", "all"] },
                                { "type": "integer", "minimum": 1, "maximum": 4294967295_u64 }
                            ]
                        }
                    },
                    "required": ["task_name", "message"],
                    "additionalProperties": false
                }),
            ),
            AgentCollaborationToolKind::SendMessage => (
                "send_message",
                "Use this mailbox-only tool for a child Agent to report progress, request help, or send supplemental information to its parent. It only enqueues a message: it never creates a Wake or Turn, never starts or resumes execution, and never wakes a completed, failed, interrupted, or idle Agent. Do not use it to assign, revise, or repeat work. A queued message is not evidence that the target is working; parent-to-descendant work must use followup_task.",
                message_schema(),
            ),
            AgentCollaborationToolKind::FollowupTask => (
                "followup_task",
                "Parent/ancestor-to-descendant task assignment. Use this to start, continue, revise, or repeat work on an existing descendant, including one whose latest task is completed, failed, interrupted, or idle. It reliably enqueues the follow-up and guarantees a future execution opportunity without starting a concurrent Turn. Use send_message only for child-to-parent mailbox reports, not task assignment.",
                message_schema(),
            ),
            AgentCollaborationToolKind::Wait => (
                "wait_agent",
                "Wait for the first ready result, message, update, or status change from one or more descendant Agents. This is independent from command_session waits.",
                json!({
                    "type": "object",
                    "properties": {
                        "targets": {
                            "type": "array",
                            "minItems": 1,
                            "maxItems": AGENT_COLLABORATION_MAX_WAIT_TARGETS,
                            "items": { "type": "string", "minLength": 1, "maxLength": AGENT_COLLABORATION_MAX_TASK_NAME_BYTES, "description": "Exact taskName returned by spawn_agent or list_agents; Agent IDs and task paths are not accepted." }
                        },
                        "timeout_ms": {
                            "type": "integer",
                            "minimum": 0,
                            "maximum": AGENT_COLLABORATION_MAX_WAIT_MS,
                            "default": AGENT_COLLABORATION_DEFAULT_WAIT_MS
                        }
                    },
                    "required": ["targets"],
                    "additionalProperties": false
                }),
            ),
            AgentCollaborationToolKind::List => (
                "list_agents",
                "List the concise, authorized Agent-tree snapshot visible to the current Agent.",
                json!({ "type": "object", "properties": {}, "additionalProperties": false }),
            ),
            AgentCollaborationToolKind::Interrupt => (
                "interrupt_agent",
                "Request interruption of a descendant Agent's current Turn without deleting its identity, conversation, or history.",
                json!({
                    "type": "object",
                    "properties": {
                        "target": { "type": "string", "minLength": 1, "maxLength": AGENT_COLLABORATION_MAX_TASK_NAME_BYTES, "description": "Exact taskName returned by spawn_agent or list_agents; Agent IDs and task paths are not accepted." }
                    },
                    "required": ["target"],
                    "additionalProperties": false
                }),
            ),
        };
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

    fn execute(&self, _context: &ToolExecutionContext, _args: Value) -> AgentResult<Value> {
        Err(AgentError::new(
            "Agent collaboration tools require the asynchronous Host execution path.",
        ))
    }

    fn permission_policy(&self) -> AgentToolPermissionPolicy {
        AgentToolPermissionPolicy::Default
    }

    fn exposure(&self) -> AgentToolExposure {
        AgentToolExposure::RequiresCapability(ToolCapabilityId::application_owned(
            AGENT_COLLABORATION_CAPABILITY,
        ))
    }

    fn archives_result(&self) -> bool {
        false
    }

    fn cancellation_settlement(&self) -> AgentToolCancellationSettlement {
        match self.kind {
            // These operations may cross a durable commit boundary inside the Host. Await the
            // Host's bounded, authoritative settlement so cancellation can never detach a future
            // that has spawned an Agent, enqueued a message, consumed a wait receipt, or requested
            // an interrupt without returning the corresponding ToolResult.
            AgentCollaborationToolKind::Spawn
            | AgentCollaborationToolKind::SendMessage
            | AgentCollaborationToolKind::FollowupTask
            | AgentCollaborationToolKind::Wait
            | AgentCollaborationToolKind::Interrupt => {
                AgentToolCancellationSettlement::Authoritative
            }
            AgentCollaborationToolKind::List => AgentToolCancellationSettlement::Interruptible,
        }
    }
}

impl AsyncAgentTool for AgentCollaborationTool {
    fn execute_async<'a>(
        &'a self,
        context: &'a ToolExecutionContext,
        args: Value,
    ) -> BoxAgentToolFuture<'a> {
        Box::pin(async move {
            context.check_cancelled()?;
            let action = self.action(args)?;
            let (services, invocation) = context.agent_collaboration_invocation(action)?;
            let output = services
                .executor
                .execute(
                    invocation,
                    AgentCollaborationExecutionControl::new(
                        context.cancellation_token(),
                        context.steer_input(),
                    ),
                )
                .await?;
            let persistence = match output.persistence {
                AgentCollaborationResultPersistence::RuntimeCommits => {
                    AgentToolResultPersistence::RuntimeCommits
                }
                AgentCollaborationResultPersistence::PrecommittedWaitToolResult => {
                    AgentToolResultPersistence::PrecommittedTrace
                }
            };
            Ok(AgentToolExecutionValue {
                value: output.result.into_model_value()?,
                persistence,
            })
        })
    }
}

fn message_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "target": { "type": "string", "minLength": 1, "maxLength": AGENT_COLLABORATION_MAX_TASK_NAME_BYTES, "description": "Exact taskName returned by spawn_agent or list_agents; Agent IDs and task paths are not accepted." },
            "message": { "type": "string", "minLength": 1, "maxLength": 65536 }
        },
        "required": ["target", "message"],
        "additionalProperties": false
    })
}

fn parse_message_args(tool: &str, args: Value) -> AgentResult<AgentMessageRequest> {
    let input: MessageArgs = parse_args(tool, args)?;
    Ok(AgentMessageRequest {
        target_task_name: nonempty(
            "target",
            input.target,
            AGENT_COLLABORATION_MAX_TASK_NAME_BYTES,
        )?,
        message: nonempty("message", input.message, 64 * 1024)?,
    })
}

fn parse_args<T: for<'de> Deserialize<'de>>(tool: &str, args: Value) -> AgentResult<T> {
    serde_json::from_value(args).map_err(|error| {
        AgentError::structured(
            "agent.collaboration.invalid_arguments",
            format!("{tool} arguments are invalid: {error}"),
            json!({
                "type": "agent_collaboration",
                "category": "invalid_arguments",
                "retryable": false,
                "tool": tool,
            }),
        )
    })
}

fn nonempty(field: &'static str, value: String, maximum: usize) -> AgentResult<String> {
    if value.trim().is_empty()
        || value.trim() != value
        || value.len() > maximum
        || value.contains('\0')
    {
        return Err(argument_error(
            field,
            format!("must be non-empty, trimmed, NUL-free, and <= {maximum} bytes"),
        ));
    }
    Ok(value)
}

fn optional_selector(
    field: &'static str,
    value: Option<String>,
    maximum: usize,
) -> AgentResult<Option<String>> {
    value
        .map(|value| nonempty(field, value, maximum))
        .transpose()
}

fn argument_error(field: &'static str, reason: impl Into<String>) -> AgentError {
    AgentError::structured(
        "agent.collaboration.invalid_arguments",
        format!("Invalid {field}."),
        json!({
            "type": "agent_collaboration",
            "category": "invalid_arguments",
            "field": field,
            "reason": reason.into(),
            "retryable": false,
        }),
    )
}

fn parse_fork_turns(value: Option<ForkTurnsArg>) -> AgentResult<AgentForkTurns> {
    match value.unwrap_or(ForkTurnsArg::Name(ForkTurnsName::None)) {
        ForkTurnsArg::Name(ForkTurnsName::None) => Ok(AgentForkTurns::None),
        ForkTurnsArg::Name(ForkTurnsName::All) => Ok(AgentForkTurns::All),
        ForkTurnsArg::Count(count) if count > 0 => u32::try_from(count)
            .map(AgentForkTurns::Last)
            .map_err(|_| argument_error("fork_turns", "count exceeds u32")),
        ForkTurnsArg::Count(_) => Err(argument_error("fork_turns", "count must be positive")),
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SpawnArgs {
    task_name: String,
    message: String,
    #[serde(default)]
    agent_type: Option<String>,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    reasoning_effort: Option<ReasoningEffort>,
    #[serde(default)]
    fork_turns: Option<ForkTurnsArg>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum ForkTurnsArg {
    Name(ForkTurnsName),
    Count(u64),
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum ForkTurnsName {
    None,
    All,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct MessageArgs {
    target: String,
    message: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WaitArgs {
    targets: Vec<String>,
    #[serde(default)]
    timeout_ms: Option<u64>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TargetArgs {
    target: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct EmptyArgs {}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    #[derive(Default)]
    struct CountingWaitExecutor {
        calls: AtomicUsize,
    }

    impl crate::AgentCollaborationExecutor for CountingWaitExecutor {
        fn execute(
            &self,
            _invocation: crate::AgentCollaborationInvocation,
            _control: crate::AgentCollaborationExecutionControl,
        ) -> crate::AgentCollaborationExecutionFuture {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Box::pin(async {
                Ok(crate::AgentCollaborationExecutionOutput {
                    result: crate::AgentCollaborationToolResult::WaitStopped {
                        reason: "fixture".to_string(),
                    },
                    persistence:
                        crate::AgentCollaborationResultPersistence::PrecommittedWaitToolResult,
                })
            })
        }
    }

    fn collaboration_context(
        executor: Arc<CountingWaitExecutor>,
    ) -> (
        ToolExecutionContext,
        crate::AgentCollaborationRuntimeServices,
    ) {
        let run_context = crate::AgentRunContext {
            conversation_id: Some("conversation-root".to_string()),
            project_id: Some("project-root".to_string()),
            workspace: None,
            attachment_library: None,
            permissions: crate::AgentPermissions::default(),
            collaboration_identity: None,
        };
        let services = crate::AgentCollaborationRuntimeServices::new(
            executor,
            crate::AgentCollaborationCaller {
                agent_id: "agent-root".to_string(),
                root_agent_id: "agent-root".to_string(),
                root_conversation_id: "conversation-root".to_string(),
                parent_agent_id: None,
                conversation_id: "conversation-root".to_string(),
                project_id: Some("project-root".to_string()),
                task_name: "Root".to_string(),
                task_path: "/root".to_string(),
            },
            crate::AgentCollaborationSelectorDirectory::default(),
        );
        let context = ToolExecutionContext::from_run_context(Some(&run_context))
            .with_runtime_services("run-root".to_string(), None)
            .with_agent_collaboration(Some(services.clone()), Some("assistant-root".to_string()))
            .with_model_batch_index(7);
        (context, services)
    }

    #[test]
    fn tool_set_is_exact_and_schemas_are_portable() {
        let definitions = [
            AgentCollaborationToolKind::Spawn,
            AgentCollaborationToolKind::SendMessage,
            AgentCollaborationToolKind::FollowupTask,
            AgentCollaborationToolKind::Wait,
            AgentCollaborationToolKind::List,
            AgentCollaborationToolKind::Interrupt,
        ]
        .map(|kind| AgentCollaborationTool::new(kind).definition());
        assert_eq!(
            definitions
                .iter()
                .map(|definition| definition.name.as_str())
                .collect::<Vec<_>>(),
            crate::AGENT_COLLABORATION_TOOL_NAMES
        );
        for definition in definitions {
            super::super::schema::validate_portable_tool_input_schema(
                &definition.name,
                &definition.input_schema,
            )
            .unwrap();
        }
    }

    #[test]
    fn strict_arguments_and_wait_bounds_fail_closed() {
        let spawn = AgentCollaborationTool::new(AgentCollaborationToolKind::Spawn);
        assert!(spawn
            .action(json!({"task_name":"Review","message":"Check","sender":"forged"}))
            .is_err());
        assert!(spawn
            .action(json!({
                "task_name":"Review",
                "message":"Check",
                "permissions": {"read":"all","write":"all"}
            }))
            .is_err());
        let wait = AgentCollaborationTool::new(AgentCollaborationToolKind::Wait);
        assert!(wait.action(json!({"targets":[],"timeout_ms":0})).is_err());
        assert!(wait
            .action(json!({"targets":["agent-a"],"timeout_ms":300001}))
            .is_err());
    }

    #[test]
    fn spawn_rejects_the_root_reserved_name_before_host_dispatch() {
        let spawn = AgentCollaborationTool::new(AgentCollaborationToolKind::Spawn);
        let error = spawn
            .action(json!({"task_name": ROOT_AGENT_TASK_NAME, "message": "Check"}))
            .unwrap_err();
        assert_eq!(error.code(), Some("agent.collaboration.invalid_arguments"));
        let details = error.details().unwrap();
        assert_eq!(details["field"], "task_name");
        assert!(details["reason"]
            .as_str()
            .unwrap()
            .contains("reserved for the root Agent"));
        assert!(spawn
            .action(json!({"task_name": "核心检查", "message": "Check"}))
            .is_ok());
    }

    #[test]
    fn target_schemas_and_actions_use_exact_task_names_with_spawn_name_limits() {
        let name = "r".repeat(AGENT_COLLABORATION_MAX_TASK_NAME_BYTES);
        for kind in [
            AgentCollaborationToolKind::SendMessage,
            AgentCollaborationToolKind::FollowupTask,
        ] {
            let tool = AgentCollaborationTool::new(kind);
            let action = tool
                .action(json!({"target": name, "message": "Continue"}))
                .unwrap();
            match action {
                AgentCollaborationAction::SendMessage(request)
                | AgentCollaborationAction::FollowupTask(request) => {
                    assert_eq!(request.target_task_name, name)
                }
                _ => panic!("expected a message action"),
            }
            let target = tool.definition().input_schema["properties"]["target"].clone();
            assert_eq!(target["maxLength"], AGENT_COLLABORATION_MAX_TASK_NAME_BYTES);
            assert!(target["description"]
                .as_str()
                .unwrap()
                .contains("Exact taskName"));
            assert!(tool
                .action(json!({"target": format!(" {name}"), "message": "Continue"}))
                .is_err());
        }
        let wait = AgentCollaborationTool::new(AgentCollaborationToolKind::Wait);
        let AgentCollaborationAction::Wait(request) = wait
            .action(json!({"targets": [name], "timeout_ms": 0}))
            .unwrap()
        else {
            panic!("expected wait action");
        };
        assert_eq!(request.target_task_names, vec![name.clone()]);
        assert!(wait.action(json!({"targets": [name, name]})).is_err());
        let interrupt = AgentCollaborationTool::new(AgentCollaborationToolKind::Interrupt);
        assert_eq!(
            interrupt.action(json!({"target": name})).unwrap(),
            AgentCollaborationAction::Interrupt {
                target_task_name: name
            }
        );
    }

    #[test]
    fn message_and_followup_descriptions_separate_reporting_from_task_assignment() {
        let send =
            AgentCollaborationTool::new(AgentCollaborationToolKind::SendMessage).definition();
        assert!(send
            .description
            .contains("for a child Agent to report progress"));
        assert!(send.description.contains("never creates a Wake or Turn"));
        assert!(send
            .description
            .contains("queued message is not evidence that the target is working"));
        assert!(send.description.contains("must use followup_task"));

        let followup =
            AgentCollaborationTool::new(AgentCollaborationToolKind::FollowupTask).definition();
        assert!(followup
            .description
            .contains("Parent/ancestor-to-descendant task assignment"));
        assert!(followup
            .description
            .contains("guarantees a future execution opportunity"));
        assert!(followup
            .description
            .contains("Use send_message only for child-to-parent mailbox reports"));
    }

    #[test]
    fn registry_exposes_all_six_only_after_host_capability_is_enabled() {
        let mut registry = super::super::ToolRegistry::defaults_with_search(None);
        assert!(crate::AGENT_COLLABORATION_TOOL_NAMES
            .iter()
            .all(|name| registry.definition_for(name).is_none()));
        registry.register_agent_collaboration_tools();
        let present = crate::AGENT_COLLABORATION_TOOL_NAMES
            .iter()
            .filter(|name| registry.definition_for(name).is_some())
            .count();
        assert_eq!(present, 6);
        registry.register_agent_collaboration_tools();
        assert_eq!(
            crate::AGENT_COLLABORATION_TOOL_NAMES
                .iter()
                .filter(|name| registry.definition_for(name).is_some())
                .count(),
            6
        );
    }

    #[test]
    fn durable_collaboration_effects_settle_authoritatively_on_cancellation() {
        for kind in [
            AgentCollaborationToolKind::Spawn,
            AgentCollaborationToolKind::SendMessage,
            AgentCollaborationToolKind::FollowupTask,
            AgentCollaborationToolKind::Wait,
            AgentCollaborationToolKind::Interrupt,
        ] {
            assert_eq!(
                AgentCollaborationTool::new(kind).cancellation_settlement(),
                AgentToolCancellationSettlement::Authoritative
            );
        }
        assert_eq!(
            AgentCollaborationTool::new(AgentCollaborationToolKind::List).cancellation_settlement(),
            AgentToolCancellationSettlement::Interruptible
        );
    }

    #[test]
    fn invocation_carries_the_host_frozen_caller_model_capability() {
        let executor = Arc::new(CountingWaitExecutor::default());
        let (context, _) = collaboration_context(executor);
        let context = context
            .with_model_capabilities(crate::ModelCapabilities { image_input: true })
            .with_tool_call_id("call-capability-projection".to_string());
        let (_, invocation) = context
            .agent_collaboration_invocation(crate::AgentCollaborationAction::List)
            .unwrap();
        assert_eq!(
            invocation.caller_model_capabilities,
            crate::ModelCapabilities { image_input: true }
        );
    }

    #[tokio::test]
    async fn one_provider_batch_admits_only_one_durable_wait_and_persists_the_second_failure() {
        let executor = Arc::new(CountingWaitExecutor::default());
        let (context, services) = collaboration_context(Arc::clone(&executor));
        let mut registry = super::super::ToolRegistry::defaults_with_search(None);
        registry.register_agent_collaboration_tools();
        let registry = Arc::new(registry);
        let call = |id: &str, target: &str| crate::AgentToolCall {
            id: id.to_string(),
            tool: "wait_agent".to_string(),
            args: json!({"targets": [target], "timeout_ms": 0}),
            approval_status: crate::AgentApprovalStatus::NotRequired,
            reason: None,
        };

        let first = Arc::clone(&registry)
            .execute_async(
                context.clone(),
                call("wait-first", "agent-child-a"),
                crate::AgentCancellationToken::new(),
            )
            .await
            .unwrap();
        assert!(first.result.ok);
        assert_eq!(
            first.persistence,
            AgentToolResultPersistence::PrecommittedTrace
        );

        let second = Arc::clone(&registry)
            .execute_async(
                context,
                call("wait-second", "agent-child-b"),
                crate::AgentCancellationToken::new(),
            )
            .await
            .unwrap();
        assert!(!second.result.ok);
        assert_eq!(
            second.persistence,
            AgentToolResultPersistence::RuntimeCommits
        );
        assert_eq!(
            second.result.result.as_ref().unwrap()["errorCode"],
            "agent.collaboration.wait_batch_conflict"
        );
        assert_eq!(executor.calls.load(Ordering::SeqCst), 1);

        let snapshot = services.run_snapshot();
        assert_eq!(snapshot.admitted_wait_model_batches, vec![7]);
        let restored = crate::AgentCollaborationRuntimeServices::new(
            executor,
            services.caller.clone(),
            crate::AgentCollaborationSelectorDirectory::default(),
        )
        .with_run_snapshot(&snapshot)
        .unwrap();
        assert!(!restored.try_admit_wait_model_batch(7).unwrap());
        assert!(restored.try_admit_wait_model_batch(8).unwrap());
    }
}
