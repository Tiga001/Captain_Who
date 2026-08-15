use std::str::FromStr;
use std::sync::{Arc, Mutex};

use mycopilot_core::{
    mcp_normalized_input_schema_identity, validate_mcp_approval_arguments, AgentCancellationToken,
    AgentError, AgentMcpApprovalMode, AgentMcpApprovalPayloadPersistence, AgentMcpServerScope,
    AgentMcpToolApproval, AgentMcpToolInvocationIdentity, AgentMcpToolProvenance, AgentResult,
    McpAgentToolAnnotations, McpAgentToolDescriptor, McpApprovedToolInvocation,
    McpOmittedContentKind, McpRuntimeProjectionLimits, McpToolApprovalRequest,
    McpToolCatalogContext, McpToolContentBlock, McpToolInvocationFuture, McpToolInvocationResult,
    McpToolInvoker, McpToolRegistrationDiagnostic,
};
use mycopilot_mcp_client::{
    McpActiveCallId, McpApprovalMode, McpCancellationToken, McpCatalogCompleteness,
    McpCatalogDigest, McpCatalogTool, McpCatalogToolCall, McpCatalogToolCallIdentity,
    McpConfigDigest, McpConfigEpoch, McpConnectionManager, McpContentBlock, McpDispatchCertainty,
    McpEmbeddedResource, McpError, McpErrorKind, McpInvocationId, McpModelCallId, McpResourceLink,
    McpSchemaDigest, McpServerId, McpServerScope, McpServerState, McpToolAnnotations, McpToolId,
    McpToolResult, McpTrustLevel,
};
use sha2::{Digest, Sha256};

use crate::application::mcp::approval_payload_store::{
    InMemoryMcpApprovalPayloadStore, McpApprovalInvocationId, McpApprovalPayload,
    McpApprovalPayloadAad, McpApprovalPayloadPersistence, McpApprovalPayloadStore,
    McpApprovalPayloadStoreError, McpApprovalStartupInspector, McpApprovalStartupPayloadState,
};
const MAX_RETAINED_DIAGNOSTICS: usize = 1_024;

/// Host-only fail-closed gate for Registry approval reconciliation.
///
/// The protocol-neutral bridge depends only on this narrow capability. The
/// production Registry event sink supplies the sticky implementation.
pub(crate) trait McpRegistrySecurityGate: Send + Sync {
    fn ensure_reconciled(&self) -> Result<(), String>;
}
/// Process-owned adapter from the MCP protocol subsystem into the Agent Runtime's stable,
/// protocol-neutral invocation boundary.
///
/// The bridge owns no MCP connection. Every invocation is revalidated by the shared Connection
/// Manager against the immutable provenance captured for the current Agent run.
pub struct McpRuntimeBridge {
    manager: Arc<McpConnectionManager>,
    payloads: Arc<dyn McpApprovalPayloadStore>,
    registry_security_gate: Arc<dyn McpRegistrySecurityGate>,
    projection_limits: McpRuntimeProjectionLimits,
    diagnostics: Mutex<Vec<McpToolRegistrationDiagnostic>>,
}

struct OpenMcpRegistrySecurityGate;

impl McpRegistrySecurityGate for OpenMcpRegistrySecurityGate {
    fn ensure_reconciled(&self) -> Result<(), String> {
        Ok(())
    }
}

impl McpRuntimeBridge {
    #[allow(dead_code)] // Repository fixtures use the explicit process-only constructor.
    pub fn new(manager: Arc<McpConnectionManager>) -> Self {
        Self::with_payload_store(
            manager,
            Arc::new(InMemoryMcpApprovalPayloadStore::default()),
        )
    }

    pub(crate) fn with_payload_store(
        manager: Arc<McpConnectionManager>,
        payloads: Arc<dyn McpApprovalPayloadStore>,
    ) -> Self {
        Self::with_payload_store_projection_limits_and_security_gate(
            manager,
            payloads,
            McpRuntimeProjectionLimits::default(),
            Arc::new(OpenMcpRegistrySecurityGate),
        )
        .expect("the built-in MCP runtime projection policy must remain valid")
    }

    #[allow(dead_code)] // Used by the production binary; the narrow fixture library has no Registry sink.
    pub(crate) fn with_payload_store_and_registry_security_gate(
        manager: Arc<McpConnectionManager>,
        payloads: Arc<dyn McpApprovalPayloadStore>,
        registry_security_gate: Arc<dyn McpRegistrySecurityGate>,
    ) -> Self {
        Self::with_payload_store_projection_limits_and_security_gate(
            manager,
            payloads,
            McpRuntimeProjectionLimits::default(),
            registry_security_gate,
        )
        .expect("the built-in MCP runtime projection policy must remain valid")
    }

    #[allow(dead_code)] // Exercised by binary-side Host policy tests.
    pub(crate) fn with_payload_store_and_projection_limits(
        manager: Arc<McpConnectionManager>,
        payloads: Arc<dyn McpApprovalPayloadStore>,
        projection_limits: McpRuntimeProjectionLimits,
    ) -> AgentResult<Self> {
        Self::with_payload_store_projection_limits_and_security_gate(
            manager,
            payloads,
            projection_limits,
            Arc::new(OpenMcpRegistrySecurityGate),
        )
    }

    fn with_payload_store_projection_limits_and_security_gate(
        manager: Arc<McpConnectionManager>,
        payloads: Arc<dyn McpApprovalPayloadStore>,
        projection_limits: McpRuntimeProjectionLimits,
        registry_security_gate: Arc<dyn McpRegistrySecurityGate>,
    ) -> AgentResult<Self> {
        projection_limits.validate()?;
        Ok(Self {
            manager,
            payloads,
            registry_security_gate,
            projection_limits,
            diagnostics: Mutex::new(Vec::new()),
        })
    }

    #[cfg(test)]
    fn diagnostics(&self) -> Vec<McpToolRegistrationDiagnostic> {
        self.diagnostics
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clone()
    }

    fn revalidate_approval(
        &self,
        approval: &AgentMcpToolApproval,
        caller: Option<&McpToolCatalogContext>,
    ) -> AgentResult<()> {
        self.ensure_registry_reconciled()?;
        let provenance = &approval.identity.provenance;
        if approval.approval_mode == AgentMcpApprovalMode::Deny {
            return Err(policy_error("mcp.approval_policy_denied"));
        }
        let server_id = McpServerId::from_str(&provenance.server_id)
            .map_err(|_| stale_tool_error("mcp.invalid_server_identity"))?;
        let expected_epoch = McpConfigEpoch::from_str(&provenance.config_epoch)
            .map_err(|_| stale_tool_error("mcp.invalid_config_epoch"))?;
        let expected_digest = McpConfigDigest::from_str(&provenance.config_digest)
            .map_err(|_| stale_tool_error("mcp.invalid_config_digest"))?;
        let status = self
            .manager
            .get_status(server_id)
            .map_err(map_invocation_error)?
            .ok_or_else(|| stale_tool_error("mcp.server_not_registered"))?;
        let catalog = self
            .manager
            .catalog(server_id)
            .map_err(map_invocation_error)?
            .ok_or_else(|| stale_tool_error("mcp.catalog_not_available"))?;
        if !status.enabled
            || status.state != McpServerState::Ready
            || status.trust == McpTrustLevel::Untrusted
            || !approval_mode_matches(approval.approval_mode, status.approval_mode)
            || status.catalog_completeness != McpCatalogCompleteness::Complete
            || status.config_epoch != expected_epoch
            || status.registry_revision != provenance.registry_revision
            || status.config_digest != expected_digest
            || status.catalog_generation != provenance.catalog_generation
            || catalog
                .content_digest
                .as_ref()
                .map(ToString::to_string)
                .as_deref()
                != Some(provenance.catalog_digest.as_str())
            || !scope_matches_provenance(&status.scope, &provenance.scope)
            || caller.is_some_and(|caller| !scope_visible_to_context(&status.scope, caller))
            || catalog.server_id != server_id
            || catalog.completeness != McpCatalogCompleteness::Complete
            || catalog.source_config_epoch != Some(expected_epoch)
            || catalog.source_registry_revision != Some(provenance.registry_revision)
            || catalog.source_config_digest.as_ref() != Some(&expected_digest)
        {
            return Err(stale_tool_error("mcp.approval_snapshot_stale"));
        }
        let tool = catalog
            .tools
            .iter()
            .find(|tool| {
                tool.id.server_id == server_id && tool.raw_name == provenance.raw_tool_name
            })
            .ok_or_else(|| stale_tool_error("mcp.approval_tool_missing"))?;
        if !tool.routable
            || tool.id.raw_name != provenance.raw_tool_name
            || tool.model_name != provenance.model_tool_name
            || tool.schema_digest.to_string() != provenance.catalog_schema_digest
        {
            return Err(stale_tool_error("mcp.approval_tool_drift"));
        }
        let normalized_identity =
            mcp_normalized_input_schema_identity(&tool.model_name, &tool.descriptor.input_schema)
                .map_err(|_| stale_tool_error("mcp.approval_tool_schema_invalid"))?;
        if normalized_identity.schema_digest != provenance.schema_digest
            || normalized_identity.normalizer_version != provenance.schema_normalizer_version
        {
            return Err(stale_tool_error("mcp.approval_tool_schema_drift"));
        }
        Ok(())
    }

    fn ensure_registry_reconciled(&self) -> AgentResult<()> {
        self.registry_security_gate
            .ensure_reconciled()
            .map_err(|_| {
                AgentError::structured(
                    "mcp.registry_reconciliation_required",
                    "MCP approvals are temporarily blocked pending a fail-closed Registry reconciliation.",
                    serde_json::json!({
                        "type": "mcp_registry",
                        "code": "registryReconciliationRequired",
                        "retryable": false,
                    }),
                )
            })
    }

    fn current_payload_persistence(&self) -> AgentResult<AgentMcpApprovalPayloadPersistence> {
        match self.payloads.persistence() {
            McpApprovalPayloadPersistence::ProcessOnly => {
                Ok(AgentMcpApprovalPayloadPersistence::ProcessOnly)
            }
            McpApprovalPayloadPersistence::DurableAuthenticatedEnvelope => {
                Ok(AgentMcpApprovalPayloadPersistence::DurableAuthenticatedEnvelope)
            }
            McpApprovalPayloadPersistence::Unavailable => Err(AgentError::structured(
                "mcp.approval_payload_unavailable",
                "The MCP approval payload store is unavailable.",
                serde_json::json!({
                    "type": "mcp_approval",
                    "code": "payloadStoreUnavailable",
                    "retryable": false,
                }),
            )),
        }
    }

