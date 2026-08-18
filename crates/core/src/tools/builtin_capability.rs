use super::{
    AgentTool, AgentToolExposure, AgentToolPermissionPolicy, AsyncAgentTool, BoxAgentToolFuture,
};
use crate::builtin_capabilities::{
    build_activation_approval, BuiltinCapabilityDispatchRequest, BuiltinCapabilityId,
    BuiltinCapabilityManifest, BuiltinCapabilityRuntime, BuiltinCapabilityToolDescriptor,
    CapabilityActivationState,
};
use crate::protocol::{
    AgentError, AgentProposedAction, AgentResult, AgentToolApprovalMode, AgentToolCall,
    AgentToolDefinition, AgentToolResult, AgentToolSafety,
};
use crate::tools::context::ToolExecutionContext;
use crate::tools::tool_set::ToolCapabilityId;
use serde::Deserialize;
use serde_json::{json, Value};

const ACTIVATE_CAPABILITY_TOOL_NAME: &str = "activate_capability";
const BUILTIN_TOOL_ARGUMENT_MAX_BYTES: usize = 64 * 1024;
const BUILTIN_TOOL_RESULT_MAX_BYTES: usize = 4 * 1024 * 1024;
const BUILTIN_TOOL_MODEL_RESULT_MAX_BYTES: usize = 64 * 1024;
const BUILTIN_TOOL_JSON_MAX_DEPTH: usize = 32;
const BUILTIN_TOOL_JSON_MAX_NODES: usize = 4_096;
const BUILTIN_TOOL_JSON_MAX_OBJECT_FIELDS: usize = 256;
const BUILTIN_TOOL_JSON_MAX_ARRAY_ITEMS: usize = 1_024;
const BUILTIN_TOOL_MODEL_MAX_DEPTH: usize = 16;
const BUILTIN_TOOL_MODEL_MAX_OBJECT_FIELDS: usize = 64;
const BUILTIN_TOOL_MODEL_MAX_ARRAY_ITEMS: usize = 64;
const BUILTIN_TOOL_MODEL_MAX_STRING_CHARS: usize = 16 * 1024;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ActivateCapabilityArgs {
    capability: String,
    reason: String,
}

pub(crate) fn tool_capability_id(
    capability_id: &BuiltinCapabilityId,
) -> AgentResult<ToolCapabilityId> {
    ToolCapabilityId::parse(format!("builtin.{}", capability_id.as_str()))
}

pub(crate) struct ActivateCapabilityTool {
    run_id: String,
    runtime: BuiltinCapabilityRuntime,
}

impl ActivateCapabilityTool {
    pub(crate) fn new(run_id: String, runtime: BuiltinCapabilityRuntime) -> Self {
        Self { run_id, runtime }
    }

    fn parse(&self, args: &Value) -> AgentResult<(BuiltinCapabilityId, String)> {
        let args: ActivateCapabilityArgs = serde_json::from_value(args.clone())
            .map_err(|_| AgentError::new("activate_capability 参数无效。"))?;
        let id = BuiltinCapabilityId::parse(args.capability)?;
        if self.runtime.manifest(&id).is_none() {
            return Err(AgentError::new("请求的内置能力未注册。"));
        }
        if args.reason.trim().is_empty() || args.reason.len() > 4 * 1024 {
            return Err(AgentError::new("激活理由不能为空且不能超过 4 KiB。"));
        }
        Ok((id, args.reason))
    }
}

