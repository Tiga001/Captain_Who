use super::{
    AgentTool, AgentToolCancellationSettlement, AsyncAgentTool, BoxAgentToolFuture,
    ToolExecutionContext,
};
use crate::command::CommandSessionId;
use crate::protocol::{
    AgentCommandSessionStatus, AgentError, AgentResult, AgentToolApprovalMode, AgentToolDefinition,
    AgentToolResult, AgentToolSafety,
};
use crate::runtime::{
    AgentCommandSessionAction, AgentCommandSessionExecutionControl,
    AgentCommandSessionExecutionOutput, AgentCommandSessionExecutionRequest,
    AGENT_COMMAND_SESSION_DEFAULT_WAIT_MS, AGENT_COMMAND_SESSION_INTERRUPT_WAIT_MS,
    AGENT_COMMAND_SESSION_MAX_WAIT_MS, AGENT_COMMAND_SESSION_MODEL_OUTPUT_BYTES,
};
use serde::Deserialize;
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use std::sync::Arc;

const LEGACY_COMMAND_SESSION_MIN_WAIT_MS: u64 = 30_000;

pub(super) struct CommandSessionTool;

impl AgentTool for CommandSessionTool {
    fn exposure(&self) -> super::AgentToolExposure {
        super::AgentToolExposure::Stable
    }

    fn permission_policy(&self) -> super::AgentToolPermissionPolicy {
        super::AgentToolPermissionPolicy::Default
    }

    fn definition(&self) -> AgentToolDefinition {
        AgentToolDefinition {
            name: "command_session".to_string(),
            description: "Wait for or interrupt a managed command returned by run_command with status=running. action defaults to wait. wait lets the Host quietly collect incremental output until the command reaches a terminal state or the Host's bounded deadline; ordinary output does not require repeated calls. For a GUI app or long-lived server, normally continue without waiting for natural exit. For a build or test whose result is required, call wait and do not emit repeated waiting narration. A running status is not final success, and background exit never starts a new model turn. Arbitrary stdin is not supported.".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "sessionId": {
                        "type": "string",
                        "pattern": "^cmd_[0-9a-fA-F]{32}$",
                        "description": "The exact sessionId returned by run_command."
                    },
                    "action": {
                        "type": "string",
                        "enum": ["wait", "interrupt"],
                        "description": "Defaults to wait. wait observes terminal state through a Host-bounded quiet wait; interrupt sends a controlled interrupt to this Session."
                    }
                },
                "required": ["sessionId"],
                "additionalProperties": false
            }),
            // `interrupt` is a narrow control operation over an already-authorized Session and
            // must not open a second approval lifecycle.
            safety: AgentToolSafety::ReadOnly,
            requires_workspace: false,
            requires_approval: false,
            approval_mode: AgentToolApprovalMode::Never,
        }
    }

    fn execute(&self, context: &ToolExecutionContext, args: Value) -> AgentResult<Value> {
        let (session_id, request, control) = prepare_execution(context, args)?;
        let output = context
            .command_session_executor()?
            .execute_command_session(request, control)?;
        validate_host_output(&session_id, &output)?;
        Ok(model_result(output))
    }

    fn cancellation_settlement(&self) -> AgentToolCancellationSettlement {
        // The Host observes cancellation itself and returns without signaling the handed-off
        // process or committing a model-read receipt. Await that bounded authoritative outcome
        // instead of detaching a blocking worker which could consume the cursor after Run stop.
        AgentToolCancellationSettlement::Authoritative
    }

    fn archives_result(&self) -> bool {
        // Session output is archived exactly once by terminal Session settlement. Polling it must
        // never recursively create another Exact History copy.
        false
    }

    fn trace_projection(&self, result: &AgentToolResult) -> AgentToolResult {
        let mut projected = result.clone();
        if let Some(value) = projected.result.as_ref() {
            projected.result = Some(trace_result(value));
        }
        crate::conversation_trace::canonical_tool_result_for_context(&projected)
    }
}