    fn ensure_payload_persistence_matches(
        &self,
        approval: &AgentMcpToolApproval,
    ) -> AgentResult<()> {
        if self.current_payload_persistence()? != approval.payload_persistence {
            return Err(AgentError::structured(
                "mcp.approval_payload_binding_changed",
                "The MCP approval payload persistence binding changed.",
                serde_json::json!({
                    "type": "mcp_approval",
                    "code": "payloadPersistenceChanged",
                    "retryable": false,
                }),
            ));
        }
        Ok(())
    }
}

impl McpApprovalStartupInspector for McpRuntimeBridge {
    fn inspect_startup_payload(
        &self,
        approval: &AgentMcpToolApproval,
    ) -> McpApprovalStartupPayloadState {
        if approval.expires_at <= mycopilot_core::storage::now_ms() {
            return McpApprovalStartupPayloadState::Expired;
        }
        if approval.payload_persistence
            != AgentMcpApprovalPayloadPersistence::DurableAuthenticatedEnvelope
            || self.payloads.persistence()
                != McpApprovalPayloadPersistence::DurableAuthenticatedEnvelope
        {
            return McpApprovalStartupPayloadState::Unavailable;
        }
        let Ok(invocation_id) =
            McpApprovalInvocationId::parse(approval.identity.invocation_id.clone())
        else {
            return McpApprovalStartupPayloadState::Unavailable;
        };
        match self.payloads.load(&invocation_id, &approval_aad(approval)) {
            Ok(_) => McpApprovalStartupPayloadState::DurableAvailable,
            Err(McpApprovalPayloadStoreError::PayloadExpired) => {
                McpApprovalStartupPayloadState::Expired
            }
            Err(_) => McpApprovalStartupPayloadState::Unavailable,
        }
    }
}

impl McpToolInvoker for McpRuntimeBridge {
    fn catalog(&self, context: &McpToolCatalogContext) -> AgentResult<Vec<McpAgentToolDescriptor>> {
        self.ensure_registry_reconciled()?;
        let statuses = self.manager.list_statuses().map_err(map_catalog_error)?;
        let mut tools = Vec::new();
        let mut catalog_tool_count = 0_usize;
        let mut catalog_bytes = 0_usize;
        for status in statuses {
            if !status.enabled
                || status.state != McpServerState::Ready
                || status.trust == McpTrustLevel::Untrusted
                || status.approval_mode == McpApprovalMode::Deny
                || status.catalog_completeness != McpCatalogCompleteness::Complete
                || !scope_visible_to_context(&status.scope, context)
            {
                continue;
            }
            let Some(catalog) = self
                .manager
                .catalog(status.server_id)
                .map_err(map_catalog_error)?
            else {
                continue;
            };
            let Some(current_status) = self
                .manager
                .get_status(status.server_id)
                .map_err(map_catalog_error)?
            else {
                continue;
            };
            if !current_status.enabled
                || current_status.state != McpServerState::Ready
                || current_status.catalog_completeness != McpCatalogCompleteness::Complete
                || current_status.config_epoch != status.config_epoch
                || current_status.registry_revision != status.registry_revision
                || current_status.config_digest != status.config_digest
                || current_status.catalog_generation != catalog.generation
                || current_status.scope != status.scope
                || current_status.trust != status.trust
                || current_status.trust == McpTrustLevel::Untrusted
                || current_status.approval_mode == McpApprovalMode::Deny
                || current_status.approval_mode != status.approval_mode
                || !scope_visible_to_context(&current_status.scope, context)
            {
                continue;
            }
            let Some(scope) = map_scope(&current_status.scope) else {
                continue;
            };
            if catalog.server_id != status.server_id
                || catalog.completeness != McpCatalogCompleteness::Complete
                || catalog.source_config_epoch != Some(current_status.config_epoch)
                || catalog.source_registry_revision != Some(current_status.registry_revision)
                || catalog.source_config_digest.as_ref() != Some(&current_status.config_digest)
                || catalog.generation == 0
            {
                continue;
            }
            let Some(catalog_digest) = catalog.content_digest.as_ref().map(ToString::to_string)
            else {
                continue;
            };
            for tool in catalog.tools.into_iter().filter(|tool| tool.routable) {
                if tool.id.server_id != status.server_id || tool.raw_name != tool.id.raw_name {
                    continue;
                }
                reserve_catalog_budget(
                    &tool,
                    &mut catalog_tool_count,
                    &mut catalog_bytes,
                    &self.projection_limits,
                )?;
                let normalized_identity = mcp_normalized_input_schema_identity(
                    &tool.model_name,
                    &tool.descriptor.input_schema,
                )
                .ok();
                let provenance = AgentMcpToolProvenance {
                    server_id: status.server_id.to_string(),
                    scope: scope.clone(),
                    raw_tool_name: tool.raw_name,
                    model_tool_name: tool.model_name,
                    config_epoch: current_status.config_epoch.to_string(),
                    registry_revision: current_status.registry_revision,
                    config_digest: current_status.config_digest.to_string(),
                    catalog_generation: catalog.generation,
                    catalog_digest: catalog_digest.clone(),
                    catalog_schema_digest: tool.schema_digest.to_string(),
                    schema_digest: normalized_identity
                        .as_ref()
                        .map(|identity| identity.schema_digest.clone())
                        .unwrap_or_default(),
                    schema_normalizer_version: normalized_identity
                        .map(|identity| identity.normalizer_version)
                        .unwrap_or_default(),
                };
                tools.push(McpAgentToolDescriptor {
                    provenance,
                    approval_mode: map_approval_mode(current_status.approval_mode)
                        .ok_or_else(|| policy_error("mcp.approval_policy_denied"))?,
                    server_display_name: current_status.display_name.clone(),
                    description: tool.descriptor.description,
                    input_schema: tool.descriptor.input_schema,
                    output_schema: tool.descriptor.output_schema,
                    annotations: map_annotations(tool.descriptor.annotations),
                });
            }
        }
        tools.sort_by(|left, right| {
            left.provenance
                .model_tool_name
                .cmp(&right.provenance.model_tool_name)
                .then_with(|| left.provenance.server_id.cmp(&right.provenance.server_id))
                .then_with(|| {
                    left.provenance
                        .raw_tool_name
                        .cmp(&right.provenance.raw_tool_name)
                })
        });
        Ok(tools)
    }

    fn prepare_approval(
        &self,
        request: McpToolApprovalRequest,
    ) -> AgentResult<AgentMcpToolApproval> {
        let (mut approval, arguments, caller) = request.into_parts();
        validate_mcp_approval_arguments(&approval, &arguments)?;
        self.revalidate_approval(&approval, Some(&caller))?;
        approval.payload_persistence = self.current_payload_persistence()?;
        let invocation_id = McpApprovalInvocationId::parse(approval.identity.invocation_id.clone())
            .map_err(map_payload_error)?;
        let aad = approval_aad(&approval);
        let payload = McpApprovalPayload::from_json(&arguments).map_err(map_payload_error)?;
        self.payloads
            .seal(&invocation_id, aad, payload)
            .map_err(map_payload_error)?;
        Ok(approval)
    }

    fn invalidate_prepared_approval(
        &self,
        identity: &AgentMcpToolInvocationIdentity,
    ) -> AgentResult<()> {
        let invocation_id = McpApprovalInvocationId::parse(identity.invocation_id.clone())
            .map_err(map_payload_error)?;
        self.payloads
            .delete(&invocation_id)
            .map(|_| ())
            .map_err(map_payload_error)
    }

    fn revalidate_approved(&self, approval: &AgentMcpToolApproval) -> AgentResult<()> {
        self.revalidate_approval(approval, None)?;
        self.ensure_payload_persistence_matches(approval)?;
        let invocation_id = McpApprovalInvocationId::parse(approval.identity.invocation_id.clone())
            .map_err(map_payload_error)?;
        let aad = approval_aad(approval);
        let payload = self
            .payloads
            .load(&invocation_id, &aad)
            .map_err(map_payload_error)?;
        payload
            .with_json(|arguments| validate_mcp_approval_arguments(approval, arguments))
            .map_err(map_payload_error)?
    }

    fn invoke_approved<'a>(
        &'a self,
        invocation: McpApprovedToolInvocation,
        cancellation: AgentCancellationToken,
    ) -> McpToolInvocationFuture<'a> {
        Box::pin(async move {
            cancellation.check()?;
            let approval = invocation.approval;
            self.revalidate_approval(&approval, None)?;
            self.ensure_payload_persistence_matches(&approval)?;
            let invocation_id =
                McpApprovalInvocationId::parse(approval.identity.invocation_id.clone())
                    .map_err(map_payload_error)?;
            let aad = approval_aad(&approval);
            let payload = self
                .payloads
                .consume(&invocation_id, &aad)
                .map_err(map_payload_error)?;
            let arguments = payload.with_json(Clone::clone).map_err(map_payload_error)?;
            validate_mcp_approval_arguments(&approval, &arguments)?;
            cancellation.check()?;
            let request = catalog_call(&approval, arguments)?;
            let active_id = active_call_id(&approval)?;
            self.ensure_registry_reconciled()?;
            let mcp_cancellation = McpCancellationToken::new();
            let call = self.manager.call_catalog_tool_identified(
                active_id,
                request,
                mcp_cancellation.clone(),
            );
            tokio::pin!(call);
            tokio::select! {
                biased;
                _ = cancellation.cancelled() => {
                    mcp_cancellation.cancel();
                    map_tool_result(
                        call.await.map_err(map_invocation_error)?,
                        &self.projection_limits,
                    )
                }
                result = &mut call => map_tool_result(
                    result.map_err(map_invocation_error)?,
                    &self.projection_limits,
                ),
            }
        })
    }

    fn report_diagnostics(&self, diagnostics: &[McpToolRegistrationDiagnostic]) {
        let mut retained = self
            .diagnostics
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        retained.clear();
        retained.extend(diagnostics.iter().take(MAX_RETAINED_DIAGNOSTICS).cloned());
    }
}

