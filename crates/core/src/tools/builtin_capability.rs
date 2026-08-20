use super::{
    AgentTool, AgentToolExposure, AgentToolPermissionPolicy, AsyncAgentTool,
    BoxAgentProposedActionFuture, BoxAgentToolFuture,
};
use crate::builtin_capabilities::{
    build_activation_approval, builtin_tool_requires_approval, BuiltinCapabilityDispatchRequest,
    BuiltinCapabilityId, BuiltinCapabilityInvocation, BuiltinCapabilityManifest,
    BuiltinCapabilityRuntime, BuiltinCapabilityToolDescriptor, BuiltinMcpToolFilePreparation,
    CapabilityActivationState,
};
use crate::protocol::{
    AgentError, AgentProposedAction, AgentResult, AgentToolApprovalMode, AgentToolCall,
    AgentToolDefinition, AgentToolResult, AgentToolSafety, BuiltinMcpToolResourceSummary,
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
const BUILTIN_BROWSER_DIAGNOSTIC_TOKEN_MAX_BYTES: usize = 128;
const BUILTIN_BROWSER_DIAGNOSTIC_MAX_DURATION_MS: u64 = 24 * 60 * 60 * 1_000;

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
    package_name: String,
    package_version: String,
    upstream_catalog_digest: String,
    policy_digest: String,
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
            package_name: manifest.provider_contract.package_name.clone(),
            package_version: manifest.provider_contract.package_version.clone(),
            upstream_catalog_digest: manifest.provider_contract.upstream_catalog_digest.clone(),
            policy_digest: manifest.provider_contract.policy_digest.clone(),
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

    pub(crate) fn package_name(&self) -> &str {
        &self.package_name
    }

    pub(crate) fn package_version(&self) -> &str {
        &self.package_version
    }

    pub(crate) fn upstream_catalog_digest(&self) -> &str {
        &self.upstream_catalog_digest
    }

    pub(crate) fn policy_digest(&self) -> &str {
        &self.policy_digest
    }

    pub(crate) fn tool_id(&self) -> &str {
        &self.descriptor.tool_id
    }

    pub(crate) fn model_name(&self) -> &str {
        &self.descriptor.model_name
    }

    pub(crate) fn raw_name(&self) -> &str {
        &self.descriptor.raw_name
    }

    pub(crate) fn upstream_schema_digest(&self) -> &str {
        &self.descriptor.upstream_schema_digest
    }

    pub(crate) fn host_overlay_digest(&self) -> &str {
        &self.descriptor.host_overlay_digest
    }

    pub(crate) fn host_input_schema_digest(&self) -> &str {
        &self.descriptor.schema_digest
    }

    fn frozen_invocation(
        &self,
        context: &ToolExecutionContext,
        arguments: Value,
    ) -> AgentResult<BuiltinCapabilityInvocation> {
        let grant = self
            .runtime
            .live_grant(context.run_id()?, &self.capability_id)?
            .ok_or_else(|| AgentError::new("当前任务没有有效的内置能力授权。"))?;
        Ok(BuiltinCapabilityInvocation {
            run_id: context.run_id()?.to_string(),
            capability_id: self.capability_id.clone(),
            managed_mcp_id: self.managed_mcp_id.clone(),
            package_name: self.package_name.clone(),
            package_version: self.package_version.clone(),
            upstream_catalog_digest: self.upstream_catalog_digest.clone(),
            policy_digest: self.policy_digest.clone(),
            activation_id: grant.activation_id,
            manifest_digest: self.manifest_digest.clone(),
            policy_revision: grant.policy_revision,
            tool_id: self.descriptor.tool_id.clone(),
            raw_name: self.descriptor.raw_name.clone(),
            model_name: self.descriptor.model_name.clone(),
            upstream_schema_digest: self.descriptor.upstream_schema_digest.clone(),
            host_overlay_digest: self.descriptor.host_overlay_digest.clone(),
            host_input_schema_digest: self.descriptor.schema_digest.clone(),
            call_id: context.tool_call_id()?.to_string(),
            arguments,
            builtin_tool_grant: None,
        })
    }

    fn sensitive_scope(&self) -> (&'static str, &'static str) {
        sensitive_tool_scope(&self.descriptor.tool_id)
    }
}

