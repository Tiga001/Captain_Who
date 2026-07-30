use super::*;
use crate::protocol::{AgentApprovalStatus, AgentToolCall, AgentToolIdentity};
use crate::tools::{
    AgentToolCallCheckpointPersistence, AgentToolExposure, AgentToolPermissionPolicy,
    EffectiveToolSet, ToolRegistry,
};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Mutex;
use std::time::Duration;

#[test]
fn runtime_projection_limits_have_safe_serde_defaults_and_reject_weakening() {
    let defaults: McpRuntimeProjectionLimits = serde_json::from_value(json!({})).unwrap();
    assert_eq!(defaults, McpRuntimeProjectionLimits::SAFE_DEFAULT);
    defaults.validate().unwrap();
    assert_eq!(
        MCP_RUNTIME_MAX_TOOL_DEFINITIONS,
        defaults.max_tool_definitions
    );
    assert_eq!(MCP_RUNTIME_MAX_CATALOG_BYTES, defaults.max_catalog_bytes);

    let tightened: McpRuntimeProjectionLimits = serde_json::from_value(json!({
        "maxModelTextBytes": 1024,
        "maxContentBlocks": 8,
    }))
    .unwrap();
    assert_eq!(tightened.max_model_text_bytes, 1_024);
    assert_eq!(tightened.max_content_blocks, 8);
    assert_eq!(
        tightened.max_model_structured_bytes,
        defaults.max_model_structured_bytes
    );
    tightened.validate().unwrap();

    let weakened = McpRuntimeProjectionLimits {
        max_model_text_bytes: defaults.max_model_text_bytes + 1,
        ..defaults
    };
    assert_eq!(
        weakened.validate().unwrap_err().code(),
        Some("mcp.invalid_runtime_projection_limits")
    );
    assert!(serde_json::from_value::<McpRuntimeProjectionLimits>(json!({
        "maxModelTextBytez": 1024,
    }))
    .is_err());
}

#[derive(Clone)]
enum MockBehavior {
    Return(McpToolInvocationResult),
    WaitForCancellation,
}

struct MockMcpToolInvoker {
    catalog: Vec<McpAgentToolDescriptor>,
    behavior: MockBehavior,
    prepared: Mutex<BTreeMap<String, (AgentMcpToolApproval, Value)>>,
    invocations: Mutex<Vec<McpApprovedToolInvocation>>,
    consumed_arguments: Mutex<Vec<Value>>,
    invalidated: Mutex<Vec<AgentMcpToolInvocationIdentity>>,
    cancellation_tokens: Mutex<Vec<AgentCancellationToken>>,
    diagnostics: Mutex<Vec<McpToolRegistrationDiagnostic>>,
}

impl MockMcpToolInvoker {
    fn returning(
        catalog: Vec<McpAgentToolDescriptor>,
        result: McpToolInvocationResult,
    ) -> Arc<Self> {
        Arc::new(Self {
            catalog,
            behavior: MockBehavior::Return(result),
            prepared: Mutex::new(BTreeMap::new()),
            invocations: Mutex::new(Vec::new()),
            consumed_arguments: Mutex::new(Vec::new()),
            invalidated: Mutex::new(Vec::new()),
            cancellation_tokens: Mutex::new(Vec::new()),
            diagnostics: Mutex::new(Vec::new()),
        })
    }

    fn waiting_for_cancellation(catalog: Vec<McpAgentToolDescriptor>) -> Arc<Self> {
        Arc::new(Self {
            catalog,
            behavior: MockBehavior::WaitForCancellation,
            prepared: Mutex::new(BTreeMap::new()),
            invocations: Mutex::new(Vec::new()),
            consumed_arguments: Mutex::new(Vec::new()),
            invalidated: Mutex::new(Vec::new()),
            cancellation_tokens: Mutex::new(Vec::new()),
            diagnostics: Mutex::new(Vec::new()),
        })
    }
}

impl McpToolInvoker for MockMcpToolInvoker {
    fn catalog(
        &self,
        _context: &McpToolCatalogContext,
    ) -> AgentResult<Vec<McpAgentToolDescriptor>> {
        Ok(self.catalog.clone())
    }

    fn prepare_approval(
        &self,
        request: McpToolApprovalRequest,
    ) -> AgentResult<AgentMcpToolApproval> {
        let (approval, arguments, _) = request.into_parts();
        validate_mcp_approval_arguments(&approval, &arguments)?;
        self.prepared.lock().expect("prepared mutex").insert(
            approval.identity.invocation_id.clone(),
            (approval.clone(), arguments),
        );
        Ok(approval)
    }

    fn invalidate_prepared_approval(
        &self,
        identity: &AgentMcpToolInvocationIdentity,
    ) -> AgentResult<()> {
        self.prepared
            .lock()
            .expect("prepared mutex")
            .remove(&identity.invocation_id);
        self.invalidated
            .lock()
            .expect("invalidated mutex")
            .push(identity.clone());
        Ok(())
    }

    fn invoke_approved<'a>(
        &'a self,
        invocation: McpApprovedToolInvocation,
        cancellation: AgentCancellationToken,
    ) -> McpToolInvocationFuture<'a> {
        let prepared = self
            .prepared
            .lock()
            .expect("prepared mutex")
            .remove(&invocation.approval.identity.invocation_id);
        let Some((approval, arguments)) = prepared else {
            return Box::pin(async {
                Err(AgentError::new(
                    "mock MCP invocation was not prepared or was already consumed",
                ))
            });
        };
        if approval != invocation.approval {
            return Box::pin(async { Err(AgentError::new("mock MCP approval binding changed")) });
        }
        self.invocations
            .lock()
            .expect("invocations mutex")
            .push(invocation);
        self.consumed_arguments
            .lock()
            .expect("consumed arguments mutex")
            .push(arguments);
        self.cancellation_tokens
            .lock()
            .expect("cancellation tokens mutex")
            .push(cancellation.clone());
        match &self.behavior {
            MockBehavior::Return(result) => {
                let result = result.clone();
                Box::pin(async move { Ok(result) })
            }
            MockBehavior::WaitForCancellation => Box::pin(async move {
                cancellation.cancelled().await;
                Err(AgentError::cancelled())
            }),
        }
    }

    fn report_diagnostics(&self, diagnostics: &[McpToolRegistrationDiagnostic]) {
        *self.diagnostics.lock().expect("diagnostics mutex") = diagnostics.to_vec();
    }
}