fn reserve_catalog_budget(
    tool: &McpCatalogTool,
    tool_count: &mut usize,
    bytes: &mut usize,
    limits: &McpRuntimeProjectionLimits,
) -> AgentResult<()> {
    let next_count = tool_count.checked_add(1).ok_or_else(catalog_budget_error)?;
    let mut next_bytes = bytes
        .checked_add(tool.raw_name.len())
        .and_then(|value| value.checked_add(tool.model_name.len()))
        .and_then(|value| {
            value.checked_add(tool.descriptor.description.as_ref().map_or(0, String::len))
        })
        .ok_or_else(catalog_budget_error)?;
    for schema in
        std::iter::once(&tool.descriptor.input_schema).chain(tool.descriptor.output_schema.as_ref())
    {
        let encoded = serde_json::to_vec(schema).map_err(|_| catalog_budget_error())?;
        next_bytes = next_bytes
            .checked_add(encoded.len())
            .ok_or_else(catalog_budget_error)?;
    }
    if next_count > limits.max_tool_definitions || next_bytes > limits.max_catalog_bytes {
        return Err(catalog_budget_error());
    }
    *tool_count = next_count;
    *bytes = next_bytes;
    Ok(())
}

fn catalog_budget_error() -> AgentError {
    AgentError::structured(
        "mcp.catalog_budget_exceeded",
        "The MCP catalog exceeds the Host tool budget; built-in Agent tools remain available.",
        serde_json::json!({
            "type": "mcp_catalog",
            "code": "catalogBudgetExceeded",
            "retryable": false,
        }),
    )
}

fn catalog_call(
    approval: &AgentMcpToolApproval,
    arguments: serde_json::Value,
) -> AgentResult<McpCatalogToolCall> {
    let provenance = &approval.identity.provenance;
    let server_id = McpServerId::from_str(&provenance.server_id)
        .map_err(|_| stale_tool_error("mcp.invalid_server_identity"))?;
    let config_epoch = McpConfigEpoch::from_str(&provenance.config_epoch)
        .map_err(|_| stale_tool_error("mcp.invalid_config_epoch"))?;
    let config_digest = McpConfigDigest::from_str(&provenance.config_digest)
        .map_err(|_| stale_tool_error("mcp.invalid_config_digest"))?;
    let catalog_digest = McpCatalogDigest::from_str(&provenance.catalog_digest)
        .map_err(|_| stale_tool_error("mcp.invalid_catalog_digest"))?;
    let catalog_schema_digest = McpSchemaDigest::from_str(&provenance.catalog_schema_digest)
        .map_err(|_| stale_tool_error("mcp.invalid_catalog_schema_digest"))?;
    if provenance.raw_tool_name.trim().is_empty()
        || provenance.model_tool_name.trim().is_empty()
        || provenance.registry_revision == 0
        || provenance.catalog_generation == 0
    {
        return Err(stale_tool_error("mcp.invalid_tool_identity"));
    }
    Ok(McpCatalogToolCall::new(
        McpCatalogToolCallIdentity {
            tool_id: McpToolId {
                server_id,
                raw_name: provenance.raw_tool_name.clone(),
            },
            expected_config_epoch: config_epoch,
            expected_registry_revision: provenance.registry_revision,
            expected_config_digest: config_digest,
            expected_catalog_generation: provenance.catalog_generation,
            expected_catalog_digest: catalog_digest,
            expected_schema_digest: catalog_schema_digest,
            expected_model_name: provenance.model_tool_name.clone(),
        },
        arguments,
    ))
}

fn scope_visible_to_context(scope: &McpServerScope, context: &McpToolCatalogContext) -> bool {
    match scope {
        McpServerScope::Builtin | McpServerScope::User | McpServerScope::Managed => true,
        McpServerScope::Project { project_id } => context
            .project_id
            .as_deref()
            .is_some_and(|active| active == project_id),
        // A plugin id alone does not prove that the active Agent run is owned by that plugin.
        McpServerScope::Plugin { .. } => false,
        _ => false,
    }
}

fn scope_matches_provenance(scope: &McpServerScope, provenance: &AgentMcpServerScope) -> bool {
    match (scope, provenance) {
        (McpServerScope::Builtin, AgentMcpServerScope::Builtin)
        | (McpServerScope::User, AgentMcpServerScope::User)
        | (McpServerScope::Managed, AgentMcpServerScope::Managed) => true,
        (
            McpServerScope::Project { project_id },
            AgentMcpServerScope::Project {
                project_id: provenance_project,
            },
        ) => project_id == provenance_project,
        (
            McpServerScope::Plugin { plugin_id },
            AgentMcpServerScope::Plugin {
                plugin_id: provenance_plugin,
            },
        ) => plugin_id == provenance_plugin,
        _ => false,
    }
}

fn map_scope(scope: &McpServerScope) -> Option<AgentMcpServerScope> {
    match scope {
        McpServerScope::Builtin => Some(AgentMcpServerScope::Builtin),
        McpServerScope::User => Some(AgentMcpServerScope::User),
        McpServerScope::Project { project_id } => Some(AgentMcpServerScope::Project {
            project_id: project_id.clone(),
        }),
        McpServerScope::Plugin { plugin_id } => Some(AgentMcpServerScope::Plugin {
            plugin_id: plugin_id.clone(),
        }),
        McpServerScope::Managed => Some(AgentMcpServerScope::Managed),
        _ => None,
    }
}

fn map_approval_mode(mode: McpApprovalMode) -> Option<AgentMcpApprovalMode> {
    match mode {
        McpApprovalMode::Prompt => Some(AgentMcpApprovalMode::Prompt),
        McpApprovalMode::Auto => Some(AgentMcpApprovalMode::Auto),
        McpApprovalMode::Deny => None,
        _ => None,
    }
}

fn approval_mode_matches(agent: AgentMcpApprovalMode, server: McpApprovalMode) -> bool {
    matches!(
        (agent, server),
        (AgentMcpApprovalMode::Prompt, McpApprovalMode::Prompt)
            | (AgentMcpApprovalMode::Auto, McpApprovalMode::Auto)
    )
}

fn map_annotations(annotations: Option<McpToolAnnotations>) -> McpAgentToolAnnotations {
    annotations.map_or_else(McpAgentToolAnnotations::default, |annotations| {
        McpAgentToolAnnotations {
            read_only_hint: annotations.read_only_hint,
            destructive_hint: annotations.destructive_hint,
            idempotent_hint: annotations.idempotent_hint,
            open_world_hint: annotations.open_world_hint,
        }
    })
}

fn map_tool_result(
    result: McpToolResult,
    limits: &McpRuntimeProjectionLimits,
) -> AgentResult<McpToolInvocationResult> {
    let mut content = Vec::new();
    let mut remaining_text_bytes = limits.max_model_text_bytes;
    let mut truncated_at_source = result.content.len() > limits.max_content_blocks;
    for block in result.content.into_iter().take(limits.max_content_blocks) {
        match block {
            McpContentBlock::Text { mut text } => {
                if text.len() > remaining_text_bytes {
                    truncate_string_bytes(&mut text, remaining_text_bytes);
                    truncated_at_source = true;
                }
                remaining_text_bytes = remaining_text_bytes.saturating_sub(text.len());
                content.push(McpToolContentBlock::Text { text });
            }
            block => content.push(map_content_block(block, limits)),
        }
    }
    let structured_content = match result.structured_content {
        Some(structured) if structured_content_within_limits(&structured, limits) => {
            Some(structured)
        }
        Some(_) => {
            truncated_at_source = true;
            Some(serde_json::json!({
                "_mycopilot": {
                    "omitted": true,
                    "reason": "mcp_bridge_structured_content_limit",
                }
            }))
        }
        None => None,
    };
    Ok(McpToolInvocationResult {
        content,
        structured_content,
        is_error: result.is_error,
        truncated_at_source,
    })
}

fn map_content_block(
    content: McpContentBlock,
    limits: &McpRuntimeProjectionLimits,
) -> McpToolContentBlock {
    match content {
        McpContentBlock::Text { mut text } => {
            truncate_string_bytes(&mut text, limits.max_model_text_bytes);
            McpToolContentBlock::Text { text }
        }
        McpContentBlock::Image { data, mime_type } => McpToolContentBlock::Omitted {
            kind: McpOmittedContentKind::Image,
            mime_type: safe_mime_type(Some(mime_type), limits),
            encoded_bytes: Some(saturating_u64(data.len())),
        },
        McpContentBlock::Audio { data, mime_type } => McpToolContentBlock::Omitted {
            kind: McpOmittedContentKind::Audio,
            mime_type: safe_mime_type(Some(mime_type), limits),
            encoded_bytes: Some(saturating_u64(data.len())),
        },
        McpContentBlock::EmbeddedResource { resource } => {
            let (mime_type, content_bytes) = match resource {
                McpEmbeddedResource::Text {
                    mime_type, text, ..
                } => (mime_type, text.len()),
                McpEmbeddedResource::Blob {
                    mime_type, data, ..
                } => (mime_type, data.len()),
            };
            McpToolContentBlock::Omitted {
                kind: McpOmittedContentKind::EmbeddedResource,
                mime_type: safe_mime_type(mime_type, limits),
                encoded_bytes: Some(saturating_u64(content_bytes)),
            }
        }
        McpContentBlock::ResourceLink { resource } => map_resource_link(resource, limits),
        _ => McpToolContentBlock::Omitted {
            kind: McpOmittedContentKind::EmbeddedResource,
            mime_type: None,
            encoded_bytes: None,
        },
    }
}

fn truncate_string_bytes(value: &mut String, max_bytes: usize) {
    if value.len() <= max_bytes {
        return;
    }
    let mut boundary = max_bytes.min(value.len());
    while boundary > 0 && !value.is_char_boundary(boundary) {
        boundary -= 1;
    }
    value.truncate(boundary);
}