impl AgentTool for ActivateCapabilityTool {
    fn definition(&self) -> AgentToolDefinition {
        let capabilities = self
            .runtime
            .manifests()
            .iter()
            .map(|manifest| Value::String(manifest.descriptor.id.as_str().to_string()))
            .collect::<Vec<_>>();
        let descriptions = self
            .runtime
            .manifests()
            .iter()
            .map(|manifest| {
                format!(
                    "{} (`{}`): {}",
                    manifest.descriptor.display_name,
                    manifest.descriptor.id.as_str(),
                    manifest.descriptor.description
                )
            })
            .collect::<Vec<_>>()
            .join("\n");
        AgentToolDefinition {
            name: ACTIVATE_CAPABILITY_TOOL_NAME.to_string(),
            description: format!(
                "Request task-scoped access to a built-in capability. Approval applies only to this task and the reviewed manifest. Available capabilities:\n{descriptions}"
            ),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "capability": {"type": "string", "enum": capabilities},
                    "reason": {"type": "string", "minLength": 1, "maxLength": 4096}
                },
                "required": ["capability", "reason"],
                "additionalProperties": false
            }),
            safety: AgentToolSafety::RequiresApproval,
            requires_workspace: false,
            requires_approval: true,
            approval_mode: AgentToolApprovalMode::Always,
        }
    }

    fn execute(&self, _context: &ToolExecutionContext, args: Value) -> AgentResult<Value> {
        let (id, _) = self.parse(&args)?;
        let policy = self.runtime.policy(&id)?;
        if !policy.user_allowed {
            return Ok(json!({
                "status": CapabilityActivationState::DisabledByUser,
                "capability": id,
            }));
        }
        if self.runtime.live_grant(&self.run_id, &id)?.is_some() {
            return Ok(json!({
                "status": CapabilityActivationState::Active,
                "capability": id,
                "alreadyActive": true,
            }));
        }
        Err(AgentError::new("内置能力尚未获得本任务批准。"))
    }

    fn permission_policy(&self) -> AgentToolPermissionPolicy {
        AgentToolPermissionPolicy::Default
    }

    fn exposure(&self) -> AgentToolExposure {
        AgentToolExposure::Stable
    }

    fn proposed_action(
        &self,
        context: &ToolExecutionContext,
        call: &AgentToolCall,
    ) -> AgentResult<AgentProposedAction> {
        let (id, reason) = self.parse(&call.args)?;
        if context.run_id()? != self.run_id {
            return Err(AgentError::new("内置能力激活请求的 run identity 不匹配。"));
        }
        let manifest = self
            .runtime
            .manifest(&id)
            .ok_or_else(|| AgentError::new("请求的内置能力未注册。"))?;
        let policy = self.runtime.policy(&id)?;
        if !policy.user_allowed {
            return Err(AgentError::new("用户已在设置中关闭该内置能力。"));
        }
        if self.runtime.live_grant(&self.run_id, &id)?.is_some() {
            return Err(AgentError::new("该内置能力已在当前任务激活。"));
        }
        Ok(AgentProposedAction::BuiltinCapabilityActivation {
            approval: Box::new(build_activation_approval(
                &self.run_id,
                &call.id,
                manifest,
                &policy,
                reason,
            )?),
        })
    }

    fn requires_approval_for_call(&self, args: &Value) -> bool {
        let Ok((id, _)) = self.parse(args) else {
            return false;
        };
        let Ok(policy) = self.runtime.policy(&id) else {
            return false;
        };
        policy.user_allowed
            && self
                .runtime
                .live_grant(&self.run_id, &id)
                .map(|grant| grant.is_none())
                .unwrap_or(false)
    }
}

pub(crate) struct BuiltinCapabilityAgentTool {
    capability_id: BuiltinCapabilityId,
    capability_gate: ToolCapabilityId,
    managed_mcp_id: String,
    manifest_digest: String,
    descriptor: BuiltinCapabilityToolDescriptor,
    runtime: BuiltinCapabilityRuntime,
}

impl BuiltinCapabilityAgentTool {
    pub(crate) fn new(
        manifest: &BuiltinCapabilityManifest,
        descriptor: BuiltinCapabilityToolDescriptor,
        runtime: BuiltinCapabilityRuntime,
    ) -> AgentResult<Self> {
        let capability_gate = tool_capability_id(&manifest.descriptor.id)?;
        Ok(Self {
            capability_id: manifest.descriptor.id.clone(),
            capability_gate,
            managed_mcp_id: manifest.managed_mcp_id.clone(),
            manifest_digest: manifest.manifest_digest.clone(),
            descriptor,
            runtime,
        })
    }

    pub(crate) fn capability_id(&self) -> &BuiltinCapabilityId {
        &self.capability_id
    }

    pub(crate) fn manifest_digest(&self) -> &str {
        &self.manifest_digest
    }

