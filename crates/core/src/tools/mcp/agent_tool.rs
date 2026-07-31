//! Dynamic Agent-tool registration, approval preparation, and execution hooks.

use super::*;

pub(in crate::tools) struct McpAgentTool {
    invoker: Arc<dyn McpToolInvoker>,
    provenance: AgentMcpToolProvenance,
    server_display_name: String,
    approval_mode: AgentMcpApprovalMode,
    risk: AgentMcpToolRisk,
    caller: McpToolCatalogContext,
    definition: AgentToolDefinition,
    #[allow(dead_code)]
    output_schema: Option<Value>,
}

impl McpAgentTool {
    pub(in crate::tools) fn provenance(&self) -> &AgentMcpToolProvenance {
        &self.provenance
    }

    pub(in crate::tools) fn prepare(
        descriptor: McpAgentToolDescriptor,
        invoker: Arc<dyn McpToolInvoker>,
        caller: McpToolCatalogContext,
    ) -> Result<Self, McpToolRegistrationDiagnostic> {
        validate_catalog_provenance(&descriptor.provenance)?;
        let input_schema = normalize_input_schema(&descriptor.provenance, descriptor.input_schema)?;
        let normalized_identity =
            normalized_input_schema_identity(&input_schema).map_err(|_| {
                McpToolRegistrationDiagnostic::for_tool(
                    &descriptor.provenance,
                    McpToolDiagnosticCode::InvalidSchema,
                )
            })?;
        if descriptor.provenance.schema_digest != normalized_identity.schema_digest
            || descriptor.provenance.schema_normalizer_version
                != normalized_identity.normalizer_version
        {
            return Err(McpToolRegistrationDiagnostic::for_tool(
                &descriptor.provenance,
                McpToolDiagnosticCode::InvalidIdentity,
            ));
        }
        validate_provenance(&descriptor.provenance)?;
        if descriptor.approval_mode == AgentMcpApprovalMode::Deny {
            return Err(McpToolRegistrationDiagnostic::for_tool(
                &descriptor.provenance,
                McpToolDiagnosticCode::ApprovalRequiredUnsupported,
            ));
        }
        let description = normalize_description(descriptor.description.as_deref());
        let server_display_name =
            normalize_server_display_name(&descriptor.server_display_name, &descriptor.provenance);
        let risk = mcp_tool_risk(&descriptor.annotations);
        let definition = AgentToolDefinition {
            name: descriptor.provenance.model_tool_name.clone(),
            description,
            input_schema,
            safety: AgentToolSafety::RequiresApproval,
            requires_workspace: false,
            requires_approval: descriptor.approval_mode == AgentMcpApprovalMode::Prompt,
            approval_mode: if descriptor.approval_mode == AgentMcpApprovalMode::Prompt {
                AgentToolApprovalMode::Always
            } else {
                AgentToolApprovalMode::Never
            },
        };
        Ok(Self {
            invoker,
            provenance: descriptor.provenance,
            server_display_name,
            approval_mode: descriptor.approval_mode,
            risk,
            caller,
            definition,
            output_schema: descriptor.output_schema,
        })
    }
}

impl AgentTool for McpAgentTool {
    fn definition(&self) -> AgentToolDefinition {
        self.definition.clone()
    }

    fn execute(&self, _context: &ToolExecutionContext, _args: Value) -> AgentResult<Value> {
        Err(AgentError::new(
            "MCP tools require the asynchronous ToolRegistry execution path.",
        ))
    }

    fn permission_policy(&self) -> AgentToolPermissionPolicy {
        AgentToolPermissionPolicy::Default
    }

    fn cancellation_settlement(&self) -> crate::tools::AgentToolCancellationSettlement {
        // The Host bridge owns MCP cancellation notification and a bounded settlement grace.
        // Let that future observe the token instead of dropping it at the outer Registry select.
        crate::tools::AgentToolCancellationSettlement::Authoritative
    }

    fn exposure(&self) -> AgentToolExposure {
        AgentToolExposure::Dynamic
    }