impl AsyncAgentTool for CommandSessionTool {
    fn execute_async<'a>(
        &'a self,
        context: &'a ToolExecutionContext,
        args: Value,
    ) -> BoxAgentToolFuture<'a> {
        Box::pin(async move {
            let (session_id, request, control) = prepare_execution(context, args)?;
            let executor = Arc::clone(context.command_session_executor()?);
            let output = tokio::task::spawn_blocking(move || {
                executor.execute_command_session(request, control)
            })
            .await
            .map_err(|error| {
                AgentError::structured(
                    "agent.command_session_join_failed",
                    format!("命令 Session 观察线程失败：{error}"),
                    json!({
                        "type": "command_session",
                        "code": "commandSessionJoinFailed"
                    }),
                )
            })??;
            validate_host_output(&session_id, &output)?;
            Ok(model_result(output))
        })
    }
}

fn prepare_execution(
    context: &ToolExecutionContext,
    args: Value,
) -> AgentResult<(
    String,
    AgentCommandSessionExecutionRequest,
    AgentCommandSessionExecutionControl,
)> {
    let input: CommandSessionInput = serde_json::from_value(args)
        .map_err(|error| AgentError::new(format!("command_session 参数无效：{error}")))?;
    context.check_cancelled()?;

    let session_id = CommandSessionId::parse(&input.session_id)
        .map_err(|_| AgentError::new("command_session.sessionId 格式无效。"))?;
    let action = match input.action.unwrap_or_default() {
        CommandSessionInputAction::Wait => AgentCommandSessionAction::Poll,
        CommandSessionInputAction::Interrupt => AgentCommandSessionAction::Interrupt,
    };
    // `waitMs` was exposed by the v1 model contract. Keep accepting it so a durable
    // checkpoint or an older provider response can resume safely, but make timing a Host
    // policy for new calls and clamp the legacy hint instead of spending another model turn on
    // a harmless range error.
    let wait_ms = normalize_legacy_wait_ms(input.wait_ms, action);
    let request = AgentCommandSessionExecutionRequest {
        conversation_id: context.conversation_id()?.to_string(),
        run_id: context.run_id()?.to_string(),
        call_id: context.tool_call_id()?.to_string(),
        session_id: session_id.as_str().to_string(),
        action,
        wait_ms,
        max_output_bytes: AGENT_COMMAND_SESSION_MODEL_OUTPUT_BYTES,
    };
    let control = AgentCommandSessionExecutionControl::new(
        context.cancellation_token(),
        context.steer_input(),
    );
    Ok((session_id.as_str().to_string(), request, control))
}