    pub(crate) fn managed_mcp_id(&self) -> &str {
        &self.managed_mcp_id
    }

    pub(crate) fn tool_id(&self) -> &str {
        &self.descriptor.tool_id
    }

    pub(crate) fn model_name(&self) -> &str {
        &self.descriptor.model_name
    }
}

impl AgentTool for BuiltinCapabilityAgentTool {
    fn definition(&self) -> AgentToolDefinition {
        self.descriptor.definition()
    }

    fn execute(&self, _context: &ToolExecutionContext, _args: Value) -> AgentResult<Value> {
        Err(AgentError::new(
            "内置能力工具必须通过异步 capability invoker 执行。",
        ))
    }

    fn permission_policy(&self) -> AgentToolPermissionPolicy {
        AgentToolPermissionPolicy::Default
    }

    fn exposure(&self) -> AgentToolExposure {
        AgentToolExposure::RequiresCapability(self.capability_gate.clone())
    }

    fn archives_result(&self) -> bool {
        false
    }

    fn trace_call_projection(&self, call: &AgentToolCall) -> AgentToolCall {
        builtin_capability_private_call_projection(call)
    }

    fn event_call_projection(&self, call: &AgentToolCall) -> AgentToolCall {
        builtin_capability_private_call_projection(call)
    }

    fn checkpoint_call_projection(&self, call: &AgentToolCall) -> AgentToolCall {
        builtin_capability_private_call_projection(call)
    }

    fn trace_projection(&self, result: &AgentToolResult) -> AgentToolResult {
        builtin_capability_persistence_projection(result)
    }

    fn archive_projection(&self, result: &AgentToolResult) -> AgentToolResult {
        builtin_capability_persistence_projection(result)
    }

    fn event_projection(&self, result: &AgentToolResult) -> AgentToolResult {
        builtin_capability_persistence_projection(result)
    }

    fn checkpoint_projection(&self, result: &AgentToolResult) -> AgentToolResult {
        builtin_capability_persistence_projection(result)
    }
}

impl AsyncAgentTool for BuiltinCapabilityAgentTool {
    fn execute_async<'a>(
        &'a self,
        context: &'a ToolExecutionContext,
        args: Value,
    ) -> BoxAgentToolFuture<'a> {
        Box::pin(async move {
            validate_builtin_tool_arguments(&args)?;
            let run_id = context.run_id()?;
            let call_id = context.tool_call_id()?;
            let value = self
                .runtime
                .invoke(BuiltinCapabilityDispatchRequest {
                    run_id: run_id.to_string(),
                    capability_id: self.capability_id.clone(),
                    managed_mcp_id: self.managed_mcp_id.clone(),
                    manifest_digest: self.manifest_digest.clone(),
                    tool_id: self.descriptor.tool_id.clone(),
                    model_name: self.descriptor.model_name.clone(),
                    call_id: call_id.to_string(),
                    arguments: args,
                    cancellation: context.cancellation_token(),
                })
                .await?;
            project_builtin_tool_model_result(&value).map(Into::into)
        })
    }
}

fn validate_builtin_tool_arguments(arguments: &Value) -> AgentResult<()> {
    if !arguments.is_object() {
        return Err(AgentError::new("内置能力工具参数必须是 JSON object。"));
    }
    let bytes =
        serde_json::to_vec(arguments).map_err(|_| AgentError::new("无法验证内置能力工具参数。"))?;
    if bytes.len() > BUILTIN_TOOL_ARGUMENT_MAX_BYTES {
        return Err(AgentError::new("内置能力工具参数超过 64 KiB 上限。"));
    }
    let mut nodes = 0_usize;
    validate_builtin_json_shape(arguments, 0, &mut nodes)
}

