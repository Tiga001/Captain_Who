use std::collections::BTreeSet;
use std::str::FromStr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use mycopilot_core::{
    AgentCancellationToken, AgentError, AgentMcpServerScope, AgentMcpToolProvenance, AgentResult,
    McpAgentToolAnnotations, McpAgentToolDescriptor, McpOmittedContentKind, McpToolCatalogContext,
    McpToolContentBlock, McpToolInvocation, McpToolInvocationFuture, McpToolInvocationResult,
    McpToolInvoker, McpToolRegistrationDiagnostic, MCP_RUNTIME_MAX_CATALOG_BYTES,
    MCP_RUNTIME_MAX_TOOL_DEFINITIONS,
};
use mycopilot_mcp_client::{
    McpCancellationToken, McpCatalogCompleteness, McpCatalogDigest, McpCatalogTool,
    McpCatalogToolCall, McpConfigDigest, McpConnectionManager, McpContentBlock,
    McpEmbeddedResource, McpError, McpErrorKind, McpResourceLink, McpServerId, McpServerScope,
    McpServerState, McpToolAnnotations, McpToolId, McpToolResult, McpTrustLevel,
};

const MAX_MIME_TYPE_BYTES: usize = 128;
const MAX_RETAINED_DIAGNOSTICS: usize = 1_024;
const MAX_MCP_BRIDGE_CONTENT_BLOCKS: usize = 128;
const MAX_MCP_BRIDGE_TEXT_BYTES: usize = 16 * 1_024;
const MAX_MCP_BRIDGE_STRUCTURED_BYTES: usize = 8 * 1_024;
const MAX_MCP_BRIDGE_STRUCTURED_DEPTH: usize = 32;
const MAX_MCP_BRIDGE_STRUCTURED_NODES: usize = 4_096;
const MCP_CANCELLATION_SETTLE_GRACE: Duration = Duration::from_millis(250);

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct McpAuthorizedToolSnapshot {
    server_id: String,
    raw_tool_name: String,
    config_digest: String,
    catalog_generation: u64,
    catalog_digest: String,
}

impl McpAuthorizedToolSnapshot {
    fn from_provenance(provenance: &AgentMcpToolProvenance) -> Self {
        Self {
            server_id: provenance.server_id.clone(),
            raw_tool_name: provenance.raw_tool_name.clone(),
            config_digest: provenance.config_digest.clone(),
            catalog_generation: provenance.catalog_generation,
            catalog_digest: provenance.catalog_digest.clone(),
        }
    }
}

/// Host-owned invoke authorization. Connection trust and Server annotations never populate this
/// policy implicitly.
#[derive(Clone, Debug, Default)]
pub(crate) struct McpToolAuthorizationPolicy {
    unapproved_read_only: BTreeSet<McpAuthorizedToolSnapshot>,
}

impl McpToolAuthorizationPolicy {
    pub(crate) fn deny_all() -> Self {
        Self::default()
    }

    /// Add one Host-authorized immutable catalog snapshot.
    ///
    /// This is deliberately not inferred from Server annotations or connection trust. The future
    /// settings/approval layer can construct this policy from its own authorization records; the
    /// current production bootstrap keeps the empty deny-all policy.
    #[allow(dead_code)] // Production grant loading arrives with the persisted authorization round.
    pub(crate) fn allow_unapproved_read_only(
        mut self,
        provenance: &AgentMcpToolProvenance,
    ) -> Self {
        self.unapproved_read_only
            .insert(McpAuthorizedToolSnapshot::from_provenance(provenance));
        self
    }

    fn allows_unapproved_read_only(&self, provenance: &AgentMcpToolProvenance) -> bool {
        self.unapproved_read_only
            .contains(&McpAuthorizedToolSnapshot::from_provenance(provenance))
    }
}

/// Process-owned adapter from the MCP protocol subsystem into the Agent Runtime's stable,
/// protocol-neutral invocation boundary.
///
/// The bridge owns no MCP connection. Every invocation is revalidated by the shared Connection
/// Manager against the immutable provenance captured for the current Agent run.
pub(crate) struct McpRuntimeBridge {
    manager: Arc<McpConnectionManager>,
    authorization: Arc<McpToolAuthorizationPolicy>,
    diagnostics: Mutex<Vec<McpToolRegistrationDiagnostic>>,
}