fn sensitive_tool_scope(tool_id: &str) -> (&'static str, &'static str) {
    match tool_id {
        "browser_drop" | "browser_file_upload" => ("file_upload", "managed_surface"),
        "browser_network_request" => ("network_sensitive_read", "managed_surface"),
        // Playwright cookie APIs and storage-state operate on the managed browser context. Main
        // freezes profile authority without requiring a visible page or model-provided origin.
        "browser_cookie_get" | "browser_cookie_list" => ("cookie_read", "managed_browser_profile"),
        "browser_cookie_clear" | "browser_cookie_delete" | "browser_cookie_set" => {
            ("cookie_write", "managed_browser_profile")
        }
        "browser_localstorage_get" | "browser_localstorage_list" => {
            ("local_storage_read", "local_storage_read")
        }
        "browser_localstorage_clear"
        | "browser_localstorage_delete"
        | "browser_localstorage_set" => ("local_storage_write", "local_storage_write"),
        "browser_sessionstorage_get" | "browser_sessionstorage_list" => {
            ("session_storage_read", "session_storage_read")
        }
        "browser_sessionstorage_clear"
        | "browser_sessionstorage_delete"
        | "browser_sessionstorage_set" => ("session_storage_write", "session_storage_write"),
        "browser_set_storage_state" => ("storage_state_import", "managed_browser_profile"),
        "browser_storage_state" => ("storage_state_export", "managed_browser_profile"),
        "browser_evaluate" => ("page_script_execution", "managed_surface"),
        _ => ("sensitive_browser_operation", "sensitive_browser_operation"),
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
        let mut projected = builtin_capability_private_call_projection(call);
        projected.reason = Some(
            if self.descriptor.builtin_approval_mode
                != crate::builtin_capabilities::BuiltinMcpToolApprovalMode::Never
            {
                let (operation_category, _) = self.sensitive_scope();
                value_free_sensitive_approval_reason(operation_category).to_string()
            } else {
                // Even a harmless browser Tool's model-authored reason may repeat a password or form
                // value without a recognizable label. Typed identity already drives the localized
                // activity text, so persist only this Host-owned value-free explanation.
                "The model requests this reviewed managed-browser operation.".to_string()
            },
        );
        projected
    }

    fn checkpoint_call_projection(&self, call: &AgentToolCall) -> AgentToolCall {
        builtin_capability_private_call_projection(call)
    }

    fn trace_projection(&self, result: &AgentToolResult) -> AgentToolResult {
        builtin_capability_tool_result_persistence_projection(result)
    }

    fn archive_projection(&self, result: &AgentToolResult) -> AgentToolResult {
        builtin_capability_tool_result_persistence_projection(result)
    }

    fn event_projection(&self, result: &AgentToolResult) -> AgentToolResult {
        builtin_capability_tool_result_persistence_projection(result)
    }

    fn checkpoint_projection(&self, result: &AgentToolResult) -> AgentToolResult {
        builtin_capability_tool_result_persistence_projection(result)
    }

    fn proposed_action(
        &self,
        _context: &ToolExecutionContext,
        _call: &AgentToolCall,
    ) -> AgentResult<AgentProposedAction> {
        Err(AgentError::new(
            "敏感内置 MCP Tool 必须通过异步 Host 页面绑定路径创建审批。",
        ))
    }

    fn proposed_action_async<'a>(
        &'a self,
        context: &'a ToolExecutionContext,
        call: &'a AgentToolCall,
    ) -> BoxAgentProposedActionFuture<'a> {
        Box::pin(async move {
            validate_builtin_tool_arguments(&call.args)?;
            if !builtin_tool_requires_approval(&self.descriptor, &call.args) {
                return Err(AgentError::new("该内置 MCP Tool 调用不需要敏感审批。"));
            }
            let (operation_category, resource_scope) = self.sensitive_scope();
            let call_reason = value_free_sensitive_approval_reason(operation_category).to_string();
            let resource_summary = BuiltinMcpToolResourceSummary {
                scope: resource_scope.to_string(),
                display_name: match resource_scope {
                    "managed_browser_profile" => "Managed browser profile".to_string(),
                    "managed_surface" => "Current managed browser surface".to_string(),
                    _ => operation_category.replace('_', " "),
                },
                file_basenames: Vec::new(),
                origin: None,
            };
            let file_preparation =
                sensitive_file_preparation(context, &self.descriptor.tool_id, &call.args)?;
            let invocation = self.frozen_invocation(context, call.args.clone())?;
            let approval = self
                .runtime
                .prepare_builtin_mcp_tool_approval_async(
                    invocation,
                    call_reason,
                    operation_category.to_string(),
                    resource_summary,
                    file_preparation,
                    self.descriptor.builtin_risk_kinds.clone(),
                    context.cancellation_token(),
                )
                .await?;
            Ok(AgentProposedAction::BuiltinMcpToolApproval {
                approval: Box::new(approval),
            })
        })
    }

    fn invalidate_proposed_action(&self, action: &AgentProposedAction) -> AgentResult<()> {
        let AgentProposedAction::BuiltinMcpToolApproval { approval } = action else {
            return Ok(());
        };
        self.runtime.dismiss_builtin_mcp_tool_approval(approval)
    }

    fn requires_approval_for_call(&self, args: &Value) -> bool {
        builtin_tool_requires_approval(&self.descriptor, args)
    }
}

