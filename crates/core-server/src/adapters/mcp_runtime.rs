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

pub(crate) fn map_tool_result(
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

pub(crate) fn map_invocation_error(error: McpError) -> AgentError {
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
mod tests;