impl McpRuntimeBridge {
    pub(crate) fn new(manager: Arc<McpConnectionManager>) -> Self {
        Self::with_authorization(manager, Arc::new(McpToolAuthorizationPolicy::deny_all()))
    }

    pub(crate) fn with_authorization(
        manager: Arc<McpConnectionManager>,
        authorization: Arc<McpToolAuthorizationPolicy>,
    ) -> Self {
        Self {
            manager,
            authorization,
            diagnostics: Mutex::new(Vec::new()),
        }
    }

    #[cfg(test)]
    fn diagnostics(&self) -> Vec<McpToolRegistrationDiagnostic> {
        self.diagnostics
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clone()
    }

    fn authorize_invocation(&self, invocation: &McpToolInvocation) -> AgentResult<()> {
        let server_id = McpServerId::from_str(&invocation.provenance.server_id)
            .map_err(|_| stale_tool_error("mcp.invalid_server_identity"))?;
        let expected_digest = McpConfigDigest::from_str(&invocation.provenance.config_digest)
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
            || status.catalog_completeness != McpCatalogCompleteness::Complete
            || status.config_digest != expected_digest
            || status.catalog_generation != invocation.provenance.catalog_generation
            || catalog
                .content_digest
                .as_ref()
                .map(ToString::to_string)
                .as_deref()
                != Some(invocation.provenance.catalog_digest.as_str())
            || !scope_matches_provenance(&status.scope, &invocation.provenance.scope)
            || !scope_visible_to_context(&status.scope, &invocation.caller)
            || !self
                .authorization
                .allows_unapproved_read_only(&invocation.provenance)
        {
            return Err(AgentError::structured(
                "mcp.invoke_not_authorized",
                "The MCP tool is not authorized for this Agent run.",
                serde_json::json!({
                    "type": "mcp_tool",
                    "code": "invokeNotAuthorized",
                    "retryable": false,
                }),
            ));
        }
        Ok(())
    }
}