fn structured_content_within_limits(
    value: &serde_json::Value,
    limits: &McpRuntimeProjectionLimits,
) -> bool {
    let mut pending = vec![(value, 1_usize)];
    let mut nodes = 0_usize;
    let mut estimated_bytes = 0_usize;
    while let Some((value, depth)) = pending.pop() {
        if depth > limits.max_model_structured_depth {
            return false;
        }
        let Some(next_nodes) = nodes.checked_add(1) else {
            return false;
        };
        nodes = next_nodes;
        if nodes > limits.max_model_structured_nodes {
            return false;
        }
        match value {
            serde_json::Value::Object(object) => {
                for (key, child) in object.iter().rev() {
                    let Some(next_bytes) = estimated_bytes.checked_add(key.len()) else {
                        return false;
                    };
                    estimated_bytes = next_bytes;
                    pending.push((child, depth.saturating_add(1)));
                }
            }
            serde_json::Value::Array(array) => pending.extend(
                array
                    .iter()
                    .rev()
                    .map(|child| (child, depth.saturating_add(1))),
            ),
            serde_json::Value::String(value) => {
                let Some(next_bytes) = estimated_bytes.checked_add(value.len()) else {
                    return false;
                };
                estimated_bytes = next_bytes;
            }
            serde_json::Value::Number(_) => {
                estimated_bytes = estimated_bytes.saturating_add(32);
            }
            serde_json::Value::Bool(_) | serde_json::Value::Null => {
                estimated_bytes = estimated_bytes.saturating_add(8);
            }
        }
        if estimated_bytes > limits.max_model_structured_bytes {
            return false;
        }
    }
    serde_json::to_vec(value)
        .is_ok_and(|encoded| encoded.len() <= limits.max_model_structured_bytes)
}

fn map_resource_link(
    resource: McpResourceLink,
    limits: &McpRuntimeProjectionLimits,
) -> McpToolContentBlock {
    McpToolContentBlock::Omitted {
        kind: McpOmittedContentKind::ResourceLink,
        mime_type: safe_mime_type(resource.mime_type, limits),
        encoded_bytes: resource.size,
    }
}

fn safe_mime_type(
    mime_type: Option<String>,
    limits: &McpRuntimeProjectionLimits,
) -> Option<String> {
    let value = mime_type?;
    if value.is_empty()
        || value.len() > limits.max_mime_type_bytes
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_graphic() && !matches!(byte, b'\"' | b'\\'))
    {
        return None;
    }
    Some(value)
}

fn saturating_u64(value: usize) -> u64 {
    u64::try_from(value).unwrap_or(u64::MAX)
}

fn approval_aad(approval: &AgentMcpToolApproval) -> McpApprovalPayloadAad {
    let identity = &approval.identity;
    let provenance = &identity.provenance;
    let raw_tool_name_digest = lower_sha256(provenance.raw_tool_name.as_bytes());
    McpApprovalPayloadAad {
        run_id: identity.run_id.clone(),
        action_id: identity.action_id.clone(),
        call_id: identity.call_id.clone(),
        server_id: provenance.server_id.clone(),
        config_epoch: provenance.config_epoch.clone(),
        registry_revision: provenance.registry_revision,
        config_digest: provenance.config_digest.clone(),
        catalog_digest: provenance.catalog_digest.clone(),
        catalog_generation: provenance.catalog_generation,
        raw_tool_name_digest,
        schema_digest: provenance.schema_digest.clone(),
        arguments_digest: identity.arguments_digest.clone(),
        payload_persistence: approval.payload_persistence,
        created_at_ms: approval.created_at,
        expires_at_ms: approval.expires_at,
    }
}

fn active_call_id(approval: &AgentMcpToolApproval) -> AgentResult<McpActiveCallId> {
    let server_id = McpServerId::from_str(&approval.identity.provenance.server_id)
        .map_err(|_| stale_tool_error("mcp.invalid_server_identity"))?;
    let invocation_id = McpInvocationId::from_str(&approval.identity.invocation_id)
        .map_err(|_| stale_tool_error("mcp.invalid_invocation_identity"))?;
    let model_call_id = McpModelCallId::from_str(&approval.identity.call_id)
        .map_err(|_| stale_tool_error("mcp.invalid_model_call_identity"))?;
    Ok(McpActiveCallId::new(
        server_id,
        invocation_id,
        model_call_id,
    ))
}

fn lower_sha256(value: &[u8]) -> String {
    let digest = Sha256::digest(value);
    let mut output = String::with_capacity(digest.len() * 2);
    for byte in digest {
        use std::fmt::Write as _;
        let _ = write!(output, "{byte:02x}");
    }
    output
}

fn map_payload_error(error: McpApprovalPayloadStoreError) -> AgentError {
    let (code, reason) = match error {
        McpApprovalPayloadStoreError::PayloadAlreadyExists => {
            ("mcp.approval_payload_replay", "payloadAlreadyExists")
        }
        McpApprovalPayloadStoreError::PayloadNotFound => {
            ("mcp.approval_payload_unavailable", "payloadNotFound")
        }
        McpApprovalPayloadStoreError::PayloadExpired => {
            ("mcp.approval_payload_expired", "payloadExpired")
        }
        McpApprovalPayloadStoreError::BindingMismatch
        | McpApprovalPayloadStoreError::AuthenticationFailed => {
            ("mcp.approval_payload_binding_invalid", "bindingInvalid")
        }
        McpApprovalPayloadStoreError::InvalidInvocationId
        | McpApprovalPayloadStoreError::InvalidMetadata
        | McpApprovalPayloadStoreError::InvalidPayload
        | McpApprovalPayloadStoreError::InvalidEnvelope => {
            ("mcp.approval_payload_invalid", "payloadInvalid")
        }
        McpApprovalPayloadStoreError::Unavailable
        | McpApprovalPayloadStoreError::CredentialUnavailable
        | McpApprovalPayloadStoreError::RepositoryUnavailable
        | McpApprovalPayloadStoreError::RandomnessUnavailable => {
            ("mcp.approval_payload_store_unavailable", "storeUnavailable")
        }
    };
    AgentError::structured(
        code,
        "The sealed MCP approval payload is unavailable or invalid.",
        serde_json::json!({
            "type": "mcp_approval",
            "code": reason,
            "retryable": false,
        }),
    )
}

fn policy_error(code: &'static str) -> AgentError {
    AgentError::structured(
        code,
        "The MCP server approval policy denies this invocation.",
        serde_json::json!({
            "type": "mcp_approval",
            "code": "approvalPolicyDenied",
            "retryable": false,
        }),
    )
}

fn map_catalog_error(_error: McpError) -> AgentError {
    AgentError::structured(
        "mcp.catalog_unavailable",
        "The MCP tool catalog is temporarily unavailable.",
        serde_json::json!({"retryable": true}),
    )
}

fn map_invocation_error(error: McpError) -> AgentError {
    let dispatch_certainty = error
        .dispatch_certainty
        .unwrap_or(McpDispatchCertainty::DefinitelyNotDispatched);
    if error.kind == McpErrorKind::OutcomeUnknown
        || !matches!(
            dispatch_certainty,
            McpDispatchCertainty::DefinitelyNotDispatched | McpDispatchCertainty::ResponseReceived
        )
    {
        return AgentError::structured(
            "mcp.tool_outcome_unknown",
            "The MCP request may have reached the server, but its outcome is unknown.",
            serde_json::json!({
                "type": "mcp_tool",
                "code": "outcomeUnknown",
                "retryable": false,
                "dispatchCertainty": dispatch_certainty_label(
                    McpDispatchCertainty::PossiblyDispatched
                ),
                "reason": safe_outcome_unknown_reason(error.outcome_unknown_reason),
            }),
        );
    }
    let dispatch_certainty = dispatch_certainty_label(dispatch_certainty);
    let retryable = dispatch_certainty == "definitely_not_dispatched";
    match error.kind {
        McpErrorKind::Cancelled => AgentError::cancelled(),
        McpErrorKind::OutputTooLarge => AgentError::structured(
            "mcp.tool_output_too_large",
            "The MCP server returned a Tool response that exceeded Host output limits.",
            serde_json::json!({
                "type": "mcp_tool",
                "code": "outputTooLarge",
                "retryable": false,
                "dispatchCertainty": dispatch_certainty,
            }),
        ),
        McpErrorKind::Timeout => AgentError::structured(
            "mcp.tool_timeout",
            "The MCP tool invocation timed out.",
            serde_json::json!({
                "retryable": retryable,
                "dispatchCertainty": dispatch_certainty,
            }),
        ),
        McpErrorKind::Config => AgentError::structured(
            "mcp.tool_snapshot_stale",
            "The MCP tool definition is stale or invalid; refresh the tool catalog and retry.",
            serde_json::json!({
                "retryable": retryable,
                "dispatchCertainty": dispatch_certainty,
            }),
        ),
        McpErrorKind::Capacity => AgentError::structured(
            "mcp.tool_capacity_exceeded",
            "The MCP tool invocation did not start because Host capacity is full.",
            serde_json::json!({
                "retryable": false,
                "dispatchCertainty": dispatch_certainty,
            }),
        ),
        McpErrorKind::Spawn
        | McpErrorKind::Negotiation
        | McpErrorKind::Protocol
        | McpErrorKind::ServerExited
        | McpErrorKind::Shutdown => AgentError::structured(
            "mcp.tool_unavailable",
            "The MCP tool is temporarily unavailable.",
            serde_json::json!({
                "retryable": retryable,
                "dispatchCertainty": dispatch_certainty,
            }),
        ),
        McpErrorKind::OutcomeUnknown => unreachable!("handled above"),
        _ => AgentError::structured(
            "mcp.tool_unavailable",
            "The MCP tool is temporarily unavailable.",
            serde_json::json!({
                "retryable": retryable,
                "dispatchCertainty": dispatch_certainty,
            }),
        ),
    }
}

fn dispatch_certainty_label(certainty: McpDispatchCertainty) -> &'static str {
    match certainty {
        McpDispatchCertainty::DefinitelyNotDispatched => "definitely_not_dispatched",
        McpDispatchCertainty::PossiblyDispatched => "possibly_dispatched",
        McpDispatchCertainty::ResponseReceived => "response_received",
        _ => "possibly_dispatched",
    }
}