fn sensitive_file_preparation(
    context: &ToolExecutionContext,
    tool_id: &str,
    arguments: &Value,
) -> AgentResult<Option<BuiltinMcpToolFilePreparation>> {
    let resolve_paths = |values: &[Value]| -> AgentResult<Vec<String>> {
        if values.is_empty() || values.len() > 16 {
            return Err(AgentError::new("浏览器文件预检数量无效。"));
        }
        values
            .iter()
            .map(|value| {
                let model_path = value
                    .as_str()
                    .ok_or_else(|| AgentError::new("浏览器文件路径必须是字符串。"))?;
                // Models commonly repeat the absolute path returned by a workspace search. That
                // must not require global filesystem permission when the canonical file is still
                // inside the already-authorized workspace. Canonical containment also rejects an
                // in-workspace symlink that escapes the workspace. Absolute paths elsewhere keep
                // using the normal All/attachment authority checks below.
                let resolved = if std::path::Path::new(model_path.trim()).is_absolute() {
                    let canonical = std::path::Path::new(model_path.trim())
                        .canonicalize()
                        .map_err(|_| AgentError::new("browser.file.invalid_file"))?;
                    if context
                        .workspace_root_optional()?
                        .is_some_and(|root| canonical.starts_with(root))
                    {
                        canonical
                    } else {
                        context
                            .resolve_existing_path(model_path)
                            .map_err(|_| AgentError::new("browser.file.invalid_file"))?
                    }
                } else {
                    context
                        .resolve_existing_path(model_path)
                        .map_err(|_| AgentError::new("browser.file.invalid_file"))?
                };
                if !resolved.is_file() {
                    return Err(AgentError::new("browser.file.invalid_file"));
                }
                resolved
                    .to_str()
                    .map(ToString::to_string)
                    .ok_or_else(|| AgentError::new("browser.file.invalid_file"))
            })
            .collect()
    };

    match tool_id {
        "browser_file_upload" => match arguments.get("paths") {
            Some(Value::Array(paths)) if !paths.is_empty() => Ok(Some(
                BuiltinMcpToolFilePreparation::ResolvedPaths(resolve_paths(paths)?),
            )),
            Some(Value::Array(_)) | None => Ok(None),
            Some(_) => Err(AgentError::new("浏览器文件路径列表无效。")),
        },
        "browser_drop" => match arguments.get("paths") {
            Some(Value::Array(paths)) if !paths.is_empty() => Ok(Some(
                BuiltinMcpToolFilePreparation::ResolvedPaths(resolve_paths(paths)?),
            )),
            _ => Ok(None),
        },
        "browser_set_storage_state" => {
            let filename = arguments
                .get("filename")
                .and_then(Value::as_str)
                .ok_or_else(|| AgentError::new("storage-state 文件路径无效。"))?;
            Ok(Some(BuiltinMcpToolFilePreparation::ResolvedPaths(
                resolve_paths(&[Value::String(filename.to_string())])?,
            )))
        }
        _ => Ok(None),
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
                    package_name: self.package_name.clone(),
                    package_version: self.package_version.clone(),
                    upstream_catalog_digest: self.upstream_catalog_digest.clone(),
                    policy_digest: self.policy_digest.clone(),
                    manifest_digest: self.manifest_digest.clone(),
                    tool_id: self.descriptor.tool_id.clone(),
                    raw_name: self.descriptor.raw_name.clone(),
                    model_name: self.descriptor.model_name.clone(),
                    upstream_schema_digest: self.descriptor.upstream_schema_digest.clone(),
                    host_overlay_digest: self.descriptor.host_overlay_digest.clone(),
                    host_input_schema_digest: self.descriptor.schema_digest.clone(),
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

fn value_free_sensitive_approval_reason(operation_category: &str) -> &'static str {
    match operation_category {
        "file_upload" => "The model requests use of user-selected files in the managed page.",
        "network_sensitive_read" => {
            "The model requests sensitive network details from the managed page."
        }
        "cookie_read" => "The model requests reading browser cookies for the managed page.",
        "cookie_write" => "The model requests changing browser cookies for the managed page.",
        "local_storage_read" => "The model requests reading local storage for the managed page.",
        "local_storage_write" => "The model requests changing local storage for the managed page.",
        "session_storage_read" => {
            "The model requests reading session storage for the managed page."
        }
        "session_storage_write" => {
            "The model requests changing session storage for the managed page."
        }
        "storage_state_import" => "The model requests importing reviewed browser storage state.",
        "storage_state_export" => "The model requests exporting protected browser storage state.",
        "page_script_execution" => {
            "The model requests running a reviewed script in the managed page."
        }
        _ => "The model requests a reviewed sensitive managed-browser operation.",
    }
}

fn safe_builtin_browser_diagnostic_token(value: &Value) -> Option<&str> {
    let token = value.as_str()?;
    let valid = !token.is_empty()
        && token.len() <= BUILTIN_BROWSER_DIAGNOSTIC_TOKEN_MAX_BYTES
        && token
            .bytes()
            .next()
            .is_some_and(|byte| byte.is_ascii_lowercase())
        && token.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || b"._-".contains(&byte)
        })
        && !token.ends_with(['.', '_', '-'])
        && !token.contains("..");
    valid.then_some(token)
}

fn safe_builtin_browser_error_code(value: &Value) -> Option<&str> {
    let code = safe_builtin_browser_diagnostic_token(value)?;
    let namespaced = code.starts_with("mcp.")
        || code.starts_with("browser.")
        || code.starts_with("builtin.browser_automation.")
        || code.starts_with("builtin_mcp_tool.");
    let reviewed_unprefixed = matches!(
        code,
        "cancelled"
            | "timeout"
            | "closed"
            | "busy"
            | "surface_unavailable"
            | "surface_capacity_exceeded"
            | "target_closed"
            | "tool_not_reviewed"
            | "invalid_arguments"
            | "catalog_drift"
            | "output_too_large"
            | "protocol_error"
            | "outcome_unknown"
            | "internal_safe_error"
            | "frame_not_found"
            | "stale_frame_ref"
            | "frame_not_editable"
            | "frame_input_delivery_failed"
            | "frame_detached"
    );
    (namespaced || reviewed_unprefixed).then_some(code)
}

fn safe_builtin_browser_dispatch_certainty(value: &Value) -> Option<&str> {
    match value.as_str()? {
        certainty @ ("definitely_not_dispatched" | "possibly_dispatched" | "response_received") => {
            Some(certainty)
        }
        _ => None,
    }
}