fn validate_builtin_json_shape(value: &Value, depth: usize, nodes: &mut usize) -> AgentResult<()> {
    if depth > BUILTIN_TOOL_JSON_MAX_DEPTH {
        return Err(AgentError::new("内置能力工具参数 JSON 层级过深。"));
    }
    *nodes = nodes
        .checked_add(1)
        .ok_or_else(|| AgentError::new("内置能力工具参数节点计数溢出。"))?;
    if *nodes > BUILTIN_TOOL_JSON_MAX_NODES {
        return Err(AgentError::new("内置能力工具参数 JSON 节点过多。"));
    }
    match value {
        Value::Object(object) => {
            if object.len() > BUILTIN_TOOL_JSON_MAX_OBJECT_FIELDS {
                return Err(AgentError::new("内置能力工具参数 object 字段过多。"));
            }
            for child in object.values() {
                validate_builtin_json_shape(child, depth + 1, nodes)?;
            }
        }
        Value::Array(array) => {
            if array.len() > BUILTIN_TOOL_JSON_MAX_ARRAY_ITEMS {
                return Err(AgentError::new("内置能力工具参数 array 项过多。"));
            }
            for child in array {
                validate_builtin_json_shape(child, depth + 1, nodes)?;
            }
        }
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => {}
    }
    Ok(())
}

fn project_builtin_tool_model_result(value: &Value) -> AgentResult<Value> {
    let raw_bytes =
        serde_json::to_vec(value).map_err(|_| AgentError::new("无法验证内置能力工具结果。"))?;
    if raw_bytes.len() > BUILTIN_TOOL_RESULT_MAX_BYTES {
        return Err(AgentError::new("内置能力工具结果超过 4 MiB 上限。"));
    }
    let (projected, truncated) = project_builtin_runtime_value(value, 0);
    let projected_bytes = serde_json::to_vec(&projected)
        .map_err(|_| AgentError::new("无法投影内置能力工具结果。"))?;
    if projected_bytes.len() <= BUILTIN_TOOL_MODEL_RESULT_MAX_BYTES && !truncated {
        return Ok(projected);
    }
    if projected_bytes.len() <= BUILTIN_TOOL_MODEL_RESULT_MAX_BYTES {
        return Ok(json!({
            "content": projected,
            "truncated": true,
            "message": "Builtin capability output was bounded before entering model context."
        }));
    }
    Ok(json!({
        "truncated": true,
        "originalBytes": raw_bytes.len(),
        "message": "Builtin capability output exceeded the 64 KiB model projection budget."
    }))
}

fn project_builtin_runtime_value(value: &Value, depth: usize) -> (Value, bool) {
    if depth > BUILTIN_TOOL_MODEL_MAX_DEPTH {
        return (json!("[nested content omitted]"), true);
    }
    match value {
        Value::Object(object) => {
            let mut truncated = object.len() > BUILTIN_TOOL_MODEL_MAX_OBJECT_FIELDS;
            let mut projected = serde_json::Map::new();
            for (key, child) in object.iter().take(BUILTIN_TOOL_MODEL_MAX_OBJECT_FIELDS) {
                let (child, child_truncated) = project_builtin_runtime_value(child, depth + 1);
                projected.insert(key.clone(), child);
                truncated |= child_truncated;
            }
            (Value::Object(projected), truncated)
        }
        Value::Array(array) => {
            let mut truncated = array.len() > BUILTIN_TOOL_MODEL_MAX_ARRAY_ITEMS;
            let projected = array
                .iter()
                .take(BUILTIN_TOOL_MODEL_MAX_ARRAY_ITEMS)
                .map(|child| {
                    let (child, child_truncated) = project_builtin_runtime_value(child, depth + 1);
                    truncated |= child_truncated;
                    child
                })
                .collect();
            (Value::Array(projected), truncated)
        }
        Value::String(text) => {
            let (sanitized, binary_omitted) =
                crate::conversation_trace_projection::sanitize_runtime_text(text);
            let char_count = sanitized.chars().count();
            if char_count <= BUILTIN_TOOL_MODEL_MAX_STRING_CHARS {
                return (Value::String(sanitized), binary_omitted);
            }
            let bounded = sanitized
                .chars()
                .take(BUILTIN_TOOL_MODEL_MAX_STRING_CHARS)
                .collect::<String>();
            (Value::String(format!("{bounded}\n...[truncated]")), true)
        }
        Value::Null | Value::Bool(_) | Value::Number(_) => (value.clone(), false),
    }
}

fn builtin_capability_private_call_projection(call: &AgentToolCall) -> AgentToolCall {
    let mut projected = call.clone();
    projected.args = json!({});
    projected.reason = None;
    projected
}