fn safe_outcome_unknown_reason(
    reason: Option<mycopilot_mcp_client::McpOutcomeUnknownReason>,
) -> &'static str {
    use mycopilot_mcp_client::McpOutcomeUnknownReason;
    match reason {
        Some(McpOutcomeUnknownReason::Cancelled) => "cancelled",
        Some(McpOutcomeUnknownReason::TimedOut) => "timed_out",
        Some(McpOutcomeUnknownReason::Shutdown) => "shutdown",
        Some(McpOutcomeUnknownReason::ServerStopped) => "server_stopped",
        Some(McpOutcomeUnknownReason::ServerRestarted) => "server_restarted",
        Some(McpOutcomeUnknownReason::ServerRemoved) => "server_removed",
        Some(McpOutcomeUnknownReason::ServerExited) => "server_exited",
        Some(McpOutcomeUnknownReason::TransportClosed) => "transport_closed",
        Some(McpOutcomeUnknownReason::ProtocolFailure) => "protocol_failure",
        None => "unknown",
        Some(_) => "unknown",
    }
}

fn stale_tool_error(code: &'static str) -> AgentError {
    AgentError::structured(
        code,
        "The MCP tool definition is stale or invalid; refresh the tool catalog and retry.",
        serde_json::json!({
            "retryable": true,
            "dispatchCertainty": "definitely_not_dispatched",
        }),
    )
}

#[cfg(test)]
mod tests {
    use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
    use mycopilot_core::image_generation::InMemoryCredentialStore;
    use mycopilot_core::{
        mcp_tool_arguments_digest, send_chat_with_host_services, AgentApiStyle,
        AgentApprovalStatus, AgentChatInput, AgentChatMessage, AgentEventEmitter,
        AgentMcpArgumentSummary, AgentMcpToolApprovalSummary, AgentMcpToolRisk,
        AgentProposedAction, AgentRunContext, AgentRuntimeHostServices, AgentToolCall,
        ConversationTraceSnapshot, McpToolRuntime, ModelCapabilities,
    };
    use mycopilot_mcp_client::{
        BoxMcpFuture, InMemoryMcpRegistry, McpCapabilitySnapshot, McpConnectionState, McpConnector,
        McpImplementationInfo, McpLifecycleKind, McpManagerPolicy, McpPeer, McpProtocolSnapshot,
        McpRegistry, McpServerConfig, McpStdioConfig, McpToolCall, McpToolDescriptor, McpToolPage,
        McpTransportConfig,
    };
    use serde_json::json;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::{Duration, SystemTime, UNIX_EPOCH};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::{TcpListener, TcpStream};

    use super::*;
    use crate::application::mcp::approval_payload_store::{
        DurableMcpApprovalPayloadStore, InMemoryMcpApprovalEnvelopeRepository,
        UnavailableMcpApprovalPayloadStore,
    };

    async fn read_json_request(stream: &mut TcpStream) -> serde_json::Value {
        let mut request = Vec::new();
        let mut buffer = [0_u8; 4_096];
        let mut body_start = None;
        let mut expected_length = None;
        loop {
            let read = stream.read(&mut buffer).await.unwrap();
            assert!(read > 0, "fixture model connection closed early");
            request.extend_from_slice(&buffer[..read]);
            if body_start.is_none() {
                if let Some(header_end) = request.windows(4).position(|part| part == b"\r\n\r\n") {
                    let headers = String::from_utf8_lossy(&request[..header_end]);
                    let content_length = headers
                        .lines()
                        .find_map(|line| {
                            let (name, value) = line.split_once(':')?;
                            name.eq_ignore_ascii_case("content-length")
                                .then(|| value.trim().parse::<usize>().ok())
                                .flatten()
                        })
                        .unwrap_or_default();
                    body_start = Some(header_end + 4);
                    expected_length = Some(header_end + 4 + content_length);
                }
            }
            if expected_length.is_some_and(|length| request.len() >= length) {
                break;
            }
        }
        serde_json::from_slice(&request[body_start.unwrap()..expected_length.unwrap()]).unwrap()
    }