struct TestBuiltinTool {
    name: &'static str,
    description: &'static str,
}

impl AgentTool for TestBuiltinTool {
    fn definition(&self) -> AgentToolDefinition {
        AgentToolDefinition {
            name: self.name.to_string(),
            description: self.description.to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {},
                "additionalProperties": false
            }),
            safety: AgentToolSafety::ReadOnly,
            requires_workspace: false,
            requires_approval: false,
            approval_mode: AgentToolApprovalMode::Never,
        }
    }

    fn execute(&self, _context: &ToolExecutionContext, _args: Value) -> AgentResult<Value> {
        Ok(json!({"source": "builtin"}))
    }

    fn permission_policy(&self) -> AgentToolPermissionPolicy {
        AgentToolPermissionPolicy::Default
    }

    fn exposure(&self) -> AgentToolExposure {
        AgentToolExposure::Stable
    }
}

fn provenance(raw_name: &str, model_name: &str) -> AgentMcpToolProvenance {
    let normalized =
        mcp_normalized_input_schema_identity(model_name, &json!({"type": "object"})).unwrap();
    AgentMcpToolProvenance {
        server_id: "4f4763c4-61a8-4a5a-9455-429f701548e1".to_string(),
        scope: AgentMcpServerScope::Project {
            project_id: "project-fixture".to_string(),
        },
        raw_tool_name: raw_name.to_string(),
        model_tool_name: model_name.to_string(),
        config_epoch: "bf616f04-d3ec-4bd7-825f-731a9f0892f4".to_string(),
        registry_revision: 11,
        config_digest: "a".repeat(64),
        catalog_generation: 7,
        catalog_digest: "b".repeat(64),
        catalog_schema_digest: "c".repeat(64),
        schema_digest: normalized.schema_digest,
        schema_normalizer_version: normalized.normalizer_version,
    }
}

fn provenance_for_schema(
    raw_name: &str,
    model_name: &str,
    input_schema: &Value,
) -> AgentMcpToolProvenance {
    let mut provenance = provenance(raw_name, model_name);
    let normalized = mcp_normalized_input_schema_identity(model_name, input_schema).ok();
    provenance.schema_digest = normalized
        .as_ref()
        .map(|identity| identity.schema_digest.clone())
        .unwrap_or_default();
    provenance.schema_normalizer_version = normalized
        .map(|identity| identity.normalizer_version)
        .unwrap_or_default();
    provenance
}

fn descriptor(raw_name: &str, model_name: &str, input_schema: Value) -> McpAgentToolDescriptor {
    McpAgentToolDescriptor {
        provenance: provenance_for_schema(raw_name, model_name, &input_schema),
        approval_mode: AgentMcpApprovalMode::Prompt,
        server_display_name: "Fixture MCP".to_string(),
        description: Some(format!("{raw_name} fixture tool")),
        input_schema,
        output_schema: Some(json!({
            "type": "object",
            "properties": {"internalOnly": {"type": "boolean"}}
        })),
        annotations: McpAgentToolAnnotations {
            read_only_hint: Some(true),
            destructive_hint: Some(false),
            idempotent_hint: Some(true),
            open_world_hint: Some(false),
        },
    }
}

fn empty_result() -> McpToolInvocationResult {
    McpToolInvocationResult {
        content: Vec::new(),
        structured_content: None,
        is_error: false,
        truncated_at_source: false,
    }
}

fn registry_with(invoker: Arc<MockMcpToolInvoker>) -> ToolRegistry {
    let invoker: Arc<dyn McpToolInvoker> = invoker;
    let runtime = McpToolRuntime::capture(invoker);
    let mut registry = ToolRegistry::empty();
    registry.register_mcp_runtime(&runtime);
    registry
}

fn call(tool: &str, args: Value) -> AgentToolCall {
    AgentToolCall {
        id: crate::llm::model_response_tool_call_id("mcp-test-run", 0, 0, "provider-mcp-call-1"),
        tool: tool.to_string(),
        args,
        approval_status: AgentApprovalStatus::NotRequired,
        reason: None,
    }
}

fn propose(registry: &ToolRegistry, tool: &str, args: Value) -> AgentResult<AgentMcpToolApproval> {
    let context = ToolExecutionContext::from_run_context(None)
        .with_runtime_services("mcp-test-run".to_string(), None);
    match registry.proposed_action(&context, &call(tool, args))? {
        AgentProposedAction::McpToolCall { approval } => Ok(*approval),
        _ => Err(AgentError::new("expected typed MCP approval")),
    }
}

async fn invoke_prepared(
    invoker: &MockMcpToolInvoker,
    approval: AgentMcpToolApproval,
    cancellation: AgentCancellationToken,
) -> AgentResult<AgentToolResult> {
    let result = invoker
        .invoke_approved(
            McpApprovedToolInvocation {
                approval: approval.clone(),
            },
            cancellation,
        )
        .await?;
    mcp_tool_result_from_approved_invocation(&approval, &result)
}

mod approval;
mod execution;
mod lifecycle;
mod registration;
mod schema;