fn builtin_capability_persistence_projection(result: &AgentToolResult) -> AgentToolResult {
    AgentToolResult {
        exact_archive_file: None,
        call_id: result.call_id.clone(),
        tool: result.tool.clone(),
        ok: result.ok,
        result: Some(json!({
            "schemaVersion": 1,
            "type": "builtin_capability_tool",
            "status": if result.ok { "completed" } else { "failed" },
            "contentOmitted": true,
        })),
        error: result.error.as_ref().map(|_| {
            "The built-in capability tool failed; private diagnostics were omitted.".to_string()
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::builtin_capabilities::{
        BuiltinCapabilityDescriptor, BuiltinCapabilityFuture, BuiltinCapabilityInvocation,
        BuiltinCapabilityPolicy, BuiltinCapabilityProvider, CapabilityActivationId,
        CapabilityGrant,
    };
    use crate::protocol::{AgentApprovalStatus, AgentToolIdentity};
    use crate::tools::ToolRegistry;
    use crate::AgentCancellationToken;
    use std::collections::BTreeSet;
    use std::sync::{Arc, Mutex};
    use uuid::Uuid;

    #[derive(Clone)]
    struct TestProvider {
        manifest: BuiltinCapabilityManifest,
        policy: Arc<Mutex<BuiltinCapabilityPolicy>>,
        grant: Arc<Mutex<Option<CapabilityGrant>>>,
        invocations: Arc<Mutex<Vec<BuiltinCapabilityInvocation>>>,
        result: Arc<Mutex<Value>>,
    }

    impl BuiltinCapabilityProvider for TestProvider {
        fn manifests(&self) -> AgentResult<Vec<BuiltinCapabilityManifest>> {
            Ok(vec![self.manifest.clone()])
        }

        fn policy(&self, _: &BuiltinCapabilityId) -> AgentResult<BuiltinCapabilityPolicy> {
            Ok(self.policy.lock().unwrap().clone())
        }

        fn grant(&self, _: &str, _: &BuiltinCapabilityId) -> AgentResult<Option<CapabilityGrant>> {
            Ok(self.grant.lock().unwrap().clone())
        }

        fn approve_activation(
            &self,
            approval: &crate::AgentBuiltinCapabilityActivationApproval,
        ) -> AgentResult<CapabilityGrant> {
            self.grant
                .lock()
                .unwrap()
                .clone()
                .ok_or_else(|| AgentError::new(format!("no grant for {}", approval.action_id)))
        }

        fn revoke_grants(&self, _: &BuiltinCapabilityId) -> AgentResult<()> {
            *self.grant.lock().unwrap() = None;
            Ok(())
        }

        fn revoke_activation(&self, _: &CapabilityActivationId, _: &str) -> AgentResult<()> {
            *self.grant.lock().unwrap() = None;
            Ok(())
        }

        fn invoke_authorized<'a>(
            &'a self,
            invocation: BuiltinCapabilityInvocation,
            expected_grant: CapabilityGrant,
            _: AgentCancellationToken,
        ) -> BuiltinCapabilityFuture<'a, Value> {
            Box::pin(async move {
                assert_eq!(invocation.activation_id, expected_grant.activation_id);
                self.invocations.lock().unwrap().push(invocation);
                Ok(self.result.lock().unwrap().clone())
            })
        }
    }

    fn harness(allowed: bool) -> (BuiltinCapabilityRuntime, TestProvider) {
        let manifest = BuiltinCapabilityManifest::new(
            BuiltinCapabilityDescriptor {
                id: BuiltinCapabilityId::parse("browser.automation").unwrap(),
                display_name: "Browser automation".to_string(),
                description: "Control the managed browser".to_string(),
            },
            "builtin.browser_automation.mcp",
            "1",
            vec![BuiltinCapabilityToolDescriptor::new(
                "browser.snapshot",
                "browser_snapshot",
                "Read the current page",
                json!({
                    "type": "object",
                    "properties": {},
                    "additionalProperties": false
                }),
                AgentToolSafety::ReadOnly,
                false,
            )
            .unwrap()],
        )
        .unwrap();
        let provider = TestProvider {
            manifest,
            policy: Arc::new(Mutex::new(BuiltinCapabilityPolicy {
                user_allowed: allowed,
                revision: 7,
            })),
            grant: Arc::new(Mutex::new(None)),
            invocations: Arc::new(Mutex::new(Vec::new())),
            result: Arc::new(Mutex::new(json!({"ok": true}))),
        };
        (
            BuiltinCapabilityRuntime::new(Arc::new(provider.clone())).unwrap(),
            provider,
        )
    }

    fn context() -> ToolExecutionContext {
        context_for("run-1")
    }

    fn context_for(run_id: &str) -> ToolExecutionContext {
        ToolExecutionContext::from_run_context(None)
            .with_runtime_services(run_id.to_string(), None)
            .with_tool_call_id("call-1".to_string())
    }

    fn activation_call() -> AgentToolCall {
        AgentToolCall {
            id: "call-1".to_string(),
            tool: ACTIVATE_CAPABILITY_TOOL_NAME.to_string(),
            args: json!({
                "capability": "browser.automation",
                "reason": "Open the task page"
            }),
            approval_status: AgentApprovalStatus::Required,
            reason: None,
        }
    }

    #[test]
    fn activation_proposes_exact_typed_approval() {
        let (runtime, _) = harness(true);
        let tool = ActivateCapabilityTool::new("run-1".to_string(), runtime);
        assert!(tool.requires_approval_for_call(&activation_call().args));
        let action = tool
            .proposed_action(&context(), &activation_call())
            .unwrap();
        let AgentProposedAction::BuiltinCapabilityActivation { approval } = action else {
            panic!("expected typed built-in capability activation");
        };
        assert_eq!(approval.run_id, "run-1");
        assert_eq!(approval.call_id, "call-1");
        assert_eq!(approval.capability_id, "browser.automation");
        assert_eq!(approval.policy_revision, 7);
        assert_eq!(approval.reason, "Open the task page");
        assert_eq!(approval.approval_status, AgentApprovalStatus::Required);
        assert!(Uuid::parse_str(&approval.action_id).is_ok());
        assert!(Uuid::parse_str(&approval.activation_id).is_ok());
    }

    #[test]
    fn approved_activation_creates_only_an_exact_host_grant() {
        let (runtime, provider) = harness(true);
        let tool = ActivateCapabilityTool::new("run-1".to_string(), runtime.clone());
        let action = tool
            .proposed_action(&context(), &activation_call())
            .unwrap();
        let AgentProposedAction::BuiltinCapabilityActivation { mut approval } = action else {
            panic!("expected typed built-in capability activation");
        };
        approval.approval_status = AgentApprovalStatus::Approved;
        let manifest = runtime.manifests()[0].clone();
        let grant = CapabilityGrant {
            run_id: approval.run_id.clone(),
            capability_id: manifest.descriptor.id,
            activation_id: CapabilityActivationId::parse(approval.activation_id.clone()).unwrap(),
            manifest_digest: approval.manifest_digest.clone(),
            policy_revision: approval.policy_revision,
            created_at: approval.created_at,
            expires_at: approval.created_at
                + crate::builtin_capabilities::BUILTIN_CAPABILITY_GRANT_TTL_SECONDS,
        };
        *provider.grant.lock().unwrap() = Some(grant.clone());
        assert_eq!(runtime.approve_activation(&approval).unwrap(), grant);

        approval.manifest_digest = "sha256:deadbeef".to_string();
        assert!(runtime.approve_activation(&approval).is_err());
    }

    #[test]
    fn live_grant_skips_same_run_approval_but_a_new_run_still_requires_approval() {
        let (runtime, provider) = harness(true);
        let manifest = runtime.manifests()[0].clone();
        let now = crate::builtin_capabilities::unix_timestamp();
        *provider.grant.lock().unwrap() = Some(CapabilityGrant {
            run_id: "run-1".to_string(),
            capability_id: manifest.descriptor.id,
            activation_id: CapabilityActivationId::generate(),
            manifest_digest: manifest.manifest_digest,
            policy_revision: 7,
            created_at: now,
            expires_at: now + 60,
        });

        let same_run = ActivateCapabilityTool::new("run-1".to_string(), runtime.clone());
        assert!(!same_run.requires_approval_for_call(&activation_call().args));
        let result = same_run
            .execute(&context(), activation_call().args)
            .unwrap();
        assert_eq!(result["status"], json!(CapabilityActivationState::Active));
        assert_eq!(result["alreadyActive"], true);

        let new_run = ActivateCapabilityTool::new("run-2".to_string(), runtime);
        assert!(new_run.requires_approval_for_call(&activation_call().args));
        let proposal = new_run
            .proposed_action(&context_for("run-2"), &activation_call())
            .unwrap();
        let AgentProposedAction::BuiltinCapabilityActivation { approval } = proposal else {
            panic!("new run must receive a fresh typed activation approval");
        };
        assert_eq!(approval.run_id, "run-2");
    }

    #[test]
    fn disabled_capability_returns_normal_result_without_approval() {
        let (runtime, _) = harness(false);
        let tool = ActivateCapabilityTool::new("run-1".to_string(), runtime);
        assert!(!tool.requires_approval_for_call(&activation_call().args));
        let result = tool.execute(&context(), activation_call().args).unwrap();
        assert_eq!(result["status"], "disabled_by_user");
    }

    #[test]
    fn capability_gate_prefix_overflow_is_a_structured_error_not_a_panic() {
        let longest_core_id = BuiltinCapabilityId::parse("a".repeat(128)).unwrap();
        let error = tool_capability_id(&longest_core_id).unwrap_err();
        assert!(error.to_string().contains("工具能力 id"));

        let longest_prefixed_id = BuiltinCapabilityId::parse("a".repeat(120)).unwrap();
        assert!(tool_capability_id(&longest_prefixed_id).is_ok());
    }

    #[test]
    fn manifest_tools_are_typed_and_hidden_until_grant_capability_is_active() {
        let (runtime, _) = harness(true);
        let manifest = &runtime.manifests()[0];
        let mut registry = ToolRegistry::empty();
        registry
            .register_builtin_capability_tool(
                BuiltinCapabilityAgentTool::new(
                    manifest,
                    manifest.tools[0].clone(),
                    runtime.clone(),
                )
                .unwrap(),
            )
            .unwrap();

        let hidden = registry
            .effective_tool_set(registry.definitions(), &Default::default())
            .unwrap();
        assert!(!hidden.contains("browser_snapshot"));
        assert!(matches!(
            hidden.unavailability("browser_snapshot"),
            Some(
                crate::tools::tool_set::ToolUnavailability::RequiresBuiltinCapabilityActivation { .. }
            )
        ));

        let active = BTreeSet::from([tool_capability_id(&manifest.descriptor.id).unwrap()]);
        let exposed = registry
            .effective_tool_set(registry.definitions(), &active)
            .unwrap();
        assert!(exposed.contains("browser_snapshot"));
        assert!(matches!(
            registry.identity("browser_snapshot"),
            Some(AgentToolIdentity::BuiltinCapability {
                capability_id,
                managed_mcp_id,
                tool_id,
                model_name,
                ..
            }) if capability_id == "browser.automation"
                && managed_mcp_id == "builtin.browser_automation.mcp"
                && tool_id == "browser.snapshot"
                && model_name == "browser_snapshot"
        ));
    }

    #[tokio::test]
    async fn dispatch_revalidates_live_grant_and_routes_typed_invocation() {
        let (runtime, provider) = harness(true);
        let manifest = runtime.manifests()[0].clone();
        *provider.grant.lock().unwrap() = Some(CapabilityGrant {
            run_id: "run-1".to_string(),
            capability_id: manifest.descriptor.id.clone(),
            activation_id: CapabilityActivationId::generate(),
            manifest_digest: manifest.manifest_digest.clone(),
            policy_revision: 7,
            created_at: crate::builtin_capabilities::unix_timestamp(),
            expires_at: crate::builtin_capabilities::unix_timestamp() + 60,
        });
        let tool =
            BuiltinCapabilityAgentTool::new(&manifest, manifest.tools[0].clone(), runtime.clone())
                .unwrap();
        let result = tool.execute_async(&context(), json!({})).await.unwrap();
        assert_eq!(result.value, json!({"ok": true}));
        {
            let invocations = provider.invocations.lock().unwrap();
            assert_eq!(invocations.len(), 1);
            assert_eq!(invocations[0].tool_id, "browser.snapshot");
            assert_eq!(invocations[0].model_name, "browser_snapshot");
            assert_eq!(
                invocations[0].managed_mcp_id,
                "builtin.browser_automation.mcp"
            );
            assert_eq!(invocations[0].capability_id.as_str(), "browser.automation");
        }

        provider.policy.lock().unwrap().revision = 8;
        let error = match tool.execute_async(&context(), json!({})).await {
            Ok(_) => panic!("drifted grant must fail closed"),
            Err(error) => error,
        };
        assert!(error.to_string().contains("没有有效"));
        assert_eq!(provider.invocations.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn builtin_tool_enforces_private_projections_and_hard_json_budgets() {
        let (runtime, provider) = harness(true);
        let manifest = runtime.manifests()[0].clone();
        let now = crate::builtin_capabilities::unix_timestamp();
        *provider.grant.lock().unwrap() = Some(CapabilityGrant {
            run_id: "run-1".to_string(),
            capability_id: manifest.descriptor.id.clone(),
            activation_id: CapabilityActivationId::generate(),
            manifest_digest: manifest.manifest_digest.clone(),
            policy_revision: 7,
            created_at: now,
            expires_at: now + 60,
        });
        let tool =
            BuiltinCapabilityAgentTool::new(&manifest, manifest.tools[0].clone(), runtime).unwrap();

        let secret = "BUILTIN_PRIVATE_CANARY";
        let call = AgentToolCall {
            id: "private-call".to_string(),
            tool: "browser_snapshot".to_string(),
            args: json!({"value": secret}),
            approval_status: AgentApprovalStatus::Approved,
            reason: Some(secret.to_string()),
        };
        for projected in [
            tool.trace_call_projection(&call),
            tool.event_call_projection(&call),
            tool.checkpoint_call_projection(&call),
        ] {
            assert_eq!(projected.args, json!({}));
            assert_eq!(projected.reason, None);
            assert!(!serde_json::to_string(&projected).unwrap().contains(secret));
        }
        let raw_result = AgentToolResult {
            exact_archive_file: None,
            call_id: call.id.clone(),
            tool: call.tool.clone(),
            ok: true,
            result: Some(json!({"page": secret})),
            error: None,
        };
        for projected in [
            tool.trace_projection(&raw_result),
            tool.archive_projection(&raw_result),
            tool.event_projection(&raw_result),
            tool.checkpoint_projection(&raw_result),
        ] {
            let serialized = serde_json::to_string(&projected).unwrap();
            assert!(!serialized.contains(secret));
            assert_eq!(projected.result.unwrap()["contentOmitted"], true);
        }

        let oversized_arguments = json!({"payload": "x".repeat(BUILTIN_TOOL_ARGUMENT_MAX_BYTES)});
        let oversized_error = match tool.execute_async(&context(), oversized_arguments).await {
            Ok(_) => panic!("oversized built-in arguments must fail closed"),
            Err(error) => error,
        };
        assert!(oversized_error.to_string().contains("64 KiB"));
        assert!(provider.invocations.lock().unwrap().is_empty());

        *provider.result.lock().unwrap() =
            json!({"payload": "x".repeat(BUILTIN_TOOL_RESULT_MAX_BYTES)});
        let oversized_result_error = match tool.execute_async(&context(), json!({})).await {
            Ok(_) => panic!("oversized built-in result must fail closed"),
            Err(error) => error,
        };
        assert!(oversized_result_error.to_string().contains("4 MiB"));

        *provider.result.lock().unwrap() = json!({
            "payload": "x".repeat(BUILTIN_TOOL_MODEL_RESULT_MAX_BYTES + 1)
        });
        let projected = tool.execute_async(&context(), json!({})).await.unwrap();
        assert_eq!(projected.value["truncated"], true);
        assert!(
            serde_json::to_vec(&projected.value).unwrap().len()
                < BUILTIN_TOOL_MODEL_RESULT_MAX_BYTES
        );
    }
}