    fn proposed_action(
        &self,
        context: &ToolExecutionContext,
        call: &AgentToolCall,
    ) -> AgentResult<AgentProposedAction> {
        if call.tool != self.provenance.model_tool_name {
            return Err(AgentError::structured(
                "mcp.invalid_tool_identity",
                "The MCP Tool Call does not match its frozen model-visible identity.",
                json!({
                    "type": "mcp_approval",
                    "code": "modelToolNameMismatch",
                    "retryable": false,
                }),
            ));
        }
        let (server_arguments, display_reason) = split_model_mcp_arguments(&call.args)?;
        let run_id = context.run_id()?.to_string();
        let created_at = crate::storage::now_ms();
        let identity = AgentMcpToolInvocationIdentity {
            action_id: uuid::Uuid::new_v4().to_string(),
            invocation_id: uuid::Uuid::new_v4().to_string(),
            run_id,
            call_id: call.id.clone(),
            provenance: self.provenance.clone(),
            arguments_digest: mcp_tool_arguments_digest(&server_arguments)?,
        };
        let mut projected_call = project_mcp_tool_call(call);
        projected_call.approval_status = if self.approval_mode == AgentMcpApprovalMode::Prompt {
            AgentApprovalStatus::Required
        } else {
            AgentApprovalStatus::Approved
        };
        let approval = AgentMcpToolApproval {
            identity,
            call: projected_call,
            summary: AgentMcpToolApprovalSummary {
                server_id: self.provenance.server_id.clone(),
                server_display_name: self.server_display_name.clone(),
                scope: self.provenance.scope.clone(),
                raw_tool_name: self.provenance.raw_tool_name.clone(),
                model_tool_name: self.provenance.model_tool_name.clone(),
                display_reason: Some(display_reason),
                arguments: summarize_mcp_arguments(&server_arguments)?,
                risk: self.risk,
                external: true,
            },
            approval_mode: self.approval_mode,
            payload_persistence: AgentMcpApprovalPayloadPersistence::ProcessOnly,
            created_at,
            expires_at: created_at.saturating_add(MCP_APPROVAL_TTL_MS),
        };
        validate_mcp_tool_approval(&approval)?;
        let prepared = self.invoker.prepare_approval(McpToolApprovalRequest {
            approval: approval.clone(),
            arguments: server_arguments,
            caller: self.caller.clone(),
        })?;
        let mut expected_prepared = approval.clone();
        expected_prepared.payload_persistence = prepared.payload_persistence;
        if prepared != expected_prepared {
            let _ = self
                .invoker
                .invalidate_prepared_approval(&approval.identity);
            let _ = self
                .invoker
                .invalidate_prepared_approval(&prepared.identity);
            return Err(AgentError::structured(
                "mcp.approval_binding_changed",
                "The MCP Host changed the frozen approval identity.",
                json!({
                    "type": "mcp_approval",
                    "code": "preparedApprovalMismatch",
                    "retryable": false,
                }),
            ));
        }
        validate_mcp_tool_approval(&prepared)?;
        Ok(AgentProposedAction::McpToolCall {
            approval: Box::new(prepared),
        })
    }

    fn invalidate_proposed_action(&self, action: &AgentProposedAction) -> AgentResult<()> {
        let AgentProposedAction::McpToolCall { approval } = action else {
            return Err(AgentError::new(
                "MCP Tool received a non-MCP action invalidation request.",
            ));
        };
        if approval.identity.provenance != self.provenance {
            return Err(AgentError::new(
                "MCP approval invalidation does not match the registered Tool identity.",
            ));
        }
        self.invoker
            .invalidate_prepared_approval(&approval.identity)
    }

    fn archives_result(&self) -> bool {
        false
    }

    fn auto_executes_prepared_action(&self) -> bool {
        self.approval_mode == AgentMcpApprovalMode::Auto
    }

    fn trace_call_projection(
        &self,
        call: &crate::protocol::AgentToolCall,
    ) -> crate::protocol::AgentToolCall {
        project_mcp_tool_call(call)
    }

    fn event_call_projection(
        &self,
        call: &crate::protocol::AgentToolCall,
    ) -> crate::protocol::AgentToolCall {
        project_mcp_tool_call(call)
    }

    fn model_call_projection(
        &self,
        call: &crate::protocol::AgentToolCall,
    ) -> crate::protocol::AgentToolCall {
        call.clone()
    }

    fn checkpoint_call_projection(
        &self,
        call: &crate::protocol::AgentToolCall,
    ) -> crate::protocol::AgentToolCall {
        project_mcp_tool_call(call)
    }

    fn model_projection(
        &self,
        result: &crate::protocol::AgentToolResult,
    ) -> crate::protocol::AgentToolResult {
        crate::tools::mcp_tool_result_model_projection(result)
    }

    fn trace_projection(
        &self,
        result: &crate::protocol::AgentToolResult,
    ) -> crate::protocol::AgentToolResult {
        self.persistence_projection(result)
    }

    fn archive_projection(
        &self,
        result: &crate::protocol::AgentToolResult,
    ) -> crate::protocol::AgentToolResult {
        self.persistence_projection(result)
    }

    fn checkpoint_projection(
        &self,
        result: &crate::protocol::AgentToolResult,
    ) -> crate::protocol::AgentToolResult {
        self.persistence_projection(result)
    }
}

impl McpAgentTool {
    fn persistence_projection(
        &self,
        result: &crate::protocol::AgentToolResult,
    ) -> crate::protocol::AgentToolResult {
        crate::tools::mcp_tool_result_persistence_projection(result)
    }
}

impl AsyncAgentTool for McpAgentTool {
    fn execute_async<'a>(
        &'a self,
        context: &'a ToolExecutionContext,
        args: Value,
    ) -> BoxAgentToolFuture<'a> {
        let _ = (context, args);
        Box::pin(async move {
            Err(AgentError::structured(
                "mcp.approval_required",
                "MCP tools can only execute through a one-time approved invocation.",
                json!({
                    "type": "mcp_approval",
                    "code": "approvedInvocationRequired",
                    "retryable": false,
                }),
            ))
        })
    }
}