impl McpToolInvoker for McpRuntimeBridge {
    fn catalog(&self, context: &McpToolCatalogContext) -> AgentResult<Vec<McpAgentToolDescriptor>> {
        let statuses = self.manager.list_statuses().map_err(map_catalog_error)?;
        let mut tools = Vec::new();
        let mut catalog_tool_count = 0_usize;
        let mut catalog_bytes = 0_usize;
        for status in statuses {
            if !status.enabled
                || status.state != McpServerState::Ready
                || status.trust == McpTrustLevel::Untrusted
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
                || current_status.config_digest != status.config_digest
                || current_status.catalog_generation != catalog.generation
                || current_status.scope != status.scope
                || current_status.trust != status.trust
                || current_status.trust == McpTrustLevel::Untrusted
                || !scope_visible_to_context(&current_status.scope, context)
            {
                continue;
            }
            let Some(scope) = map_scope(&current_status.scope) else {
                continue;
            };
            if catalog.server_id != status.server_id
                || catalog.completeness != McpCatalogCompleteness::Complete
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
                reserve_catalog_budget(&tool, &mut catalog_tool_count, &mut catalog_bytes)?;
                let provenance = AgentMcpToolProvenance {
                    server_id: status.server_id.to_string(),
                    scope: scope.clone(),
                    raw_tool_name: tool.raw_name,
                    model_tool_name: tool.model_name,
                    config_digest: current_status.config_digest.to_string(),
                    catalog_generation: catalog.generation,
                    catalog_digest: catalog_digest.clone(),
                };
                tools.push(McpAgentToolDescriptor {
                    host_allows_unapproved_read_only_invocation: self
                        .authorization
                        .allows_unapproved_read_only(&provenance),
                    provenance,
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

    fn invoke<'a>(
        &'a self,
        invocation: McpToolInvocation,
        cancellation: AgentCancellationToken,
    ) -> McpToolInvocationFuture<'a> {
        Box::pin(async move {
            cancellation.check()?;
            self.authorize_invocation(&invocation)?;
            let request = catalog_call(&invocation)?;
            let mcp_cancellation = McpCancellationToken::new();
            let call = self
                .manager
                .call_catalog_tool(request, mcp_cancellation.clone());
            tokio::pin!(call);
            tokio::select! {
                biased;
                _ = cancellation.cancelled() => {
                    mcp_cancellation.cancel();
                    // Keep polling the protocol future after signalling cancellation so the MCP
                    // client can issue its cancellation notification instead of merely dropping
                    // the request handle. A faulty peer cannot hold Agent cancellation forever.
                    let _ = tokio::time::timeout(MCP_CANCELLATION_SETTLE_GRACE, &mut call).await;
                    Err(AgentError::cancelled())
                }
                result = &mut call => map_tool_result(result.map_err(map_invocation_error)?),
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
    if next_count > MCP_RUNTIME_MAX_TOOL_DEFINITIONS || next_bytes > MCP_RUNTIME_MAX_CATALOG_BYTES {
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

fn catalog_call(invocation: &McpToolInvocation) -> AgentResult<McpCatalogToolCall> {
    let server_id = McpServerId::from_str(&invocation.provenance.server_id)
        .map_err(|_| stale_tool_error("mcp.invalid_server_identity"))?;
    let config_digest = McpConfigDigest::from_str(&invocation.provenance.config_digest)
        .map_err(|_| stale_tool_error("mcp.invalid_config_digest"))?;
    let catalog_digest = McpCatalogDigest::from_str(&invocation.provenance.catalog_digest)
        .map_err(|_| stale_tool_error("mcp.invalid_catalog_digest"))?;
    if invocation.provenance.raw_tool_name.trim().is_empty()
        || invocation.provenance.model_tool_name.trim().is_empty()
        || invocation.provenance.catalog_generation == 0
    {
        return Err(stale_tool_error("mcp.invalid_tool_identity"));
    }
    Ok(McpCatalogToolCall {
        tool_id: McpToolId {
            server_id,
            raw_name: invocation.provenance.raw_tool_name.clone(),
        },
        expected_config_digest: config_digest,
        expected_catalog_generation: invocation.provenance.catalog_generation,
        expected_catalog_digest: catalog_digest,
        expected_model_name: invocation.provenance.model_tool_name.clone(),
        arguments: invocation.arguments.clone(),
        timeout_ms: None,
    })
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

fn map_tool_result(result: McpToolResult) -> AgentResult<McpToolInvocationResult> {
    let mut content = Vec::new();
    let mut remaining_text_bytes = MAX_MCP_BRIDGE_TEXT_BYTES;
    let mut truncated_at_source = result.content.len() > MAX_MCP_BRIDGE_CONTENT_BLOCKS;
    for block in result
        .content
        .into_iter()
        .take(MAX_MCP_BRIDGE_CONTENT_BLOCKS)
    {
        match block {
            McpContentBlock::Text { mut text } => {
                if text.len() > remaining_text_bytes {
                    truncate_string_bytes(&mut text, remaining_text_bytes);
                    truncated_at_source = true;
                }
                remaining_text_bytes = remaining_text_bytes.saturating_sub(text.len());
                content.push(McpToolContentBlock::Text { text });
            }
            block => content.push(map_content_block(block)),
        }
    }
    let structured_content = match result.structured_content {
        Some(structured) if structured_content_within_limits(&structured) => Some(structured),
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

fn map_content_block(content: McpContentBlock) -> McpToolContentBlock {
    match content {
        McpContentBlock::Text { mut text } => {
            truncate_string_bytes(&mut text, MAX_MCP_BRIDGE_TEXT_BYTES);
            McpToolContentBlock::Text { text }
        }
        McpContentBlock::Image { data, mime_type } => McpToolContentBlock::Omitted {
            kind: McpOmittedContentKind::Image,
            mime_type: safe_mime_type(Some(mime_type)),
            encoded_bytes: Some(saturating_u64(data.len())),
        },
        McpContentBlock::Audio { data, mime_type } => McpToolContentBlock::Omitted {
            kind: McpOmittedContentKind::Audio,
            mime_type: safe_mime_type(Some(mime_type)),
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
                mime_type: safe_mime_type(mime_type),
                encoded_bytes: Some(saturating_u64(content_bytes)),
            }
        }
        McpContentBlock::ResourceLink { resource } => map_resource_link(resource),
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

fn structured_content_within_limits(value: &serde_json::Value) -> bool {
    fn visit(
        value: &serde_json::Value,
        depth: usize,
        nodes: &mut usize,
        bytes: &mut usize,
    ) -> bool {
        if depth > MAX_MCP_BRIDGE_STRUCTURED_DEPTH
            || *nodes >= MAX_MCP_BRIDGE_STRUCTURED_NODES
            || *bytes > MAX_MCP_BRIDGE_STRUCTURED_BYTES
        {
            return false;
        }
        *nodes += 1;
        match value {
            serde_json::Value::Object(object) => object.iter().all(|(key, value)| {
                *bytes = bytes.saturating_add(key.len());
                *bytes <= MAX_MCP_BRIDGE_STRUCTURED_BYTES && visit(value, depth + 1, nodes, bytes)
            }),
            serde_json::Value::Array(array) => array
                .iter()
                .all(|value| visit(value, depth + 1, nodes, bytes)),
            serde_json::Value::String(value) => {
                *bytes = bytes.saturating_add(value.len());
                *bytes <= MAX_MCP_BRIDGE_STRUCTURED_BYTES
            }
            serde_json::Value::Number(_) => {
                *bytes = bytes.saturating_add(32);
                *bytes <= MAX_MCP_BRIDGE_STRUCTURED_BYTES
            }
            serde_json::Value::Bool(_) | serde_json::Value::Null => {
                *bytes = bytes.saturating_add(8);
                *bytes <= MAX_MCP_BRIDGE_STRUCTURED_BYTES
            }
        }
    }

    let mut nodes = 0;
    let mut bytes = 0;
    visit(value, 0, &mut nodes, &mut bytes)
        && serde_json::to_vec(value)
            .is_ok_and(|encoded| encoded.len() <= MAX_MCP_BRIDGE_STRUCTURED_BYTES)
}

fn map_resource_link(resource: McpResourceLink) -> McpToolContentBlock {
    McpToolContentBlock::Omitted {
        kind: McpOmittedContentKind::ResourceLink,
        mime_type: safe_mime_type(resource.mime_type),
        encoded_bytes: resource.size,
    }
}

fn safe_mime_type(mime_type: Option<String>) -> Option<String> {
    let value = mime_type?;
    if value.is_empty()
        || value.len() > MAX_MIME_TYPE_BYTES
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

fn map_catalog_error(_error: McpError) -> AgentError {
    AgentError::structured(
        "mcp.catalog_unavailable",
        "The MCP tool catalog is temporarily unavailable.",
        serde_json::json!({"retryable": true}),
    )
}

fn map_invocation_error(error: McpError) -> AgentError {
    match error.kind {
        McpErrorKind::Cancelled => AgentError::cancelled(),
        McpErrorKind::Timeout => AgentError::structured(
            "mcp.tool_timeout",
            "The MCP tool invocation timed out.",
            serde_json::json!({"retryable": true}),
        ),
        McpErrorKind::Config => stale_tool_error("mcp.tool_snapshot_stale"),
        McpErrorKind::Spawn
        | McpErrorKind::Negotiation
        | McpErrorKind::Protocol
        | McpErrorKind::ServerExited
        | McpErrorKind::Shutdown => AgentError::structured(
            "mcp.tool_unavailable",
            "The MCP tool is temporarily unavailable.",
            serde_json::json!({"retryable": true}),
        ),
        _ => AgentError::structured(
            "mcp.tool_unavailable",
            "The MCP tool is temporarily unavailable.",
            serde_json::json!({"retryable": true}),
        ),
    }
}

fn stale_tool_error(code: &'static str) -> AgentError {
    AgentError::structured(
        code,
        "The MCP tool definition is stale or invalid; refresh the tool catalog and retry.",
        serde_json::json!({"retryable": true}),
    )
}

#[cfg(test)]
mod tests {
    use mycopilot_core::{
        send_chat_with_host_services, AgentApiStyle, AgentChatInput, AgentChatMessage,
        AgentEventEmitter, AgentRunContext, AgentRuntimeHostServices, AgentToolIdentity,
        ConversationTraceSnapshot, ConversationTurnTraceItem, McpToolRuntime, ModelCapabilities,
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
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::{TcpListener, TcpStream};

    use super::*;

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

    fn config(server_id: McpServerId) -> McpServerConfig {
        McpServerConfig {
            id: server_id,
            display_name: "owned fixture".to_string(),
            scope: McpServerScope::Project {
                project_id: "project-fixture".to_string(),
            },
            trust: McpTrustLevel::Managed,
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

    fn runtime_input(api_url: String, suffix: &str) -> AgentChatInput {
        AgentChatInput {
            api_url,
            api_token: "owned-fixture-token".to_string(),
            model: "owned-fixture-model".to_string(),
            model_capabilities: ModelCapabilities::default(),
            api_style: Some(AgentApiStyle::OpenAiCompatible),
            context_window_tokens: Some(128_000),
            context_window_indicator_enabled: false,
            max_tokens: Some(4_096),
            temperature: None,
            stream: Some(false),
            context: Some(AgentRunContext {
                conversation_id: Some(format!("conversation-{suffix}")),
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
            assistant_message_id: Some(format!("assistant-{suffix}")),
            context_compaction_summary: None,
            goal: None,
            world_state_records: Vec::new(),
            skill_activation: None,
            skill_discovery: None,
            messages: vec![AgentChatMessage {
                message_id: Some(format!("user-{suffix}")),
                role: "user".to_string(),
                content: "Call the owned MCP fixture.".to_string(),
                created_at: Some(1),
                conversation_turn_trace: None,
                conversation_model_context_items: Vec::new(),
            }],
        }
    }

    async fn ready_bridge() -> (
        Arc<McpRuntimeBridge>,
        Arc<McpConnectionManager>,
        Arc<OwnedFixturePeer>,
    ) {
        let server_id = McpServerId::new();
        let registry = InMemoryMcpRegistry::shared();
        registry.add(config(server_id)).unwrap();
        let peer = Arc::new(OwnedFixturePeer::new(server_id));
        let connector: Arc<dyn McpConnector> = Arc::new(OwnedFixtureConnector {
            peer: Arc::clone(&peer),
        });
        let manager = Arc::new(
            McpConnectionManager::without_events(registry, connector, McpManagerPolicy::default())
                .unwrap(),
        );
        manager.start(server_id).await.unwrap();
        let denied_bridge = McpRuntimeBridge::new(Arc::clone(&manager));
        let provenance = denied_bridge
            .catalog(&project_context())
            .unwrap()
            .remove(0)
            .provenance;
        let authorization = Arc::new(
            McpToolAuthorizationPolicy::deny_all().allow_unapproved_read_only(&provenance),
        );
        (
            Arc::new(McpRuntimeBridge::with_authorization(
                Arc::clone(&manager),
                authorization,
            )),
            manager,
            peer,
        )
    }

    #[tokio::test]
    async fn bridge_catalog_budget_fails_before_unbounded_cross_server_aggregation() {
        let (_bridge, manager, _peer) = ready_bridge().await;
        let server_id = manager.list_statuses().unwrap()[0].server_id;
        let tool = manager.catalog(server_id).unwrap().unwrap().tools[0].clone();
        let mut tool_count = 0;
        let mut bytes = 0;
        for _ in 0..MCP_RUNTIME_MAX_TOOL_DEFINITIONS {
            reserve_catalog_budget(&tool, &mut tool_count, &mut bytes).unwrap();
        }
        let error = reserve_catalog_budget(&tool, &mut tool_count, &mut bytes).unwrap_err();
        assert_eq!(error.code(), Some("mcp.catalog_budget_exceeded"));

        let mut oversized = tool;
        oversized.descriptor.input_schema = json!({
            "type": "object",
            "description": "x".repeat(MCP_RUNTIME_MAX_CATALOG_BYTES + 1),
        });
        let error = reserve_catalog_budget(&oversized, &mut 0, &mut 0).unwrap_err();
        assert_eq!(error.code(), Some("mcp.catalog_budget_exceeded"));
        manager.stop_all().await;
    }

    #[tokio::test]
    async fn ready_catalog_routes_by_typed_raw_identity_and_omits_binary_payload() {
        let (bridge, manager, peer) = ready_bridge().await;
        let catalog = bridge.catalog(&project_context()).unwrap();
        assert_eq!(catalog.len(), 1);
        let descriptor = catalog.into_iter().next().unwrap();
        assert_eq!(descriptor.provenance.raw_tool_name, "raw/echo");
        assert!(descriptor.provenance.model_tool_name.starts_with("mcp__"));
        assert_eq!(
            descriptor.provenance.scope,
            AgentMcpServerScope::Project {
                project_id: "project-fixture".to_string()
            }
        );
        assert_eq!(descriptor.annotations.read_only_hint, Some(true));
        assert!(descriptor.output_schema.is_some());

        let result = bridge
            .invoke(
                McpToolInvocation {
                    provenance: descriptor.provenance,
                    arguments: json!({"text": "hello"}),
                    caller: project_context(),
                },
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
    async fn connection_trust_does_not_replace_tool_authorization_or_project_scope() {
        let (authorized_bridge, manager, _) = ready_bridge().await;
        assert!(authorized_bridge
            .catalog(&McpToolCatalogContext::default())
            .unwrap()
            .is_empty());
        assert!(authorized_bridge
            .catalog(&McpToolCatalogContext {
                project_id: Some("different-project".to_string()),
            })
            .unwrap()
            .is_empty());

        let denied_bridge = McpRuntimeBridge::new(Arc::clone(&manager));
        let descriptor = denied_bridge.catalog(&project_context()).unwrap().remove(0);
        assert!(!descriptor.host_allows_unapproved_read_only_invocation);
        let error = denied_bridge
            .invoke(
                McpToolInvocation {
                    provenance: descriptor.provenance,
                    arguments: json!({"text": "denied"}),
                    caller: project_context(),
                },
                AgentCancellationToken::new(),
            )
            .await
            .unwrap_err();
        assert_eq!(error.code(), Some("mcp.invoke_not_authorized"));
        let _ = manager.stop_all().await;
    }

    #[tokio::test]
    async fn agent_cancellation_is_forwarded_to_mcp_call() {
        let (bridge, manager, peer) = ready_bridge().await;
        peer.wait_for_cancellation.store(true, Ordering::SeqCst);
        let descriptor = bridge.catalog(&project_context()).unwrap().remove(0);
        let cancellation = AgentCancellationToken::new();
        let trigger = cancellation.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(10)).await;
            trigger.cancel();
        });
        let error = bridge
            .invoke(
                McpToolInvocation {
                    provenance: descriptor.provenance,
                    arguments: json!({"text": "cancel"}),
                    caller: project_context(),
                },
                cancellation,
            )
            .await
            .unwrap_err();
        assert!(error.is_cancelled());
        assert!(peer.cancellation_seen.load(Ordering::SeqCst));
        let _ = manager.stop_all().await;
    }

    #[tokio::test]
    async fn registry_runtime_cancellation_settles_through_the_mcp_bridge() {
        let (bridge, manager, peer) = ready_bridge().await;
        peer.wait_for_cancellation.store(true, Ordering::SeqCst);
        let model_name = bridge
            .catalog(&project_context())
            .unwrap()
            .remove(0)
            .provenance
            .model_tool_name;
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let _ = read_json_request(&mut stream).await;
            write_json_response(
                &mut stream,
                json!({
                    "choices": [{
                        "message": {
                            "role": "assistant",
                            "content": null,
                            "tool_calls": [{
                                "id": "provider-cancel-call",
                                "type": "function",
                                "function": {
                                    "name": model_name,
                                    "arguments": "{\"text\":\"wait\"}"
                                }
                            }]
                        },
                        "finish_reason": "tool_calls"
                    }]
                }),
            )
            .await;
        });

        let invoker: Arc<dyn McpToolInvoker> = bridge.clone();
        let host_services = AgentRuntimeHostServices::new().with_mcp_tools(
            McpToolRuntime::capture_for_context(invoker, project_context()),
        );
        let cancellation = AgentCancellationToken::new();
        let run_cancellation = cancellation.clone();
        let run = tokio::spawn(async move {
            send_chat_with_host_services(
                runtime_input(
                    format!("http://{address}/v1/chat/completions"),
                    "mcp-cancel-e2e",
                ),
                "run-mcp-cancel-e2e".to_string(),
                Arc::new(|_| {}),
                run_cancellation,
                host_services,
            )
            .await
        });
        let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
        while !peer.call_started.load(Ordering::SeqCst) {
            assert!(
                tokio::time::Instant::now() < deadline,
                "owned MCP call did not start"
            );
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        cancellation.cancel();
        let output = tokio::time::timeout(Duration::from_secs(2), run)
            .await
            .expect("Agent cancellation must settle")
            .unwrap()
            .unwrap();
        server.await.unwrap();

        assert_eq!(output.status, mycopilot_core::AgentRunStatus::Cancelled);
        assert!(peer.cancellation_seen.load(Ordering::SeqCst));
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
            for request_index in 0..2 {
                let (mut stream, _) = listener.accept().await.unwrap();
                let request = read_json_request(&mut stream).await;
                requests_for_server
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .push(request);
                let response = if request_index == 0 {
                    json!({
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
                    })
                } else {
                    json!({
                        "choices": [{
                            "message": {
                                "role": "assistant",
                                "content": "MCP fixture completed."
                            },
                            "finish_reason": "stop"
                        }]
                    })
                };
                write_json_response(&mut stream, response).await;
            }
        });

        let input = AgentChatInput {
            api_url: format!("http://{address}/v1/chat/completions"),
            api_token: "owned-fixture-token".to_string(),
            model: "owned-fixture-model".to_string(),
            model_capabilities: ModelCapabilities::default(),
            api_style: Some(AgentApiStyle::OpenAiCompatible),
            context_window_tokens: Some(128_000),
            context_window_indicator_enabled: false,
            max_tokens: Some(4_096),
            temperature: None,
            stream: Some(false),
            context: Some(AgentRunContext {
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
            goal: None,
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

        assert_eq!(output.content, "MCP fixture completed.");
        {
            let requests = requests.lock().unwrap_or_else(|error| error.into_inner());
            assert_eq!(requests.len(), 2);
            assert!(requests[0]["tools"]
                .as_array()
                .unwrap()
                .iter()
                .any(|tool| tool["function"]["name"] == model_name));
            let tool_message = requests[1]["messages"]
                .as_array()
                .unwrap()
                .iter()
                .find(|message| message["role"] == "tool")
                .expect("second request contains the MCP tool result");
            let tool_content: serde_json::Value =
                serde_json::from_str(tool_message["content"].as_str().unwrap()).unwrap();
            assert_eq!(tool_content["structuredContent"], json!({"sum": 3}));
            assert!(
                tool_message["content"]
                    .as_str()
                    .unwrap()
                    .contains(MCP_PRIVATE_RESULT),
                "the live model turn receives the bounded MCP result"
            );
            assert!(
                tool_content.get("provenance").is_none(),
                "typed MCP routing provenance stays in the trace, not model content"
            );
            assert!(
                !requests[1].to_string().contains(MODEL_PRIVATE_ARGUMENT),
                "credential-shaped MCP arguments are not replayed into model context"
            );
        }

        {
            let calls = peer.calls.lock().unwrap_or_else(|error| error.into_inner());
            assert_eq!(calls.len(), 1);
            assert_eq!(calls[0].name, "raw/echo");
            assert_eq!(
                calls[0].arguments,
                json!({
                    "text": "runtime request",
                    "api_token": MODEL_PRIVATE_ARGUMENT,
                })
            );
        }

        let trace = output
            .conversation_turn_trace
            .expect("runtime produces an MCP trace");
        assert!(trace.truncated);
        let durable_trace = serde_json::to_string(&trace).unwrap();
        assert!(!durable_trace.contains(MODEL_PRIVATE_ARGUMENT));
        assert!(
            !durable_trace.contains(MCP_PRIVATE_RESULT),
            "MCP output remains transient and is not copied into durable model context"
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
            assert!(trace_snapshots
                .last()
                .expect("runtime publishes a final MCP trace snapshot")
                .model_context_items
                .iter()
                .any(|item| item.content.contains("externalToolOutputNotPersisted")));
        }
        assert!(trace.items.iter().any(|item| matches!(
            item,
            ConversationTurnTraceItem::ToolCall {
                provenance: Some(AgentToolIdentity::Mcp { provenance }),
                ..
            } if provenance.raw_tool_name == "raw/echo"
                && provenance.model_tool_name == model_name
        )));
        let _ = manager.stop_all().await;
    }

    #[test]
    fn resource_uri_and_binary_content_are_not_retained() {
        let mapped = map_content_block(McpContentBlock::EmbeddedResource {
            resource: McpEmbeddedResource::Blob {
                uri: "https://secret.invalid/private".to_string(),
                mime_type: Some("application/octet-stream".to_string()),
                data: "private-base64".to_string(),
            },
        });
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