fn append_safe_builtin_browser_failure_diagnostics(
    details: &serde_json::Map<String, Value>,
    safe_result: &mut Value,
) {
    if let Some(error_code) = details
        .get("errorCode")
        .and_then(safe_builtin_browser_error_code)
    {
        safe_result["errorCode"] = json!(error_code);
    }
    if let Some(dispatch_certainty) = details
        .get("dispatchCertainty")
        .and_then(safe_builtin_browser_dispatch_certainty)
    {
        safe_result["dispatchCertainty"] = json!(dispatch_certainty);
    }
    if let Some(failure_stage) = details
        .get("failureStage")
        .and_then(safe_builtin_browser_diagnostic_token)
    {
        safe_result["failureStage"] = json!(failure_stage);
    }
    if let Some(duration_ms) = details
        .get("durationMs")
        .and_then(Value::as_u64)
        .filter(|duration_ms| *duration_ms <= BUILTIN_BROWSER_DIAGNOSTIC_MAX_DURATION_MS)
    {
        safe_result["durationMs"] = json!(duration_ms);
    }
}

pub fn builtin_capability_tool_result_persistence_projection(
    result: &AgentToolResult,
) -> AgentToolResult {
    let details = result.result.as_ref().and_then(Value::as_object);
    let outcome_unknown = details.is_some_and(|details| {
        details.get("errorCode").and_then(Value::as_str) == Some("mcp.tool_outcome_unknown")
            || details.get("outcome").and_then(Value::as_str) == Some("outcome_unknown")
            || details.get("code").and_then(Value::as_str) == Some("outcomeUnknown")
    });
    let cancelled = !outcome_unknown
        && details.is_some_and(|details| {
            matches!(
                details.get("errorCode").and_then(Value::as_str),
                Some("mcp.tool_cancelled" | "mcp.tool_cancelled_before_dispatch")
            ) || matches!(
                details.get("code").and_then(Value::as_str),
                Some("mcp.tool_cancelled" | "cancelled")
            ) || details.get("outcome").and_then(Value::as_str) == Some("cancelled")
                || details.get("status").and_then(Value::as_str) == Some("cancelled")
        });
    let rejected = !outcome_unknown
        && !cancelled
        && details.is_some_and(|details| {
            details.get("status").and_then(Value::as_str) == Some("rejected")
                || details.get("decision").and_then(Value::as_str) == Some("rejected")
        });
    let expired = !outcome_unknown
        && !cancelled
        && !rejected
        && details.is_some_and(|details| {
            details.get("status").and_then(Value::as_str) == Some("expired")
                || details.get("errorCode").and_then(Value::as_str)
                    == Some("mcp.tool_approval_expired")
        });
    let payload_unavailable = !outcome_unknown
        && !cancelled
        && !rejected
        && !expired
        && details.is_some_and(|details| {
            details.get("status").and_then(Value::as_str) == Some("payload_unavailable")
                || details.get("errorCode").and_then(Value::as_str)
                    == Some("mcp.approval_payload_unavailable")
        });
    let status = if outcome_unknown {
        "outcome_unknown"
    } else if cancelled {
        "cancelled"
    } else if rejected {
        "rejected"
    } else if expired {
        "expired"
    } else if payload_unavailable {
        "payload_unavailable"
    } else if result.ok {
        "completed"
    } else {
        "failed"
    };
    let mut safe_result = json!({
        "schemaVersion": 1,
        "type": "builtin_capability_tool",
        "status": status,
        "contentOmitted": true,
    });
    // Only Host-owned, bounded, value-free diagnostics cross the durable boundary. In particular,
    // do not inspect nested `structuredContent`: it originates in the managed Tool response and
    // may contain page-controlled values even when a field happens to look diagnostic.
    if result.tool.starts_with("browser_") && matches!(status, "failed" | "outcome_unknown") {
        if let Some(details) = details {
            append_safe_builtin_browser_failure_diagnostics(details, &mut safe_result);
        }
    }
    if outcome_unknown || cancelled || expired || payload_unavailable {
        safe_result["errorCode"] = json!(if outcome_unknown {
            "mcp.tool_outcome_unknown"
        } else if cancelled {
            "mcp.tool_cancelled_before_dispatch"
        } else if expired {
            "mcp.tool_approval_expired"
        } else {
            "mcp.approval_payload_unavailable"
        });
        safe_result["retryable"] = json!(false);
        safe_result["dispatchCertainty"] = json!(if outcome_unknown {
            "possibly_dispatched"
        } else {
            "definitely_not_dispatched"
        });
    }
    if rejected {
        safe_result["errorCode"] = json!("mcp.approval_rejected");
        safe_result["retryable"] = json!(false);
        safe_result["dispatchCertainty"] = json!("definitely_not_dispatched");
    }
    if let Some(artifacts) = result
        .result
        .as_ref()
        .and_then(|value| value.get("structuredContent"))
        .and_then(crate::browser_artifacts::safe_browser_artifact_references)
    {
        safe_result["artifacts"] = Value::Array(artifacts);
    }
    AgentToolResult {
        exact_archive_file: None,
        call_id: result.call_id.clone(),
        tool: result.tool.clone(),
        ok: result.ok,
        result: Some(safe_result),
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
        BuiltinCapabilityPolicy, BuiltinCapabilityProvider, BuiltinMcpToolApprovalMode,
        BuiltinMcpToolApprovalRequest, CapabilityActivationId, CapabilityGrant,
    };
    use crate::protocol::{
        AgentApprovalStatus, AgentRunContext, AgentToolIdentity, AgentWorkspaceContext,
        BuiltinMcpToolRiskKind,
    };
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

        fn prepare_builtin_mcp_tool_approval(
            &self,
            request: BuiltinMcpToolApprovalRequest,
        ) -> AgentResult<crate::AgentBuiltinMcpToolApproval> {
            crate::builtin_capabilities::build_builtin_mcp_tool_approval(
                &request,
                crate::builtin_capabilities::unix_timestamp(),
            )
        }

        fn prepare_builtin_mcp_tool_target_binding<'a>(
            &'a self,
            request: crate::builtin_capabilities::BuiltinMcpToolTargetBindingRequest,
        ) -> BuiltinCapabilityFuture<
            'a,
            crate::builtin_capabilities::PreparedBuiltinMcpToolTargetBinding,
        > {
            Box::pin(async move {
                Ok(
                    crate::builtin_capabilities::PreparedBuiltinMcpToolTargetBinding {
                        binding_id: Uuid::new_v4().to_string(),
                        target_binding_digest: format!("sha256:{}", "7".repeat(64)),
                        origin: Some("https://mail.example.test".to_string()),
                        created_at: request.created_at,
                        expires_at: request.expires_at,
                        file_basenames: Vec::new(),
                        file_revision_digest: None,
                    },
                )
            })
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

    fn sensitive_harness() -> (BuiltinCapabilityRuntime, TestProvider) {
        let descriptor = BuiltinCapabilityToolDescriptor::new(
            "browser_evaluate",
            "browser_evaluate",
            "Run a reviewed function in the managed page",
            json!({
                "type": "object",
                "properties": {
                    "function": {"type": "string"},
                    "element": {"type": "string"},
                    "target": {"type": "string"},
                    "call_reason": {"type": "string"}
                },
                "required": ["function", "call_reason"],
                "additionalProperties": false
            }),
            AgentToolSafety::RequiresApproval,
            false,
        )
        .unwrap()
        .with_builtin_approval_policy(
            BuiltinMcpToolApprovalMode::Always,
            vec![BuiltinMcpToolRiskKind::PageScriptExecution],
        )
        .unwrap();
        let manifest = BuiltinCapabilityManifest::new(
            BuiltinCapabilityDescriptor {
                id: BuiltinCapabilityId::parse("browser.automation").unwrap(),
                display_name: "Browser automation".to_string(),
                description: "Control the managed browser".to_string(),
            },
            "builtin.browser_automation.mcp",
            "1",
            vec![descriptor],
        )
        .unwrap();
        let now = crate::builtin_capabilities::unix_timestamp();
        let grant = CapabilityGrant {
            run_id: "run-1".to_string(),
            capability_id: manifest.descriptor.id.clone(),
            activation_id: CapabilityActivationId::generate(),
            manifest_digest: manifest.manifest_digest.clone(),
            upstream_catalog_digest: manifest.provider_contract.upstream_catalog_digest.clone(),
            provider_policy_digest: manifest.provider_contract.policy_digest.clone(),
            policy_revision: 7,
            created_at: now,
            expires_at: now + crate::builtin_capabilities::BUILTIN_CAPABILITY_GRANT_TTL_SECONDS,
        };
        let provider = TestProvider {
            manifest,
            policy: Arc::new(Mutex::new(BuiltinCapabilityPolicy {
                user_allowed: true,
                revision: 7,
            })),
            grant: Arc::new(Mutex::new(Some(grant))),
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

    fn workspace_context(root: &std::path::Path) -> ToolExecutionContext {
        ToolExecutionContext::from_run_context(Some(&AgentRunContext {
            collaboration_identity: None,
            conversation_id: None,
            project_id: None,
            workspace: Some(AgentWorkspaceContext {
                project_id: None,
                display_name: Some("browser file preflight fixture".to_string()),
                root_path: Some(root.to_string_lossy().to_string()),
            }),
            attachment_library: None,
            permissions: Default::default(),
        }))
        .with_runtime_services("run-1".to_string(), None)
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
            upstream_catalog_digest: manifest.provider_contract.upstream_catalog_digest.clone(),
            provider_policy_digest: manifest.provider_contract.policy_digest.clone(),
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
            manifest_digest: manifest.manifest_digest.clone(),
            upstream_catalog_digest: manifest.provider_contract.upstream_catalog_digest.clone(),
            provider_policy_digest: manifest.provider_contract.policy_digest.clone(),
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
            }) if capability_id.as_ref() == "browser.automation"
                && managed_mcp_id.as_ref() == "builtin.browser_automation.mcp"
                && tool_id.as_ref() == "browser.snapshot"
                && model_name.as_ref() == "browser_snapshot"
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
            upstream_catalog_digest: manifest.provider_contract.upstream_catalog_digest.clone(),
            provider_policy_digest: manifest.provider_contract.policy_digest.clone(),
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
            upstream_catalog_digest: manifest.provider_contract.upstream_catalog_digest.clone(),
            provider_policy_digest: manifest.provider_contract.policy_digest.clone(),
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
            args: json!({
                "value": secret,
                "call_reason": format!("使用密码 {secret}；Use {secret}"),
            }),
            approval_status: AgentApprovalStatus::Approved,
            reason: Some(secret.to_string()),
        };
        for projected in [
            tool.trace_call_projection(&call),
            tool.checkpoint_call_projection(&call),
        ] {
            assert_eq!(projected.args, json!({}));
            assert_eq!(projected.reason, None);
            assert!(!serde_json::to_string(&projected).unwrap().contains(secret));
        }
        let event_call = tool.event_call_projection(&call);
        assert_eq!(event_call.args, json!({}));
        assert_eq!(
            event_call.reason.as_deref(),
            Some("The model requests this reviewed managed-browser operation.")
        );
        assert!(!serde_json::to_string(&event_call).unwrap().contains(secret));
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

        let artifact = json!({
            "schemaVersion": 1,
            "artifactId": "browser-artifact:123e4567-e89b-42d3-a456-426614174000",
            "kind": "image",
            "displayName": "page.png",
            "mimeType": "image/png",
            "sizeBytes": 42,
            "createdAt": 1_000,
            "expiresAt": 2_000,
            "lifecycle": "run",
            "owner": "browser_automation",
            "preview": "image"
        });
        let artifact_result = AgentToolResult {
            exact_archive_file: None,
            call_id: call.id.clone(),
            tool: call.tool.clone(),
            ok: true,
            result: Some(json!({
                "type": "managed_mcp_tool_result",
                "structuredContent": {
                    "status": "completed",
                    "artifacts": [artifact.clone()],
                    "privateDiagnostic": secret,
                },
                "content": [{"type": "text", "text": secret}],
            })),
            error: None,
        };
        let projected = tool.checkpoint_projection(&artifact_result);
        let safe = projected.result.unwrap();
        assert_eq!(safe["artifacts"], json!([artifact.clone()]));
        assert!(!serde_json::to_string(&safe).unwrap().contains(secret));

        let mut malformed = artifact;
        malformed["managedPath"] = json!("/tmp/private-canary.png");
        let malformed_result = AgentToolResult {
            exact_archive_file: None,
            call_id: call.id.clone(),
            tool: call.tool.clone(),
            ok: true,
            result: Some(json!({
                "structuredContent": {"artifacts": [malformed]}
            })),
            error: None,
        };
        let safe = tool.event_projection(&malformed_result).result.unwrap();
        assert!(safe.get("artifacts").is_none());
        assert!(!serde_json::to_string(&safe)
            .unwrap()
            .contains("private-canary"));

        let outcome_unknown = AgentToolResult {
            exact_archive_file: None,
            call_id: call.id.clone(),
            tool: call.tool.clone(),
            ok: false,
            result: Some(json!({
                "type": "mcp_tool",
                "code": "outcomeUnknown",
                "errorCode": "mcp.tool_outcome_unknown",
                "dispatchCertainty": "possibly_dispatched",
                "privateDiagnostic": secret,
            })),
            error: Some(secret.to_string()),
        };
        let projected = tool.event_projection(&outcome_unknown);
        let safe = projected.result.unwrap();
        assert_eq!(safe["status"], "outcome_unknown");
        assert_eq!(safe["errorCode"], "mcp.tool_outcome_unknown");
        assert_eq!(safe["retryable"], false);
        assert!(!serde_json::to_string(&safe).unwrap().contains(secret));

        let cancelled = AgentToolResult {
            exact_archive_file: None,
            call_id: call.id.clone(),
            tool: call.tool.clone(),
            ok: false,
            result: Some(json!({
                "status": "cancelled",
                "errorCode": "mcp.tool_cancelled",
                "privateDiagnostic": secret,
            })),
            error: Some(secret.to_string()),
        };
        let safe = tool.checkpoint_projection(&cancelled).result.unwrap();
        assert_eq!(safe["status"], "cancelled");
        assert!(!serde_json::to_string(&safe).unwrap().contains(secret));

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

    #[test]
    fn builtin_browser_failed_projection_keeps_only_safe_diagnostic_allowlist() {
        let secret = "BUILTIN_BROWSER_FAILURE_PRIVATE_CANARY_7Yp9";
        let raw = AgentToolResult {
            exact_archive_file: None,
            call_id: "browser-call-1".to_string(),
            tool: "browser_evaluate".to_string(),
            ok: false,
            result: Some(json!({
                "errorCode": "builtin.browser_automation.tool_error",
                "dispatchCertainty": "response_received",
                "failureStage": "tool_execution",
                "durationMs": 61,
                "message": secret,
                "content": [{"type": "text", "text": secret}],
                "args": {"password": secret},
                "url": format!("https://{secret}.invalid/private"),
                "credentials": {"username": secret, "password": secret},
                "structuredContent": {
                    "errorCode": "target_closed",
                    "failureStage": secret,
                    "privateDiagnostic": secret,
                },
            })),
            error: Some(secret.to_string()),
        };

        let projected = builtin_capability_tool_result_persistence_projection(&raw);
        let safe = projected.result.as_ref().unwrap();
        assert_eq!(safe["status"], "failed");
        assert_eq!(safe["errorCode"], "builtin.browser_automation.tool_error");
        assert_eq!(safe["dispatchCertainty"], "response_received");
        assert_eq!(safe["failureStage"], "tool_execution");
        assert_eq!(safe["durationMs"], 61);
        assert_eq!(safe["contentOmitted"], true);
        let keys = safe
            .as_object()
            .unwrap()
            .keys()
            .cloned()
            .collect::<Vec<_>>();
        assert_eq!(
            keys,
            vec![
                "contentOmitted",
                "dispatchCertainty",
                "durationMs",
                "errorCode",
                "failureStage",
                "schemaVersion",
                "status",
                "type",
            ]
        );
        let serialized = serde_json::to_string(&projected).unwrap();
        assert!(!serialized.contains(secret));
        assert!(!serialized.contains("https://"));
        assert!(!serialized.contains("password"));
        assert!(!serialized.contains("target_closed"));

        let mut non_browser = raw;
        non_browser.tool = "future_builtin_capability".to_string();
        let non_browser_safe = builtin_capability_tool_result_persistence_projection(&non_browser)
            .result
            .unwrap();
        assert!(non_browser_safe.get("errorCode").is_none());
        assert!(non_browser_safe.get("dispatchCertainty").is_none());
        assert!(non_browser_safe.get("failureStage").is_none());
        assert!(non_browser_safe.get("durationMs").is_none());
    }

    #[test]
    fn builtin_browser_failed_projection_rejects_untrusted_diagnostic_values() {
        let secret = "BUILTIN_BROWSER_DIAGNOSTIC_VALUE_CANARY";
        let raw = AgentToolResult {
            exact_archive_file: None,
            call_id: "browser-call-2".to_string(),
            tool: "browser_snapshot".to_string(),
            ok: false,
            result: Some(json!({
                "errorCode": secret,
                "dispatchCertainty": format!("response_received_{secret}"),
                "failureStage": format!("https://user:{secret}@example.invalid"),
                "durationMs": BUILTIN_BROWSER_DIAGNOSTIC_MAX_DURATION_MS + 1,
                "structuredContent": {
                    "errorCode": "target_closed",
                    "dispatchCertainty": "response_received",
                    "failureStage": "tool_execution",
                    "durationMs": 1,
                },
            })),
            error: Some(secret.to_string()),
        };

        let projected = builtin_capability_tool_result_persistence_projection(&raw);
        let safe = projected.result.as_ref().unwrap();
        assert_eq!(
            safe,
            &json!({
                "schemaVersion": 1,
                "type": "builtin_capability_tool",
                "status": "failed",
                "contentOmitted": true,
            })
        );
        assert!(!serde_json::to_string(&projected).unwrap().contains(secret));
    }

    #[test]
    fn builtin_browser_outcome_unknown_keeps_safe_stage_and_duration() {
        let secret = "BUILTIN_BROWSER_OUTCOME_UNKNOWN_PRIVATE_CANARY";
        let raw = AgentToolResult {
            exact_archive_file: None,
            call_id: "browser-call-3".to_string(),
            tool: "browser_type".to_string(),
            ok: false,
            result: Some(json!({
                "code": "outcomeUnknown",
                "errorCode": "mcp.tool_outcome_unknown",
                "dispatchCertainty": "possibly_dispatched",
                "failureStage": "completion",
                "durationMs": 60_001,
                "message": secret,
            })),
            error: Some(secret.to_string()),
        };

        let projected = builtin_capability_tool_result_persistence_projection(&raw);
        let safe = projected.result.as_ref().unwrap();
        assert_eq!(safe["status"], "outcome_unknown");
        assert_eq!(safe["errorCode"], "mcp.tool_outcome_unknown");
        assert_eq!(safe["dispatchCertainty"], "possibly_dispatched");
        assert_eq!(safe["failureStage"], "completion");
        assert_eq!(safe["durationMs"], 60_001);
        assert_eq!(safe["retryable"], false);
        assert!(!serde_json::to_string(&projected).unwrap().contains(secret));
    }

    #[tokio::test]
    async fn sensitive_approval_uses_only_host_owned_value_free_reason_text() {
        let (runtime, _) = sensitive_harness();
        let manifest = runtime.manifests()[0].clone();
        let tool =
            BuiltinCapabilityAgentTool::new(&manifest, manifest.tools[0].clone(), runtime).unwrap();
        let canaries = [
            "UNLABELLED_PASSWORD_CANARY_7Yp9",
            "COOKIE_VALUE_CANARY",
            "STORAGE_VALUE_CANARY",
            "browser-file:123e4567-e89b-42d3-a456-426614174000",
        ];
        let call = AgentToolCall {
            id: "call-1".to_string(),
            tool: "browser_evaluate".to_string(),
            args: json!({
                "function": "() => ({password: 'UNLABELLED_PASSWORD_CANARY_7Yp9', cookie: 'COOKIE_VALUE_CANARY', storage: 'STORAGE_VALUE_CANARY'})",
                "call_reason": "使用密码 UNLABELLED_PASSWORD_CANARY_7Yp9 登录；Use UNLABELLED_PASSWORD_CANARY_7Yp9；读取 COOKIE_VALUE_CANARY、STORAGE_VALUE_CANARY 和 browser-file:123e4567-e89b-42d3-a456-426614174000"
            }),
            approval_status: AgentApprovalStatus::Required,
            reason: None,
        };
        let sync_error = tool.proposed_action(&context(), &call).unwrap_err();
        assert!(sync_error.to_string().contains("异步 Host 页面绑定"));
        let tool_context = context();
        let action = tool
            .proposed_action_async(&tool_context, &call)
            .await
            .unwrap();
        let AgentProposedAction::BuiltinMcpToolApproval { approval } = action else {
            panic!("expected a typed sensitive built-in MCP approval");
        };
        assert_eq!(
            approval.call_reason,
            "The model requests running a reviewed script in the managed page."
        );
        let serialized = serde_json::to_string(&approval).unwrap();
        for canary in canaries {
            assert!(!serialized.contains(canary));
        }
        assert!(!serialized.contains("使用密码"));
        assert!(!serialized.contains("UNLABELLED_PASSWORD_CANARY_7Yp9"));
        assert_eq!(
            tool.event_call_projection(&call).reason.as_deref(),
            Some("The model requests running a reviewed script in the managed page.")
        );
        // The exact raw invocation remains available only to the in-process Provider binding;
        // persistent/event/checkpoint projections of the original ToolCall stay empty.
        for projected in [
            tool.trace_call_projection(&call),
            tool.checkpoint_call_projection(&call),
            tool.event_call_projection(&call),
        ] {
            let projected = serde_json::to_string(&projected).unwrap();
            for canary in canaries {
                assert!(!projected.contains(canary));
            }
        }

        for (index, target) in [
            "f1e2",
            "frameLocator(\"iframe\").locator(\"[contenteditable]\")",
            "managed-frame-editor:00000000-0000-4000-8000-000000000000",
            "#unknown-selector",
        ]
        .into_iter()
        .enumerate()
        {
            let frame_call = AgentToolCall {
                id: format!("call-frame-{index}"),
                tool: "browser_evaluate".to_string(),
                args: json!({
                    "function": "(element) => element.textContent",
                    "element": "Embedded editor",
                    "target": target,
                    "call_reason": "Read the embedded editor."
                }),
                approval_status: AgentApprovalStatus::Required,
                reason: None,
            };
            let error = tool
                .proposed_action_async(&tool_context, &frame_call)
                .await
                .unwrap_err();
            assert_eq!(
                error.code(),
                Some("builtin_mcp_tool.sensitive_target_scope_unsupported")
            );
        }
    }

    #[test]
    fn sensitive_file_preflight_accepts_absolute_paths_only_inside_the_workspace() {
        let root = tempfile::tempdir().unwrap();
        let fixture = root.path().join("浙江大学2026年招生资料汇编.pptx");
        std::fs::write(&fixture, b"fixture-presentation").unwrap();
        let context = workspace_context(root.path());

        let prepared = sensitive_file_preparation(
            &context,
            "browser_file_upload",
            &json!({"paths": [fixture.to_string_lossy()]}),
        )
        .unwrap();
        assert_eq!(
            prepared,
            Some(BuiltinMcpToolFilePreparation::ResolvedPaths(vec![fixture
                .canonicalize()
                .unwrap()
                .to_string_lossy()
                .to_string()]))
        );

        let outside = tempfile::NamedTempFile::new().unwrap();
        let error = sensitive_file_preparation(
            &context,
            "browser_file_upload",
            &json!({"paths": [outside.path().to_string_lossy()]}),
        )
        .unwrap_err();
        assert_eq!(error.to_string(), "browser.file.invalid_file");

        let secret_path = root.path().join("PRIVATE_ABSOLUTE_PATH_CANARY");
        let error = sensitive_file_preparation(
            &context,
            "browser_file_upload",
            &json!({"paths": [secret_path.to_string_lossy()]}),
        )
        .unwrap_err();
        assert_eq!(error.to_string(), "browser.file.invalid_file");
        assert!(!error.to_string().contains("PRIVATE_ABSOLUTE_PATH_CANARY"));
    }

    #[test]
    fn sensitive_file_preflight_preserves_official_upload_cancel_semantics_without_paths() {
        assert_eq!(
            sensitive_file_preparation(&context(), "browser_file_upload", &json!({})).unwrap(),
            None
        );
        assert_eq!(
            sensitive_file_preparation(&context(), "browser_file_upload", &json!({"paths": []}))
                .unwrap(),
            None
        );
        assert_eq!(
            sensitive_file_preparation(
                &context(),
                "browser_drop",
                &json!({"target": "Drop zone", "data": [{"mimeType": "text/plain", "data": "hello"}]})
            )
            .unwrap(),
            None
        );
    }

    #[test]
    fn cookie_and_storage_state_approvals_name_the_global_managed_profile_scope() {
        for (tool_id, operation) in [
            ("browser_cookie_get", "cookie_read"),
            ("browser_cookie_list", "cookie_read"),
            ("browser_cookie_clear", "cookie_write"),
            ("browser_cookie_delete", "cookie_write"),
            ("browser_cookie_set", "cookie_write"),
            ("browser_set_storage_state", "storage_state_import"),
            ("browser_storage_state", "storage_state_export"),
        ] {
            assert_eq!(
                sensitive_tool_scope(tool_id),
                (operation, "managed_browser_profile")
            );
        }
        assert_eq!(
            sensitive_tool_scope("browser_localstorage_get"),
            ("local_storage_read", "local_storage_read")
        );
        assert_eq!(
            sensitive_tool_scope("browser_sessionstorage_set"),
            ("session_storage_write", "session_storage_write")
        );
        for (tool_id, operation) in [
            ("browser_drop", "file_upload"),
            ("browser_evaluate", "page_script_execution"),
            ("browser_file_upload", "file_upload"),
            ("browser_network_request", "network_sensitive_read"),
        ] {
            assert_eq!(
                sensitive_tool_scope(tool_id),
                (operation, "managed_surface")
            );
        }
    }
}