    async fn write_json_response(stream: &mut TcpStream, body: serde_json::Value) {
        let body = serde_json::to_vec(&body).unwrap();
        let headers = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        );
        stream.write_all(headers.as_bytes()).await.unwrap();
        stream.write_all(&body).await.unwrap();
    }

    struct OwnedFixturePeer {
        server_id: McpServerId,
        protocol: McpProtocolSnapshot,
        tool: McpToolDescriptor,
        result: Mutex<McpToolResult>,
        calls: Mutex<Vec<McpToolCall>>,
        wait_for_cancellation: AtomicBool,
        call_started: AtomicBool,
        cancellation_seen: AtomicBool,
    }

    impl OwnedFixturePeer {
        fn new(server_id: McpServerId) -> Self {
            Self {
                server_id,
                protocol: McpProtocolSnapshot {
                    negotiated_version: "2026-07-28".to_string(),
                    lifecycle: McpLifecycleKind::Discover,
                    server: Some(McpImplementationInfo {
                        name: "core-server-owned-fixture".to_string(),
                        version: "1.0.0".to_string(),
                    }),
                    capabilities: McpCapabilitySnapshot {
                        tools: true,
                        ..McpCapabilitySnapshot::default()
                    },
                },
                tool: McpToolDescriptor {
                    name: "raw/echo".to_string(),
                    title: None,
                    description: Some("Owned read-only fixture.".to_string()),
                    input_schema: json!({
                        "type": "object",
                        "properties": {"text": {"type": "string"}},
                        "required": ["text"]
                    }),
                    output_schema: Some(json!({"type": "object"})),
                    annotations: Some(McpToolAnnotations {
                        read_only_hint: Some(true),
                        ..McpToolAnnotations::default()
                    }),
                },
                result: Mutex::new(McpToolResult {
                    content: vec![
                        McpContentBlock::Text {
                            text: "fixture response".to_string(),
                        },
                        McpContentBlock::Image {
                            data: "encoded-image-secret".to_string(),
                            mime_type: "image/png".to_string(),
                        },
                    ],
                    structured_content: Some(json!({"sum": 3})),
                    is_error: false,
                }),
                calls: Mutex::new(Vec::new()),
                wait_for_cancellation: AtomicBool::new(false),
                call_started: AtomicBool::new(false),
                cancellation_seen: AtomicBool::new(false),
            }
        }
    }

    impl McpPeer for OwnedFixturePeer {
        fn server_id(&self) -> McpServerId {
            self.server_id
        }

        fn connection_state(&self) -> McpConnectionState {
            McpConnectionState::Ready
        }

        fn protocol_snapshot(&self) -> &McpProtocolSnapshot {
            &self.protocol
        }

        fn list_tools<'a>(&'a self, cursor: Option<String>) -> BoxMcpFuture<'a, McpToolPage> {
            Box::pin(async move {
                if cursor.is_some() {
                    return Err(McpError::protocol("unexpected owned fixture cursor"));
                }
                Ok(McpToolPage {
                    tools: vec![self.tool.clone()],
                    next_cursor: None,
                    ttl_ms: None,
                    cache_scope: None,
                })
            })
        }

        fn call_tool<'a>(
            &'a self,
            call: McpToolCall,
            cancellation: McpCancellationToken,
        ) -> BoxMcpFuture<'a, McpToolResult> {
            Box::pin(async move {
                self.call_started.store(true, Ordering::SeqCst);
                self.calls
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .push(call);
                if self.wait_for_cancellation.load(Ordering::SeqCst) {
                    cancellation.cancelled().await;
                    self.cancellation_seen.store(true, Ordering::SeqCst);
                    return Err(McpError::cancelled("owned fixture tools/call"));
                }
                Ok(self
                    .result
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .clone())
            })
        }

        fn close(&self) -> BoxMcpFuture<'_, ()> {
            Box::pin(async { Ok(()) })
        }
    }

    struct OwnedFixtureConnector {
        peer: Arc<OwnedFixturePeer>,
    }

    impl McpConnector for OwnedFixtureConnector {
        fn connect<'a>(
            &'a self,
            config: &'a McpServerConfig,
        ) -> BoxMcpFuture<'a, Arc<dyn McpPeer>> {
            Box::pin(async move {
                if config.id != self.peer.server_id {
                    return Err(McpError::config("owned fixture identity mismatch"));
                }
                Ok(Arc::clone(&self.peer) as Arc<dyn McpPeer>)
            })
        }
    }

    fn config_with_mode(server_id: McpServerId, approval_mode: McpApprovalMode) -> McpServerConfig {
        McpServerConfig {
            id: server_id,
            display_name: "owned fixture".to_string(),
            scope: McpServerScope::Project {
                project_id: "project-fixture".to_string(),
            },
            trust: McpTrustLevel::Managed,
            approval_mode,
            enabled: true,
            transport: McpTransportConfig::Stdio(McpStdioConfig {
                program: PathBuf::from("/owned/fixture"),
                arguments: Vec::new(),
                cwd: PathBuf::from("/owned"),
                environment: Vec::new(),
            }),
            connect_timeout_ms: 1_000,
            request_timeout_ms: 1_000,
            shutdown_timeout_ms: 1_000,
        }
    }

    fn project_context() -> McpToolCatalogContext {
        McpToolCatalogContext {
            project_id: Some("project-fixture".to_string()),
        }
    }

    async fn ready_bridge() -> (
        Arc<McpRuntimeBridge>,
        Arc<McpConnectionManager>,
        Arc<OwnedFixturePeer>,
    ) {
        ready_bridge_with_mode(McpApprovalMode::Prompt).await
    }

    async fn ready_bridge_with_mode(
        approval_mode: McpApprovalMode,
    ) -> (
        Arc<McpRuntimeBridge>,
        Arc<McpConnectionManager>,
        Arc<OwnedFixturePeer>,
    ) {
        let server_id = McpServerId::new();
        let registry = InMemoryMcpRegistry::shared();
        registry
            .add(config_with_mode(server_id, approval_mode))
            .unwrap();
        let peer = Arc::new(OwnedFixturePeer::new(server_id));
        let connector: Arc<dyn McpConnector> = Arc::new(OwnedFixtureConnector {
            peer: Arc::clone(&peer),
        });
        let manager = Arc::new(
            McpConnectionManager::without_events(registry, connector, McpManagerPolicy::default())
                .unwrap(),
        );
        manager.start(server_id).await.unwrap();
        (
            Arc::new(McpRuntimeBridge::new(Arc::clone(&manager))),
            manager,
            peer,
        )
    }

    fn seal_test_approval(
        bridge: &McpRuntimeBridge,
        provenance: AgentMcpToolProvenance,
        arguments: serde_json::Value,
        call_id: &str,
    ) -> AgentMcpToolApproval {
        seal_test_invocation(
            bridge,
            provenance,
            arguments,
            call_id,
            AgentMcpApprovalMode::Prompt,
            AgentApprovalStatus::Required,
        )
    }

    fn seal_test_invocation(
        bridge: &McpRuntimeBridge,
        provenance: AgentMcpToolProvenance,
        arguments: serde_json::Value,
        call_id: &str,
        approval_mode: AgentMcpApprovalMode,
        approval_status: AgentApprovalStatus,
    ) -> AgentMcpToolApproval {
        let call_id = format!("tc1_{}", URL_SAFE_NO_PAD.encode(Sha256::digest(call_id)));
        let created_at = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| i64::try_from(duration.as_millis()).unwrap_or(i64::MAX))
            .unwrap_or(1);
        let approval = AgentMcpToolApproval {
            identity: AgentMcpToolInvocationIdentity {
                action_id: uuid::Uuid::new_v4().to_string(),
                invocation_id: uuid::Uuid::new_v4().to_string(),
                run_id: "owned-adapter-test-run".to_string(),
                call_id: call_id.clone(),
                provenance: provenance.clone(),
                arguments_digest: mcp_tool_arguments_digest(&arguments).unwrap(),
            },
            call: AgentToolCall {
                id: call_id,
                tool: provenance.model_tool_name.clone(),
                args: json!({}),
                approval_status,
                reason: None,
            },
            summary: AgentMcpToolApprovalSummary {
                server_id: provenance.server_id.clone(),
                server_display_name: "owned fixture".to_string(),
                scope: provenance.scope.clone(),
                raw_tool_name: provenance.raw_tool_name.clone(),
                model_tool_name: provenance.model_tool_name.clone(),
                display_reason: None,
                arguments: AgentMcpArgumentSummary {
                    encoded_bytes: serde_json::to_vec(&arguments).unwrap().len() as u64,
                    top_level_property_count: arguments
                        .as_object()
                        .map_or(0, |object| object.len() as u64),
                    string_value_count: 0,
                    number_value_count: 0,
                    boolean_value_count: 0,
                    null_value_count: 0,
                    object_value_count: 1,
                    array_value_count: 0,
                    max_depth: 1,
                    truncated: false,
                },
                risk: AgentMcpToolRisk::ReadOnlyClaimed,
                external: true,
            },
            approval_mode,
            payload_persistence: AgentMcpApprovalPayloadPersistence::ProcessOnly,
            created_at,
            expires_at: created_at.saturating_add(60_000),
        };
        validate_mcp_approval_arguments(&approval, &arguments).unwrap();
        let invocation_id =
            McpApprovalInvocationId::parse(approval.identity.invocation_id.clone()).unwrap();
        let payload = McpApprovalPayload::from_json(&arguments).unwrap();
        bridge
            .payloads
            .seal(&invocation_id, approval_aad(&approval), payload)
            .unwrap();
        approval
    }

    struct ClosedRegistrySecurityGate;

    impl McpRegistrySecurityGate for ClosedRegistrySecurityGate {
        fn ensure_reconciled(&self) -> Result<(), String> {
            Err("fixed test gate is closed".to_string())
        }
    }

    #[tokio::test]
    async fn frozen_payload_persistence_must_match_the_current_backend() {
        let (process_bridge, manager, _) = ready_bridge().await;
        let descriptor = process_bridge
            .catalog(&project_context())
            .unwrap()
            .remove(0);
        let process_approval = seal_test_approval(
            &process_bridge,
            descriptor.provenance,
            json!({"text": "backend-binding"}),
            "payload-backend-binding",
        );
        assert_eq!(
            process_bridge.current_payload_persistence().unwrap(),
            AgentMcpApprovalPayloadPersistence::ProcessOnly
        );
        process_bridge
            .revalidate_approved(&process_approval)
            .expect("a process-only approval remains valid in its originating process");
        assert_eq!(
            process_bridge.inspect_startup_payload(&process_approval),
            McpApprovalStartupPayloadState::Unavailable,
            "process-only payloads are never restart-recoverable"
        );

        let mut forged_durable = process_approval.clone();
        forged_durable.payload_persistence =
            AgentMcpApprovalPayloadPersistence::DurableAuthenticatedEnvelope;
        assert_eq!(
            process_bridge
                .revalidate_approved(&forged_durable)
                .unwrap_err()
                .code(),
            Some("mcp.approval_payload_binding_changed")
        );

        let repository = Arc::new(InMemoryMcpApprovalEnvelopeRepository::default());
        let credentials = Arc::new(InMemoryCredentialStore::default());
        let (durable_store, _) =
            DurableMcpApprovalPayloadStore::provision(repository, credentials).unwrap();
        let durable_bridge =
            McpRuntimeBridge::with_payload_store(Arc::clone(&manager), Arc::new(durable_store));
        assert_eq!(
            durable_bridge.current_payload_persistence().unwrap(),
            AgentMcpApprovalPayloadPersistence::DurableAuthenticatedEnvelope
        );
        assert_eq!(
            durable_bridge
                .revalidate_approved(&process_approval)
                .unwrap_err()
                .code(),
            Some("mcp.approval_payload_binding_changed")
        );
        assert_eq!(
            durable_bridge.inspect_startup_payload(&process_approval),
            McpApprovalStartupPayloadState::Unavailable
        );
        assert_eq!(
            durable_bridge.inspect_startup_payload(&forged_durable),
            McpApprovalStartupPayloadState::Unavailable,
            "a durable marker alone cannot recover a payload from another backend"
        );

        let unavailable_bridge = McpRuntimeBridge::with_payload_store(
            Arc::clone(&manager),
            Arc::new(UnavailableMcpApprovalPayloadStore),
        );
        assert_eq!(
            unavailable_bridge
                .current_payload_persistence()
                .unwrap_err()
                .code(),
            Some("mcp.approval_payload_unavailable")
        );

        let _ = manager.stop_all().await;
    }

    #[tokio::test]
    async fn closed_registry_security_gate_blocks_catalog_and_dispatch_without_consuming_payload() {
        let (healthy_bridge, manager, peer) = ready_bridge().await;
        let descriptor = healthy_bridge
            .catalog(&project_context())
            .unwrap()
            .remove(0);
        let payloads: Arc<dyn McpApprovalPayloadStore> =
            Arc::new(InMemoryMcpApprovalPayloadStore::default());
        let blocked_bridge =
            McpRuntimeBridge::with_payload_store_projection_limits_and_security_gate(
                Arc::clone(&manager),
                Arc::clone(&payloads),
                McpRuntimeProjectionLimits::default(),
                Arc::new(ClosedRegistrySecurityGate),
            )
            .unwrap();
        let approval = seal_test_approval(
            &blocked_bridge,
            descriptor.provenance,
            json!({"text": "must-not-dispatch"}),
            "closed-registry-gate",
        );
        let invocation_id =
            McpApprovalInvocationId::parse(approval.identity.invocation_id.clone()).unwrap();
        let aad = approval_aad(&approval);

        let catalog_error = blocked_bridge
            .catalog(&project_context())
            .expect_err("a closed Registry security gate must hide the external catalog");
        assert_eq!(
            catalog_error.code(),
            Some("mcp.registry_reconciliation_required")
        );
        let dispatch_error = blocked_bridge
            .invoke_approved(
                McpApprovedToolInvocation {
                    approval: approval.clone(),
                },
                AgentCancellationToken::new(),
            )
            .await
            .expect_err("a closed Registry security gate must reject dispatch");
        assert_eq!(
            dispatch_error.code(),
            Some("mcp.registry_reconciliation_required")
        );
        assert!(
            payloads.load(&invocation_id, &aad).is_ok(),
            "fail-closed denial must occur before one-time payload consumption"
        );
        assert!(peer
            .calls
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .is_empty());
        manager.stop_all().await;
    }

    #[tokio::test]
    async fn bridge_catalog_budget_fails_before_unbounded_cross_server_aggregation() {
        let (_bridge, manager, _peer) = ready_bridge().await;
        let server_id = manager.list_statuses().unwrap()[0].server_id;
        let tool = manager.catalog(server_id).unwrap().unwrap().tools[0].clone();
        let mut tool_count = 0;
        let mut bytes = 0;
        let limits = McpRuntimeProjectionLimits::default();
        for _ in 0..limits.max_tool_definitions {
            reserve_catalog_budget(&tool, &mut tool_count, &mut bytes, &limits).unwrap();
        }
        let error =
            reserve_catalog_budget(&tool, &mut tool_count, &mut bytes, &limits).unwrap_err();
        assert_eq!(error.code(), Some("mcp.catalog_budget_exceeded"));

        let mut oversized = tool;
        oversized.descriptor.input_schema = json!({
            "type": "object",
            "description": "x".repeat(limits.max_catalog_bytes + 1),
        });
        let error = reserve_catalog_budget(&oversized, &mut 0, &mut 0, &limits).unwrap_err();
        assert_eq!(error.code(), Some("mcp.catalog_budget_exceeded"));
        manager.stop_all().await;
    }

    #[tokio::test]
    async fn ready_catalog_routes_by_typed_raw_identity_and_omits_binary_payload() {
        let (bridge, manager, peer) = ready_bridge().await;
        let catalog = bridge.catalog(&project_context()).unwrap();
        assert_eq!(catalog.len(), 1);
        let descriptor = catalog.into_iter().next().unwrap();
        assert_eq!(descriptor.approval_mode, AgentMcpApprovalMode::Prompt);
        assert_eq!(descriptor.provenance.raw_tool_name, "raw/echo");
        assert_eq!(
            descriptor.provenance.model_tool_name,
            "mcp__owned_fixture__raw_echo"
        );
        let status = manager.list_statuses().unwrap().remove(0);
        assert_eq!(
            descriptor.provenance.config_epoch,
            status.config_epoch.to_string()
        );
        assert_eq!(
            descriptor.provenance.registry_revision,
            status.registry_revision
        );
        assert_eq!(
            descriptor.provenance.scope,
            AgentMcpServerScope::Project {
                project_id: "project-fixture".to_string()
            }
        );
        assert_eq!(descriptor.annotations.read_only_hint, Some(true));
        assert!(descriptor.output_schema.is_some());

        let approval = seal_test_approval(
            &bridge,
            descriptor.provenance,
            json!({"text": "hello"}),
            "provider-adapter-call-1",
        );
        let result = bridge
            .invoke_approved(
                McpApprovedToolInvocation { approval },
                AgentCancellationToken::new(),
            )
            .await
            .unwrap();
        assert_eq!(result.structured_content, Some(json!({"sum": 3})));
        assert_eq!(
            result.content,
            vec![
                McpToolContentBlock::Text {
                    text: "fixture response".to_string()
                },
                McpToolContentBlock::Omitted {
                    kind: McpOmittedContentKind::Image,
                    mime_type: Some("image/png".to_string()),
                    encoded_bytes: Some("encoded-image-secret".len() as u64)
                }
            ]
        );
        {
            let calls = peer.calls.lock().unwrap_or_else(|error| error.into_inner());
            assert_eq!(calls.len(), 1);
            assert_eq!(calls[0].name, "raw/echo");
            assert_eq!(calls[0].arguments, json!({"text": "hello"}));
        }
        let _ = manager.stop_all().await;
    }

    #[tokio::test]
    async fn auto_catalog_is_visible_and_revalidation_requires_the_exact_policy() {
        let (bridge, manager, peer) = ready_bridge_with_mode(McpApprovalMode::Auto).await;
        let descriptor = bridge.catalog(&project_context()).unwrap().remove(0);
        assert_eq!(descriptor.approval_mode, AgentMcpApprovalMode::Auto);

        let approval = seal_test_invocation(
            &bridge,
            descriptor.provenance,
            json!({"text": "automatic"}),
            "provider-adapter-auto-call",
            AgentMcpApprovalMode::Auto,
            AgentApprovalStatus::Approved,
        );
        bridge
            .revalidate_approved(&approval)
            .expect("an exact automatic policy snapshot must remain routable");
        let result = bridge
            .invoke_approved(
                McpApprovedToolInvocation {
                    approval: approval.clone(),
                },
                AgentCancellationToken::new(),
            )
            .await
            .expect("automatic policy must use the same sealed one-time invocation path");
        assert!(!result.is_error);

        let mut stale_prompt = approval.clone();
        stale_prompt.approval_mode = AgentMcpApprovalMode::Prompt;
        stale_prompt.call.approval_status = AgentApprovalStatus::Required;
        assert_eq!(
            bridge
                .revalidate_approved(&stale_prompt)
                .unwrap_err()
                .code(),
            Some("mcp.approval_snapshot_stale")
        );
        assert_eq!(
            peer.calls
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .len(),
            1
        );
        assert_eq!(
            bridge
                .invoke_approved(
                    McpApprovedToolInvocation { approval },
                    AgentCancellationToken::new(),
                )
                .await
                .expect_err("an automatically dispatched payload must not replay")
                .code(),
            Some("mcp.approval_payload_unavailable")
        );
        let _ = manager.stop_all().await;
    }

    #[tokio::test]
    async fn final_revalidation_checks_raw_and_normalized_schema_identities_independently() {
        let (bridge, manager, peer) = ready_bridge().await;
        let descriptor = bridge.catalog(&project_context()).unwrap().remove(0);
        assert_ne!(
            descriptor.provenance.catalog_schema_digest,
            descriptor.provenance.schema_digest
        );
        let approval = seal_test_approval(
            &bridge,
            descriptor.provenance,
            json!({"text": "identity"}),
            "provider-adapter-schema-identity",
        );
        bridge.revalidate_approved(&approval).unwrap();

        let mut raw_drift = approval.clone();
        raw_drift.identity.provenance.catalog_schema_digest = "f".repeat(64);
        assert_eq!(
            bridge.revalidate_approved(&raw_drift).unwrap_err().code(),
            Some("mcp.approval_tool_drift")
        );

        let mut normalized_drift = approval.clone();
        normalized_drift.identity.provenance.schema_digest = "e".repeat(64);
        assert_eq!(
            bridge
                .revalidate_approved(&normalized_drift)
                .unwrap_err()
                .code(),
            Some("mcp.approval_tool_schema_drift")
        );

        let mut epoch_drift = approval.clone();
        epoch_drift.identity.provenance.config_epoch = uuid::Uuid::new_v4().to_string();
        assert_eq!(
            bridge.revalidate_approved(&epoch_drift).unwrap_err().code(),
            Some("mcp.approval_snapshot_stale")
        );

        let mut revision_drift = approval.clone();
        revision_drift.identity.provenance.registry_revision += 1;
        assert_eq!(
            bridge
                .revalidate_approved(&revision_drift)
                .unwrap_err()
                .code(),
            Some("mcp.approval_snapshot_stale")
        );

        let mut normalizer_drift = approval;
        normalizer_drift
            .identity
            .provenance
            .schema_normalizer_version += 1;
        assert_eq!(
            bridge
                .revalidate_approved(&normalizer_drift)
                .unwrap_err()
                .code(),
            Some("mcp.approval_tool_schema_drift")
        );
        assert!(
            peer.calls
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .is_empty(),
            "schema identity revalidation must never dispatch"
        );
        let _ = manager.stop_all().await;
    }

    #[tokio::test]
    async fn server_scope_is_revalidated_independently_of_connection_trust() {
        let (bridge, manager, _) = ready_bridge().await;
        assert!(bridge
            .catalog(&McpToolCatalogContext::default())
            .unwrap()
            .is_empty());
        assert!(bridge
            .catalog(&McpToolCatalogContext {
                project_id: Some("different-project".to_string()),
            })
            .unwrap()
            .is_empty());
        assert_eq!(bridge.catalog(&project_context()).unwrap().len(), 1);
        let _ = manager.stop_all().await;
    }

    #[tokio::test]
    async fn agent_cancellation_is_forwarded_to_mcp_call() {
        let (bridge, manager, peer) = ready_bridge().await;
        peer.wait_for_cancellation.store(true, Ordering::SeqCst);
        let descriptor = bridge.catalog(&project_context()).unwrap().remove(0);
        let approval = seal_test_approval(
            &bridge,
            descriptor.provenance,
            json!({"text": "cancel"}),
            "provider-adapter-cancel-call",
        );
        let cancellation = AgentCancellationToken::new();
        let trigger = cancellation.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(10)).await;
            trigger.cancel();
        });
        let error = bridge
            .invoke_approved(McpApprovedToolInvocation { approval }, cancellation)
            .await
            .unwrap_err();
        assert_eq!(error.code(), Some("mcp.tool_outcome_unknown"));
        assert!(peer.cancellation_seen.load(Ordering::SeqCst));
        let _ = manager.stop_all().await;
    }

    #[test]
    fn authoritative_oversized_response_is_non_retryable_not_outcome_unknown() {
        let error = map_invocation_error(McpError::output_too_large(
            "MCP tools/call",
            "fixture response exceeded limit",
        ));
        assert_eq!(error.code(), Some("mcp.tool_output_too_large"));
        assert_eq!(
            error
                .details()
                .and_then(|details| details.get("retryable"))
                .and_then(serde_json::Value::as_bool),
            Some(false)
        );
    }

    #[test]
    fn host_capacity_is_not_misreported_as_a_stale_tool_snapshot() {
        let error = map_invocation_error(McpError::capacity(
            "untrusted internal active-call capacity detail",
        ));
        assert_eq!(error.code(), Some("mcp.tool_capacity_exceeded"));
        assert_eq!(
            error
                .details()
                .and_then(|details| details.get("retryable"))
                .and_then(serde_json::Value::as_bool),
            Some(false)
        );
        assert_eq!(
            error
                .details()
                .and_then(|details| details.get("dispatchCertainty"))
                .and_then(serde_json::Value::as_str),
            Some("definitely_not_dispatched")
        );
    }

    #[test]
    fn invocation_error_projection_preserves_safe_dispatch_certainty() {
        let possibly_dispatched = map_invocation_error(
            McpError::protocol("untrusted fixture diagnostic")
                .with_dispatch_certainty(McpDispatchCertainty::PossiblyDispatched),
        );
        assert_eq!(possibly_dispatched.code(), Some("mcp.tool_outcome_unknown"));
        assert_eq!(
            possibly_dispatched
                .details()
                .and_then(|details| details.get("dispatchCertainty"))
                .and_then(serde_json::Value::as_str),
            Some("possibly_dispatched")
        );
        assert_eq!(
            possibly_dispatched
                .details()
                .and_then(|details| details.get("retryable"))
                .and_then(serde_json::Value::as_bool),
            Some(false)
        );

        let response_received = map_invocation_error(
            McpError::protocol("another untrusted fixture diagnostic")
                .with_dispatch_certainty(McpDispatchCertainty::ResponseReceived),
        );
        assert_eq!(response_received.code(), Some("mcp.tool_unavailable"));
        assert_eq!(
            response_received
                .details()
                .and_then(|details| details.get("dispatchCertainty"))
                .and_then(serde_json::Value::as_str),
            Some("response_received")
        );
        assert_eq!(
            response_received
                .details()
                .and_then(|details| details.get("retryable"))
                .and_then(serde_json::Value::as_bool),
            Some(false)
        );
    }

    #[test]
    fn bridge_rejects_deep_structured_content_before_recursive_encoding() {
        let limits = McpRuntimeProjectionLimits::default();
        let mut value = serde_json::Value::Null;
        for _ in 0..limits.max_model_structured_depth {
            value = serde_json::json!({"nested": value});
        }
        assert!(!structured_content_within_limits(
            &serde_json::json!({"root": value}),
            &limits,
        ));
    }

    #[test]
    fn bridge_result_projection_uses_the_host_owned_limits() {
        let limits = McpRuntimeProjectionLimits {
            max_model_text_bytes: 4,
            max_content_blocks: 1,
            ..McpRuntimeProjectionLimits::default()
        };
        limits.validate().unwrap();

        let mapped = map_tool_result(
            McpToolResult {
                content: vec![
                    McpContentBlock::Text {
                        text: "abcdef".to_string(),
                    },
                    McpContentBlock::Text {
                        text: "not retained".to_string(),
                    },
                ],
                structured_content: None,
                is_error: false,
            },
            &limits,
        )
        .unwrap();
        assert_eq!(
            mapped.content,
            vec![McpToolContentBlock::Text {
                text: "abcd".to_string(),
            }]
        );
        assert!(mapped.truncated_at_source);
    }

    #[tokio::test]
    async fn sealed_approval_payload_is_consumed_at_most_once() {
        let (bridge, manager, peer) = ready_bridge().await;
        let descriptor = bridge.catalog(&project_context()).unwrap().remove(0);
        let approval = seal_test_approval(
            &bridge,
            descriptor.provenance,
            json!({"text": "once"}),
            "provider-adapter-once-call",
        );
        bridge
            .invoke_approved(
                McpApprovedToolInvocation {
                    approval: approval.clone(),
                },
                AgentCancellationToken::new(),
            )
            .await
            .unwrap();

        let error = bridge
            .invoke_approved(
                McpApprovedToolInvocation { approval },
                AgentCancellationToken::new(),
            )
            .await
            .expect_err("a consumed approval payload must not replay");
        assert_eq!(error.code(), Some("mcp.approval_payload_unavailable"));
        assert_eq!(
            peer.calls
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .len(),
            1
        );
        let _ = manager.stop_all().await;
    }

    #[tokio::test]
    async fn agent_runtime_calls_owned_fixture_through_catalog_bridge_end_to_end() {
        const MODEL_PRIVATE_ARGUMENT: &str = "fixture-private-token-never-persist";
        const MCP_PRIVATE_RESULT: &str = "fixture-private-result-never-persist";
        let (bridge, manager, peer) = ready_bridge().await;
        peer.result
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .content
            .push(McpContentBlock::Text {
                text: MCP_PRIVATE_RESULT.to_string(),
            });
        let descriptor = bridge.catalog(&project_context()).unwrap().remove(0);
        let model_name = descriptor.provenance.model_tool_name.clone();
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let requests = Arc::new(Mutex::new(Vec::<serde_json::Value>::new()));
        let requests_for_server = Arc::clone(&requests);
        let response_model_name = model_name.clone();
        let server = tokio::spawn(async move {
            for _ in 0..1 {
                let (mut stream, _) = listener.accept().await.unwrap();
                let request = read_json_request(&mut stream).await;
                requests_for_server
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .push(request);
                let response = json!({
                    "choices": [{
                        "message": {
                            "role": "assistant",
                            "content": null,
                            "tool_calls": [{
                                "id": "provider-fixture-call",
                                "type": "function",
                                "function": {
                                    "name": response_model_name,
                                    "arguments": format!(
                                        "{{\"text\":\"runtime request\",\"api_token\":\"{}\"}}",
                                        MODEL_PRIVATE_ARGUMENT
                                    )
                                }
                            }]
                        },
                        "finish_reason": "tool_calls"
                    }]
                });
                write_json_response(&mut stream, response).await;
            }
        });

        let provider_profile_config = mycopilot_core::ProviderProfileConfig::generic_for_dialect(
            mycopilot_core::ProviderProtocolDialect::OpenAiChatCompletions,
        );
        let provider_configuration_revision =
            Some(format!("provider-protocol-v1:{}", uuid::Uuid::new_v4()));
        let provider_protocol_key = mycopilot_core::ProviderProtocolKey::new(
            mycopilot_core::ProviderProtocolDialect::OpenAiChatCompletions,
            &provider_profile_config,
            "owned-fixture-model",
            provider_configuration_revision.clone(),
        )
        .unwrap();

        let input = AgentChatInput {
            api_url: format!("http://{address}/v1/chat/completions"),
            api_token: "owned-fixture-token".to_string(),
            provider_configuration_revision,
            provider_connection_revision: None,
            search_connection_revision: None,
            provider_profile_config: Some(provider_profile_config),
            provider_protocol_key: Some(provider_protocol_key),
            model: "owned-fixture-model".to_string(),
            model_capabilities: ModelCapabilities::default(),
            api_style: Some(AgentApiStyle::OpenAiCompatible),
            context_window_tokens: Some(128_000),
            context_window_indicator_enabled: false,
            max_tokens: Some(4_096),
            temperature: None,
            stream: Some(false),
            context: Some(AgentRunContext {
                collaboration_identity: None,
                conversation_id: Some("conversation-mcp-e2e".to_string()),
                project_id: Some("project-fixture".to_string()),
                workspace: None,
                attachment_library: None,
                permissions: Default::default(),
            }),
            search_config: None,
            prompt_preferences: None,
            approval_decision: None,
            tool_continuation: None,
            attachments: Vec::new(),
            resume_checkpoint: None,
            assistant_message_id: Some("assistant-mcp-e2e".to_string()),
            context_compaction_summary: None,
            world_state_records: Vec::new(),
            skill_activation: None,
            skill_discovery: None,
            messages: vec![AgentChatMessage {
                message_id: Some("user-mcp-e2e".to_string()),
                role: "user".to_string(),
                content: "Call the owned MCP fixture.".to_string(),
                created_at: Some(1),
                conversation_turn_trace: None,
                conversation_model_context_items: Vec::new(),
            }],
        };
        let invoker: Arc<dyn McpToolInvoker> = bridge.clone();
        let trace_snapshots = Arc::new(Mutex::new(Vec::<ConversationTraceSnapshot>::new()));
        let trace_snapshots_for_observer = Arc::clone(&trace_snapshots);
        let host_services = AgentRuntimeHostServices::new()
            .with_mcp_tools(McpToolRuntime::capture_for_context(
                invoker,
                project_context(),
            ))
            .with_trace_observer(Arc::new(move |snapshot| {
                trace_snapshots_for_observer
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .push(snapshot);
                Ok(None)
            }));
        let emitter: AgentEventEmitter = Arc::new(|_| {});
        let output = send_chat_with_host_services(
            input,
            "run-mcp-e2e".to_string(),
            emitter,
            AgentCancellationToken::new(),
            host_services,
        )
        .await
        .unwrap();
        server.await.unwrap();

        assert_eq!(
            output.status,
            mycopilot_core::AgentRunStatus::WaitingForApproval
        );
        {
            let requests = requests.lock().unwrap_or_else(|error| error.into_inner());
            assert_eq!(requests.len(), 1);
            let projected_tool = requests[0]["tools"]
                .as_array()
                .unwrap()
                .iter()
                .find(|tool| tool["function"]["name"] == model_name)
                .expect("friendly MCP tool must enter the Provider payload");
            let projected_description = projected_tool["function"]["description"]
                .as_str()
                .expect("MCP tool description");
            assert!(projected_description.contains("MCP server: \"owned fixture\""));
            assert!(projected_description.contains("MCP tool: \"raw/echo\""));
            assert!(!projected_description.contains("Treat the following"));
            let system_prompt = requests[0]["messages"][0]["content"]
                .as_str()
                .expect("stable system prompt");
            assert_eq!(
                system_prompt.matches("动态 MCP 工具的服务器标签").count(),
                1
            );
        }
        assert!(
            peer.calls
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .is_empty(),
            "Prompt mode must stop at a prepared approval before dispatch"
        );
        let approval = match output.proposed_actions.as_slice() {
            [AgentProposedAction::McpToolCall { approval }] => approval.as_ref().clone(),
            actions => panic!("expected one typed MCP approval, got {actions:?}"),
        };
        assert!(approval
            .call
            .args
            .as_object()
            .is_some_and(|args| args.is_empty()));
        let invocation_id =
            McpApprovalInvocationId::parse(approval.identity.invocation_id.clone()).unwrap();
        let sealed = bridge
            .payloads
            .load(&invocation_id, &approval_aad(&approval))
            .expect("prepare_approval seals the transient argument payload");
        assert!(!format!("{sealed:?}").contains(MODEL_PRIVATE_ARGUMENT));
        sealed
            .with_json(|arguments| {
                assert_eq!(arguments["api_token"], MODEL_PRIVATE_ARGUMENT);
            })
            .unwrap();

        let durable_output = serde_json::to_string(&output).unwrap();
        assert!(!durable_output.contains(MODEL_PRIVATE_ARGUMENT));
        assert!(
            !durable_output.contains(MCP_PRIVATE_RESULT),
            "no MCP output exists before the approved invocation"
        );
        {
            let trace_snapshots = trace_snapshots
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            let serialized_snapshots = serde_json::to_string(
                &trace_snapshots
                    .iter()
                    .map(|snapshot| &snapshot.model_context_items)
                    .collect::<Vec<_>>(),
            )
            .unwrap();
            assert!(!serialized_snapshots.contains(MODEL_PRIVATE_ARGUMENT));
            assert!(!serialized_snapshots.contains(MCP_PRIVATE_RESULT));
        }
        let _ = manager.stop_all().await;
    }

    #[test]
    fn resource_uri_and_binary_content_are_not_retained() {
        let mapped = map_content_block(
            McpContentBlock::EmbeddedResource {
                resource: McpEmbeddedResource::Blob {
                    uri: "https://secret.invalid/private".to_string(),
                    mime_type: Some("application/octet-stream".to_string()),
                    data: "private-base64".to_string(),
                },
            },
            &McpRuntimeProjectionLimits::default(),
        );
        assert_eq!(
            mapped,
            McpToolContentBlock::Omitted {
                kind: McpOmittedContentKind::EmbeddedResource,
                mime_type: Some("application/octet-stream".to_string()),
                encoded_bytes: Some("private-base64".len() as u64),
            }
        );
        let rendered = format!("{mapped:?}");
        assert!(!rendered.contains("secret.invalid"));
        assert!(!rendered.contains("private-base64"));
    }

    #[test]
    fn diagnostics_are_bounded_and_replace_the_previous_snapshot() {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let bridge = runtime.block_on(async {
            let registry = InMemoryMcpRegistry::shared();
            let connector: Arc<dyn McpConnector> = Arc::new(OwnedFixtureConnector {
                peer: Arc::new(OwnedFixturePeer::new(McpServerId::new())),
            });
            McpRuntimeBridge::new(Arc::new(
                McpConnectionManager::without_events(
                    registry,
                    connector,
                    McpManagerPolicy::default(),
                )
                .unwrap(),
            ))
        });
        let diagnostic = McpToolRegistrationDiagnostic {
            server_id: None,
            model_tool_name: None,
            code: mycopilot_core::McpToolDiagnosticCode::CatalogUnavailable,
            message: "safe diagnostic".to_string(),
        };
        bridge.report_diagnostics(&vec![diagnostic; MAX_RETAINED_DIAGNOSTICS + 10]);
        assert_eq!(bridge.diagnostics().len(), MAX_RETAINED_DIAGNOSTICS);
    }
}