fn normalize_legacy_wait_ms(wait_ms: Option<u64>, action: AgentCommandSessionAction) -> u64 {
    match wait_ms {
        None | Some(0) => match action {
            AgentCommandSessionAction::Poll => AGENT_COMMAND_SESSION_DEFAULT_WAIT_MS,
            AgentCommandSessionAction::Interrupt => AGENT_COMMAND_SESSION_INTERRUPT_WAIT_MS,
        },
        Some(wait_ms) => wait_ms.clamp(
            LEGACY_COMMAND_SESSION_MIN_WAIT_MS.min(AGENT_COMMAND_SESSION_MAX_WAIT_MS),
            AGENT_COMMAND_SESSION_MAX_WAIT_MS,
        ),
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CommandSessionInput {
    session_id: String,
    #[serde(default)]
    action: Option<CommandSessionInputAction>,
    #[serde(default)]
    wait_ms: Option<u64>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
enum CommandSessionInputAction {
    #[default]
    #[serde(alias = "poll")]
    Wait,
    Interrupt,
}

fn validate_host_output(
    requested_session_id: &str,
    output: &AgentCommandSessionExecutionOutput,
) -> AgentResult<()> {
    if output.session_id != requested_session_id {
        return Err(invalid_host_output("Host 返回了不匹配的 sessionId。"));
    }
    if output.requested_after_sequence > output.latest_sequence {
        return Err(invalid_host_output(
            "Host 返回的 requestedAfterSequence 超过 latestSequence。",
        ));
    }
    match (output.first_output_sequence, output.last_output_sequence) {
        (None, None) if output.output.is_empty() => {}
        (Some(first), Some(last))
            if first > output.requested_after_sequence
                && first <= last
                && last <= output.latest_sequence => {}
        _ => {
            return Err(invalid_host_output(
                "Host 返回的输出正文与 sequence 范围不一致。",
            ));
        }
    }
    if output.output.len() > AGENT_COMMAND_SESSION_MODEL_OUTPUT_BYTES {
        return Err(invalid_host_output("Host 返回的命令输出超过模型读取上限。"));
    }
    if matches!(
        output.status,
        AgentCommandSessionStatus::Starting | AgentCommandSessionStatus::Running
    ) && output.exit_code.is_some()
    {
        return Err(invalid_host_output(
            "Host 为未结束的命令 Session 返回了 exitCode。",
        ));
    }
    Ok(())
}

fn invalid_host_output(message: &str) -> AgentError {
    AgentError::structured(
        "agent.command_session_invalid_host_output",
        message,
        json!({
            "type": "command_session",
            "code": "commandSessionInvalidHostOutput"
        }),
    )
}

fn model_result(output: AgentCommandSessionExecutionOutput) -> Value {
    let output_bytes = output.output.len();
    let output_hash = format!("{:x}", Sha256::digest(output.output.as_bytes()));
    json!({
        "sessionId": output.session_id,
        "status": output.status,
        "output": output.output,
        "exitCode": output.exit_code,
        "latestSequence": output.latest_sequence,
        "outputTruncated": output.output_truncated,
        "read": {
            "requestedAfterSequence": output.requested_after_sequence,
            "firstSequence": output.first_output_sequence,
            "throughSequence": output.last_output_sequence,
            "truncatedBefore": output.truncated_before,
            "outputBytes": output_bytes,
            "outputHash": output_hash
        }
    })
}

fn trace_result(value: &Value) -> Value {
    let Some(source) = value.as_object() else {
        return Value::Object(Map::new());
    };
    let mut projected = Map::new();
    for field in [
        "sessionId",
        "status",
        "exitCode",
        "latestSequence",
        "outputTruncated",
        "read",
    ] {
        if let Some(value) = source.get(field) {
            projected.insert(field.to_string(), value.clone());
        }
    }
    Value::Object(projected)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{
        AgentCommandPermission, AgentPermissions, AgentRunContext, AgentToolCall,
    };
    use crate::runtime::AgentCommandSessionExecutor;
    use std::sync::{Arc, Mutex};

    #[derive(Clone)]
    struct RecordingExecutor {
        requests: Arc<Mutex<Vec<AgentCommandSessionExecutionRequest>>>,
        output: AgentCommandSessionExecutionOutput,
    }

    impl AgentCommandSessionExecutor for RecordingExecutor {
        fn execute_command_session(
            &self,
            request: AgentCommandSessionExecutionRequest,
            _control: AgentCommandSessionExecutionControl,
        ) -> AgentResult<AgentCommandSessionExecutionOutput> {
            self.requests
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .push(request);
            Ok(self.output.clone())
        }
    }

    fn session_id() -> String {
        "cmd_0123456789abcdef0123456789abcdef".to_string()
    }

    fn output(status: AgentCommandSessionStatus, text: &str) -> AgentCommandSessionExecutionOutput {
        AgentCommandSessionExecutionOutput {
            session_id: session_id(),
            status,
            output: text.to_string(),
            exit_code: None,
            requested_after_sequence: 4,
            first_output_sequence: (!text.is_empty()).then_some(5),
            last_output_sequence: (!text.is_empty()).then_some(6),
            latest_sequence: 6,
            truncated_before: false,
            output_truncated: false,
        }
    }

    fn context(executor: Arc<dyn AgentCommandSessionExecutor>) -> ToolExecutionContext {
        ToolExecutionContext::from_run_context(Some(&AgentRunContext {
            workspace: None,
            permissions: AgentPermissions {
                command: AgentCommandPermission::AutoApprove,
                ..AgentPermissions::default()
            },
            conversation_id: Some("conversation-one".to_string()),
            project_id: Some("project-one".to_string()),
            attachment_library: None,
        }))
        .with_runtime_services("run-one".to_string(), None)
        .with_command_session_executor(Some(executor))
    }

    fn execute(context: &ToolExecutionContext, args: Value) -> crate::protocol::AgentToolResult {
        let registry = super::super::ToolRegistry::defaults_with_search(None);
        registry.execute(
            context,
            &AgentToolCall {
                id: "call-one".to_string(),
                tool: "command_session".to_string(),
                args,
                approval_status: crate::protocol::AgentApprovalStatus::NotRequired,
                reason: None,
            },
        )
    }

    #[test]
    fn default_registry_keeps_command_session_authoritative_and_async() {
        let registry = super::super::ToolRegistry::defaults_with_search(None);
        assert_eq!(
            registry.cancellation_settlement("command_session"),
            AgentToolCancellationSettlement::Authoritative
        );
        assert!(registry
            .tools
            .get("command_session")
            .and_then(super::super::AgentToolHandler::async_tool)
            .is_some());
    }

    #[test]
    fn schema_is_narrow_and_defaults_to_host_bounded_wait() {
        let requests = Arc::new(Mutex::new(Vec::new()));
        let context = context(Arc::new(RecordingExecutor {
            requests: Arc::clone(&requests),
            output: output(AgentCommandSessionStatus::Running, "next output"),
        }));

        let definition = CommandSessionTool.definition();
        let properties = definition.input_schema["properties"].as_object().unwrap();
        assert_eq!(properties.len(), 2);
        assert!(properties.contains_key("sessionId"));
        assert!(properties.contains_key("action"));
        assert!(!properties.contains_key("waitMs"));
        assert_eq!(
            definition.input_schema["properties"]["action"]["enum"],
            json!(["wait", "interrupt"])
        );
        assert!(definition.description.contains("action defaults to wait"));
        assert!(definition
            .description
            .contains("ordinary output does not require repeated calls"));

        let result = execute(&context, json!({ "sessionId": session_id() }));

        assert!(result.ok, "{result:?}");
        let request = requests.lock().unwrap().first().unwrap().clone();
        assert_eq!(request.conversation_id, "conversation-one");
        assert_eq!(request.run_id, "run-one");
        assert_eq!(request.call_id, "call-one");
        assert_eq!(request.action, AgentCommandSessionAction::Poll);
        assert_eq!(request.wait_ms, AGENT_COMMAND_SESSION_DEFAULT_WAIT_MS);
        assert_eq!(
            request.max_output_bytes,
            AGENT_COMMAND_SESSION_MODEL_OUTPUT_BYTES
        );
        let value = result.result.unwrap();
        assert_eq!(value["status"], "running");
        assert_eq!(value["output"], "next output");
        assert_eq!(value["read"]["requestedAfterSequence"], 4);
        assert_eq!(value["read"]["throughSequence"], 6);
    }

    #[test]
    fn forwards_explicit_wait_without_model_owned_identity() {
        let requests = Arc::new(Mutex::new(Vec::new()));
        let context = context(Arc::new(RecordingExecutor {
            requests: Arc::clone(&requests),
            output: output(AgentCommandSessionStatus::Running, "next output"),
        }));

        let result = execute(
            &context,
            json!({
                "sessionId": session_id(),
                "action": "wait"
            }),
        );

        assert!(result.ok, "{result:?}");
        let request = requests.lock().unwrap().first().unwrap().clone();
        assert_eq!(request.action, AgentCommandSessionAction::Poll);
        assert_eq!(request.wait_ms, AGENT_COMMAND_SESSION_DEFAULT_WAIT_MS);
    }

    #[test]
    fn forwards_explicit_interrupt_without_model_owned_identity() {
        let requests = Arc::new(Mutex::new(Vec::new()));
        let mut terminal = output(AgentCommandSessionStatus::Interrupted, "");
        terminal.latest_sequence = 4;
        let context = context(Arc::new(RecordingExecutor {
            requests: Arc::clone(&requests),
            output: terminal,
        }));

        let result = execute(
            &context,
            json!({
                "sessionId": session_id(),
                "action": "interrupt",
                "waitMs": 2_500
            }),
        );

        assert!(result.ok, "{result:?}");
        let request = requests.lock().unwrap().first().unwrap().clone();
        assert_eq!(request.action, AgentCommandSessionAction::Interrupt);
        assert_eq!(
            request.wait_ms,
            LEGACY_COMMAND_SESSION_MIN_WAIT_MS.min(AGENT_COMMAND_SESSION_MAX_WAIT_MS)
        );
    }

    #[test]
    fn accepts_legacy_poll_and_clamps_legacy_wait_hint() {
        let requests = Arc::new(Mutex::new(Vec::new()));
        let legacy_context = context(Arc::new(RecordingExecutor {
            requests: Arc::clone(&requests),
            output: output(AgentCommandSessionStatus::Running, ""),
        }));

        let result = execute(
            &legacy_context,
            json!({
                "sessionId": session_id(),
                "action": "poll",
                "waitMs": AGENT_COMMAND_SESSION_MAX_WAIT_MS + 1
            }),
        );

        assert!(result.ok, "{result:?}");
        let request = requests.lock().unwrap().first().unwrap().clone();
        assert_eq!(request.action, AgentCommandSessionAction::Poll);
        assert_eq!(request.wait_ms, AGENT_COMMAND_SESSION_MAX_WAIT_MS);

        let zero_wait_requests = Arc::new(Mutex::new(Vec::new()));
        let zero_wait_context = context(Arc::new(RecordingExecutor {
            requests: Arc::clone(&zero_wait_requests),
            output: output(AgentCommandSessionStatus::Running, ""),
        }));
        let zero_wait_result = execute(
            &zero_wait_context,
            json!({
                "sessionId": session_id(),
                "action": "poll",
                "waitMs": 0
            }),
        );
        assert!(zero_wait_result.ok, "{zero_wait_result:?}");
        assert_eq!(
            zero_wait_requests.lock().unwrap()[0].wait_ms,
            AGENT_COMMAND_SESSION_DEFAULT_WAIT_MS
        );
    }

    #[test]
    fn rejects_invalid_id_and_unknown_fields() {
        let requests = Arc::new(Mutex::new(Vec::new()));
        let context = context(Arc::new(RecordingExecutor {
            requests: Arc::clone(&requests),
            output: output(AgentCommandSessionStatus::Running, ""),
        }));

        for args in [
            json!({ "sessionId": "predictable" }),
            json!({ "sessionId": session_id(), "conversationId": "forged" }),
        ] {
            let result = execute(&context, args);
            assert!(!result.ok, "{result:?}");
        }
        assert!(requests.lock().unwrap().is_empty());
    }

    #[test]
    fn fails_closed_when_the_host_capability_is_absent() {
        let context = ToolExecutionContext::from_run_context(Some(&AgentRunContext {
            workspace: None,
            permissions: AgentPermissions::default(),
            conversation_id: Some("conversation-one".to_string()),
            project_id: None,
            attachment_library: None,
        }))
        .with_runtime_services("run-one".to_string(), None);

        let result = execute(&context, json!({ "sessionId": session_id() }));

        assert!(!result.ok);
        assert_eq!(result.result.unwrap()["code"], "commandSessionUnavailable");
    }

    #[test]
    fn trace_is_metadata_only_and_poll_results_are_not_archived() {
        let context = context(Arc::new(RecordingExecutor {
            requests: Arc::new(Mutex::new(Vec::new())),
            output: output(AgentCommandSessionStatus::Running, "secret output"),
        }));
        let registry = super::super::ToolRegistry::defaults_with_search(None);
        let raw = execute(&context, json!({ "sessionId": session_id() }));

        let trace = registry.trace_projection(&raw);
        let trace_value = trace.result.unwrap();
        assert!(trace_value.get("output").is_none());
        assert_eq!(trace_value["read"]["outputBytes"], 13);
        assert_eq!(
            trace_value["read"]["outputHash"].as_str().unwrap().len(),
            64
        );
        assert!(!registry.archives_result("command_session"));
    }

    #[test]
    fn rejects_inconsistent_host_identity_and_sequence_ranges() {
        let requests = Arc::new(Mutex::new(Vec::new()));
        let mut invalid = output(AgentCommandSessionStatus::Running, "data");
        invalid.session_id = "cmd_ffffffffffffffffffffffffffffffff".to_string();
        let context = context(Arc::new(RecordingExecutor {
            requests,
            output: invalid,
        }));

        let result = execute(&context, json!({ "sessionId": session_id() }));

        assert!(!result.ok);
        assert_eq!(
            result.result.unwrap()["code"],
            "commandSessionInvalidHostOutput"
        );
    }
}
