use super::{
    validate_portable_tool_input_schema, AgentTool, AgentToolExposure, AgentToolPermissionPolicy,
    AsyncAgentTool, BoxAgentToolFuture, ToolExecutionContext,
};
use crate::protocol::{
    AgentError, AgentMcpServerScope, AgentMcpToolProvenance, AgentResult, AgentToolApprovalMode,
    AgentToolDefinition, AgentToolSafety,
};
use crate::AgentCancellationToken;
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

const MAX_MCP_MODEL_TOOL_NAME_BYTES: usize = 64;
const MAX_MCP_RAW_TOOL_NAME_BYTES: usize = 1_024;
const MAX_MCP_DESCRIPTION_BYTES: usize = 1_024;
const MAX_MCP_PROVIDER_SCHEMA_BYTES: usize = 256 * 1_024;
const MAX_MCP_TEXT_RESULT_BYTES: usize = 16 * 1_024;
const MAX_MCP_STRUCTURED_RESULT_BYTES: usize = 8 * 1_024;
const MAX_MCP_CONTENT_BLOCKS: usize = 128;
/// Hard Host-wide cap applied before MCP definitions enter an Agent request.
pub const MCP_RUNTIME_MAX_TOOL_DEFINITIONS: usize = 64;
/// Combined names, descriptions, input schemas and output schemas retained for one Agent run.
pub const MCP_RUNTIME_MAX_CATALOG_BYTES: usize = 128 * 1024;

pub type McpToolInvocationFuture<'a> =
    Pin<Box<dyn Future<Output = AgentResult<McpToolInvocationResult>> + Send + 'a>>;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct McpAgentToolAnnotations {
    pub read_only_hint: Option<bool>,
    pub destructive_hint: Option<bool>,
    pub idempotent_hint: Option<bool>,
    pub open_world_hint: Option<bool>,
}

#[derive(Clone, PartialEq)]
pub struct McpAgentToolDescriptor {
    pub provenance: AgentMcpToolProvenance,
    pub description: Option<String>,
    pub input_schema: Value,
    /// Retained for validation, provenance and future output handling. It is intentionally not
    /// projected into either provider's Tool definition in this round.
    pub output_schema: Option<Value>,
    pub annotations: McpAgentToolAnnotations,
    /// Host-owned authorization, independent of the Server-authored annotation hint.
    ///
    /// User/project/plugin Servers remain false until the approval subsystem can persist and
    /// recover typed MCP actions. Managed and built-in hosts may opt in after applying their own
    /// policy.
    pub host_allows_unapproved_read_only_invocation: bool,
}

impl fmt::Debug for McpAgentToolDescriptor {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("McpAgentToolDescriptor")
            .field("provenance", &self.provenance)
            .field(
                "description_bytes",
                &self.description.as_ref().map(String::len),
            )
            .field("input_schema", &"<redacted>")
            .field("has_output_schema", &self.output_schema.is_some())
            .field("annotations", &self.annotations)
            .field(
                "host_allows_unapproved_read_only_invocation",
                &self.host_allows_unapproved_read_only_invocation,
            )
            .finish()
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct McpToolCatalogContext {
    /// Backend-authoritative active project. A project-scoped Server is never visible without an
    /// exact match. Plugin visibility remains fail-closed until the Host supplies plugin context.
    pub project_id: Option<String>,
}

#[derive(Clone, PartialEq)]
pub struct McpToolInvocation {
    pub provenance: AgentMcpToolProvenance,
    pub arguments: Value,
    pub caller: McpToolCatalogContext,
}

impl fmt::Debug for McpToolInvocation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("McpToolInvocation")
            .field("provenance", &self.provenance)
            .field("arguments", &"<redacted>")
            .field("caller", &self.caller)
            .finish()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum McpOmittedContentKind {
    Image,
    Audio,
    EmbeddedResource,
    ResourceLink,
}

#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum McpToolContentBlock {
    Text {
        text: String,
    },
    Omitted {
        kind: McpOmittedContentKind,
        #[serde(skip_serializing_if = "Option::is_none")]
        mime_type: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        encoded_bytes: Option<u64>,
    },
}

impl fmt::Debug for McpToolContentBlock {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Text { text } => formatter
                .debug_struct("Text")
                .field("bytes", &text.len())
                .finish(),
            Self::Omitted {
                kind,
                mime_type,
                encoded_bytes,
            } => formatter
                .debug_struct("Omitted")
                .field("kind", kind)
                .field("mime_type", mime_type)
                .field("encoded_bytes", encoded_bytes)
                .finish(),
        }
    }
}

#[derive(Clone, PartialEq)]
pub struct McpToolInvocationResult {
    pub content: Vec<McpToolContentBlock>,
    pub structured_content: Option<Value>,
    pub is_error: bool,
    /// True when an upstream Host adapter had to discard content before this bounded projection.
    pub truncated_at_source: bool,
}

impl fmt::Debug for McpToolInvocationResult {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("McpToolInvocationResult")
            .field("content_blocks", &self.content.len())
            .field("has_structured_content", &self.structured_content.is_some())
            .field("is_error", &self.is_error)
            .field("truncated_at_source", &self.truncated_at_source)
            .finish()
    }
}

pub trait McpToolInvoker: Send + Sync {
    fn catalog(&self, context: &McpToolCatalogContext) -> AgentResult<Vec<McpAgentToolDescriptor>>;

    fn invoke<'a>(
        &'a self,
        invocation: McpToolInvocation,
        cancellation: AgentCancellationToken,
    ) -> McpToolInvocationFuture<'a>;

    fn report_diagnostics(&self, _diagnostics: &[McpToolRegistrationDiagnostic]) {}
}

/// One immutable MCP Tool snapshot shared by context-window preview and the corresponding run.
///
/// The invoker remains process-owned and connection-shared; individual tools retain only this
/// Arc plus their frozen descriptor.
#[derive(Clone)]
pub struct McpToolRuntime {
    invoker: Arc<dyn McpToolInvoker>,
    tools: Arc<[McpAgentToolDescriptor]>,
    initial_diagnostics: Arc<[McpToolRegistrationDiagnostic]>,
    catalog_context: McpToolCatalogContext,
}

impl fmt::Debug for McpToolRuntime {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("McpToolRuntime")
            .field("tool_count", &self.tools.len())
            .field("diagnostic_count", &self.initial_diagnostics.len())
            .field("catalog_context", &self.catalog_context)
            .finish_non_exhaustive()
    }
}

impl McpToolRuntime {
    pub fn capture(invoker: Arc<dyn McpToolInvoker>) -> Self {
        Self::capture_for_context(invoker, McpToolCatalogContext::default())
    }

    pub fn capture_for_context(
        invoker: Arc<dyn McpToolInvoker>,
        catalog_context: McpToolCatalogContext,
    ) -> Self {
        match invoker.catalog(&catalog_context) {
            Ok(tools) if catalog_within_runtime_budget(&tools) => Self {
                invoker,
                tools: tools.into(),
                initial_diagnostics: Arc::from([]),
                catalog_context,
            },
            Ok(_) => {
                let diagnostics: Arc<[McpToolRegistrationDiagnostic]> =
                    Arc::from([McpToolRegistrationDiagnostic::global(
                        McpToolDiagnosticCode::CatalogBudgetExceeded,
                    )]);
                invoker.report_diagnostics(&diagnostics);
                Self {
                    invoker,
                    tools: Arc::from([]),
                    initial_diagnostics: diagnostics,
                    catalog_context,
                }
            }
            Err(error) => {
                let code = if error.code() == Some("mcp.catalog_budget_exceeded") {
                    McpToolDiagnosticCode::CatalogBudgetExceeded
                } else {
                    McpToolDiagnosticCode::CatalogUnavailable
                };
                let diagnostics: Arc<[McpToolRegistrationDiagnostic]> =
                    Arc::from([McpToolRegistrationDiagnostic::global(code)]);
                invoker.report_diagnostics(&diagnostics);
                Self {
                    invoker,
                    tools: Arc::from([]),
                    initial_diagnostics: diagnostics,
                    catalog_context,
                }
            }
        }
    }

    pub fn is_empty(&self) -> bool {
        self.tools.is_empty()
    }

    pub(crate) fn tools(&self) -> &[McpAgentToolDescriptor] {
        &self.tools
    }

    pub(crate) fn invoker(&self) -> Arc<dyn McpToolInvoker> {
        Arc::clone(&self.invoker)
    }

    pub(crate) fn initial_diagnostics(&self) -> &[McpToolRegistrationDiagnostic] {
        &self.initial_diagnostics
    }

    pub(crate) fn catalog_context(&self) -> &McpToolCatalogContext {
        &self.catalog_context
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum McpToolDiagnosticCode {
    CatalogUnavailable,
    CatalogBudgetExceeded,
    InvalidIdentity,
    InvalidModelName,
    InvalidSchema,
    SchemaTooLarge,
    ApprovalRequiredUnsupported,
    NameCollision,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpToolRegistrationDiagnostic {
    pub server_id: Option<String>,
    pub model_tool_name: Option<String>,
    pub code: McpToolDiagnosticCode,
    pub message: String,
}

impl McpToolRegistrationDiagnostic {
    fn global(code: McpToolDiagnosticCode) -> Self {
        Self {
            server_id: None,
            model_tool_name: None,
            code,
            message: diagnostic_message(code).to_string(),
        }
    }

    fn for_tool(provenance: &AgentMcpToolProvenance, code: McpToolDiagnosticCode) -> Self {
        Self {
            server_id: uuid::Uuid::parse_str(&provenance.server_id)
                .ok()
                .map(|server_id| server_id.to_string()),
            model_tool_name: valid_model_name(&provenance.model_tool_name)
                .then(|| provenance.model_tool_name.clone()),
            code,
            message: diagnostic_message(code).to_string(),
        }
    }

    pub(super) fn name_collision(provenance: &AgentMcpToolProvenance) -> Self {
        Self::for_tool(provenance, McpToolDiagnosticCode::NameCollision)
    }
}

fn diagnostic_message(code: McpToolDiagnosticCode) -> &'static str {
    match code {
        McpToolDiagnosticCode::CatalogUnavailable => {
            "The MCP catalog was unavailable; built-in tools remain usable."
        }
        McpToolDiagnosticCode::CatalogBudgetExceeded => {
            "The MCP catalog exceeds the Host tool budget; built-in tools remain usable."
        }
        McpToolDiagnosticCode::InvalidIdentity => "The MCP tool identity is invalid.",
        McpToolDiagnosticCode::InvalidModelName => "The MCP model-visible tool name is invalid.",
        McpToolDiagnosticCode::InvalidSchema => {
            "The MCP input schema is incompatible with supported providers."
        }
        McpToolDiagnosticCode::SchemaTooLarge => {
            "The MCP input schema exceeds the provider-facing size limit."
        }
        McpToolDiagnosticCode::ApprovalRequiredUnsupported => {
            "The MCP tool is not host-authorized as read-only; MCP approval recovery is not enabled."
        }
        McpToolDiagnosticCode::NameCollision => {
            "The MCP tool name collides with an already registered tool."
        }
    }
}

pub(super) struct McpAgentTool {
    invoker: Arc<dyn McpToolInvoker>,
    provenance: AgentMcpToolProvenance,
    caller: McpToolCatalogContext,
    definition: AgentToolDefinition,
    #[allow(dead_code)]
    output_schema: Option<Value>,
}

impl McpAgentTool {
    pub(super) fn prepare(
        descriptor: McpAgentToolDescriptor,
        invoker: Arc<dyn McpToolInvoker>,
        caller: McpToolCatalogContext,
    ) -> Result<Self, McpToolRegistrationDiagnostic> {
        validate_provenance(&descriptor.provenance)?;
        if !descriptor.host_allows_unapproved_read_only_invocation
            || descriptor.annotations.read_only_hint != Some(true)
            || descriptor.annotations.destructive_hint == Some(true)
        {
            return Err(McpToolRegistrationDiagnostic::for_tool(
                &descriptor.provenance,
                McpToolDiagnosticCode::ApprovalRequiredUnsupported,
            ));
        }
        let input_schema = normalize_input_schema(&descriptor.provenance, descriptor.input_schema)?;
        let description = normalize_description(descriptor.description.as_deref());
        let definition = AgentToolDefinition {
            name: descriptor.provenance.model_tool_name.clone(),
            description,
            input_schema,
            safety: AgentToolSafety::ReadOnly,
            requires_workspace: false,
            requires_approval: false,
            approval_mode: AgentToolApprovalMode::Never,
        };
        Ok(Self {
            invoker,
            provenance: descriptor.provenance,
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

    fn cancellation_settlement(&self) -> super::AgentToolCancellationSettlement {
        // The Host bridge owns MCP cancellation notification and a bounded settlement grace.
        // Let that future observe the token instead of dropping it at the outer Registry select.
        super::AgentToolCancellationSettlement::Authoritative
    }

    fn exposure(&self) -> AgentToolExposure {
        AgentToolExposure::Dynamic
    }

    fn archives_result(&self) -> bool {
        false
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
        project_mcp_tool_call(call)
    }

    fn model_projection(
        &self,
        result: &crate::protocol::AgentToolResult,
    ) -> crate::protocol::AgentToolResult {
        let mut projected = super::canonical_tool_result_for_context(result);
        if let Some(object) = projected.result.as_mut().and_then(Value::as_object_mut) {
            object.remove("provenance");
        }
        projected
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
        crate::protocol::AgentToolResult {
            call_id: result.call_id.clone(),
            tool: result.tool.clone(),
            ok: result.ok,
            result: Some(json!({
                "mcp": {
                    "contentOmitted": true,
                    "reason": "externalToolOutputNotPersisted",
                    "isError": !result.ok,
                },
                "provenance": self.provenance,
            })),
            error: (!result.ok).then(|| {
                "MCP tool reported an error; external error details were not persisted.".to_string()
            }),
            exact_archive_file: None,
        }
    }
}

impl AsyncAgentTool for McpAgentTool {
    fn execute_async<'a>(
        &'a self,
        context: &'a ToolExecutionContext,
        args: Value,
    ) -> BoxAgentToolFuture<'a> {
        if !args.is_object() {
            return Box::pin(async {
                Err(AgentError::structured(
                    "mcp.invalid_tool_arguments",
                    "MCP tool arguments must be a JSON object.",
                    json!({
                        "type": "mcp_tool_arguments",
                        "code": "rootMustBeObject",
                    }),
                ))
            });
        }
        let invocation = McpToolInvocation {
            provenance: self.provenance.clone(),
            arguments: args,
            caller: self.caller.clone(),
        };
        let cancellation = context.cancellation_token();
        Box::pin(async move {
            context.check_cancelled()?;
            let result = self.invoker.invoke(invocation, cancellation).await?;
            let value = invocation_result_value(&self.provenance, &result)?;
            if result.is_error {
                return Err(AgentError::structured(
                    "mcp.tool_execution_error",
                    "The MCP server reported a tool execution error.",
                    value,
                ));
            }
            Ok(value)
        })
    }
}

fn invocation_result_value(
    provenance: &AgentMcpToolProvenance,
    result: &McpToolInvocationResult,
) -> AgentResult<Value> {
    let (content, text_truncated, blocks_truncated) = bounded_content(&result.content);
    let content = serde_json::to_value(content)
        .map_err(|_| AgentError::new("MCP tool content could not be normalized."))?;
    let mut object = Map::from_iter([
        ("schemaVersion".to_string(), json!(1)),
        ("content".to_string(), content),
        ("isError".to_string(), json!(result.is_error)),
        (
            "provenance".to_string(),
            serde_json::to_value(provenance)
                .map_err(|_| AgentError::new("MCP provenance could not be normalized."))?,
        ),
    ]);
    if let Some(structured) = &result.structured_content {
        let structured_bytes = serde_json::to_vec(structured)
            .map_err(|_| AgentError::new("MCP structured content could not be normalized."))?;
        if structured_bytes.len() <= MAX_MCP_STRUCTURED_RESULT_BYTES {
            object.insert("structuredContent".to_string(), structured.clone());
        } else {
            object.insert(
                "structuredContent".to_string(),
                json!({
                    "_mycopilot": {
                        "omitted": true,
                        "reason": "structured_content_limit",
                        "originalBytes": structured_bytes.len(),
                    }
                }),
            );
        }
    }
    let structured_truncated = result
        .structured_content
        .as_ref()
        .and_then(|structured| serde_json::to_vec(structured).ok())
        .is_some_and(|bytes| bytes.len() > MAX_MCP_STRUCTURED_RESULT_BYTES);
    if result.truncated_at_source || text_truncated || blocks_truncated || structured_truncated {
        object.insert("truncatedAtSource".to_string(), Value::Bool(true));
        object.insert(
            "diagnostics".to_string(),
            json!({
                "textTruncated": text_truncated,
                "contentBlocksTruncated": blocks_truncated,
                "structuredContentTruncated": structured_truncated,
                "upstreamContentTruncated": result.truncated_at_source,
            }),
        );
    } else {
        object.insert("truncatedAtSource".to_string(), Value::Bool(false));
    }
    Ok(Value::Object(object))
}

fn bounded_content(content: &[McpToolContentBlock]) -> (Vec<McpToolContentBlock>, bool, bool) {
    let mut remaining_text_bytes = MAX_MCP_TEXT_RESULT_BYTES;
    let mut output = Vec::new();
    let mut text_truncated = false;
    let blocks_truncated = content.len() > MAX_MCP_CONTENT_BLOCKS;

    for block in content.iter().take(MAX_MCP_CONTENT_BLOCKS) {
        match block {
            McpToolContentBlock::Text { text } => {
                if remaining_text_bytes == 0 {
                    text_truncated = true;
                    continue;
                }
                if text.len() <= remaining_text_bytes {
                    output.push(block.clone());
                    remaining_text_bytes -= text.len();
                } else {
                    let retained = truncate_utf8(text, remaining_text_bytes);
                    output.push(McpToolContentBlock::Text {
                        text: format!(
                            "{retained}\n[MCP text output truncated by host; original block was {} bytes.]",
                            text.len()
                        ),
                    });
                    remaining_text_bytes = 0;
                    text_truncated = true;
                }
            }
            McpToolContentBlock::Omitted { .. } => output.push(block.clone()),
        }
    }
    (output, text_truncated, blocks_truncated)
}

fn catalog_within_runtime_budget(tools: &[McpAgentToolDescriptor]) -> bool {
    if tools.len() > MCP_RUNTIME_MAX_TOOL_DEFINITIONS {
        return false;
    }
    let mut bytes = 0_usize;
    for tool in tools {
        let Some(next) = bytes
            .checked_add(tool.provenance.model_tool_name.len())
            .and_then(|value| value.checked_add(tool.provenance.raw_tool_name.len()))
            .and_then(|value| value.checked_add(tool.description.as_ref().map_or(0, String::len)))
        else {
            return false;
        };
        bytes = next;
        for schema in std::iter::once(&tool.input_schema).chain(tool.output_schema.as_ref()) {
            let Ok(encoded) = serde_json::to_vec(schema) else {
                return false;
            };
            let Some(next) = bytes.checked_add(encoded.len()) else {
                return false;
            };
            bytes = next;
            if bytes > MCP_RUNTIME_MAX_CATALOG_BYTES {
                return false;
            }
        }
    }
    bytes <= MCP_RUNTIME_MAX_CATALOG_BYTES
}

fn project_mcp_tool_call(call: &crate::protocol::AgentToolCall) -> crate::protocol::AgentToolCall {
    let mut projected = call.clone();
    // MCP arguments are execution-only until the Host has a Secret Store and an explicit
    // per-tool persistence policy. Server-authored schemas and field names cannot prove that a
    // scalar is safe to retain in traces, events, model history, or approval checkpoints.
    projected.args = Value::Object(Map::new());
    projected.reason = None;
    projected
}

fn is_sensitive_mcp_argument_name(name: &str) -> bool {
    let normalized = name
        .bytes()
        .filter(|byte| byte.is_ascii_alphanumeric())
        .map(|byte| byte.to_ascii_lowercase())
        .collect::<Vec<_>>();
    let normalized = String::from_utf8(normalized).unwrap_or_default();
    matches!(
        normalized.as_str(),
        "password"
            | "passwd"
            | "secret"
            | "token"
            | "apitoken"
            | "clientsecret"
            | "apikey"
            | "accesstoken"
            | "refreshtoken"
            | "idtoken"
            | "authtoken"
            | "bearertoken"
            | "authorization"
            | "cookie"
            | "setcookie"
            | "credential"
            | "credentials"
            | "privatekey"
            | "sessiontoken"
            | "oauthcode"
            | "jwt"
            | "pat"
            | "sshkey"
    ) || [
        "password",
        "secret",
        "token",
        "apikey",
        "accesstoken",
        "refreshtoken",
        "privatekey",
        "credential",
    ]
    .iter()
    .any(|suffix| normalized.ends_with(suffix))
}

fn validate_provenance(
    provenance: &AgentMcpToolProvenance,
) -> Result<(), McpToolRegistrationDiagnostic> {
    if uuid::Uuid::parse_str(&provenance.server_id).is_err()
        || provenance.raw_tool_name.trim().is_empty()
        || provenance.raw_tool_name.trim() != provenance.raw_tool_name
        || provenance.raw_tool_name.len() > MAX_MCP_RAW_TOOL_NAME_BYTES
        || provenance.raw_tool_name.chars().any(char::is_control)
        || provenance.catalog_generation == 0
        || !valid_config_digest(&provenance.config_digest)
        || !valid_config_digest(&provenance.catalog_digest)
        || !valid_scope(&provenance.scope)
    {
        return Err(McpToolRegistrationDiagnostic::for_tool(
            provenance,
            McpToolDiagnosticCode::InvalidIdentity,
        ));
    }
    if !valid_model_name(&provenance.model_tool_name) {
        return Err(McpToolRegistrationDiagnostic::for_tool(
            provenance,
            McpToolDiagnosticCode::InvalidModelName,
        ));
    }
    Ok(())
}

fn valid_scope(scope: &AgentMcpServerScope) -> bool {
    let identifier = match scope {
        AgentMcpServerScope::Project { project_id } => Some(project_id),
        AgentMcpServerScope::Plugin { plugin_id } => Some(plugin_id),
        AgentMcpServerScope::Builtin | AgentMcpServerScope::User | AgentMcpServerScope::Managed => {
            None
        }
    };
    identifier.is_none_or(|identifier| {
        !identifier.trim().is_empty()
            && identifier.trim() == identifier
            && identifier.len() <= 1_024
            && !identifier.chars().any(char::is_control)
    })
}

fn valid_model_name(name: &str) -> bool {
    name.starts_with("mcp__")
        && !name.is_empty()
        && name.len() <= MAX_MCP_MODEL_TOOL_NAME_BYTES
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
}

fn valid_config_digest(digest: &str) -> bool {
    digest.len() == 64
        && digest
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn normalize_description(description: Option<&str>) -> String {
    let description = description
        .unwrap_or_default()
        .chars()
        .map(|character| {
            if character.is_control() && !matches!(character, '\n' | '\r' | '\t') {
                ' '
            } else {
                character
            }
        })
        .collect::<String>();
    let description = truncate_utf8(description.trim(), MAX_MCP_DESCRIPTION_BYTES);
    if description.is_empty() {
        "Read-only tool provided by an approved MCP server.".to_string()
    } else {
        description
    }
}

fn truncate_utf8(value: &str, max_bytes: usize) -> String {
    if value.len() <= max_bytes {
        return value.to_string();
    }
    let mut end = max_bytes;
    while !value.is_char_boundary(end) {
        end = end.saturating_sub(1);
    }
    value[..end].to_string()
}

fn normalize_input_schema(
    provenance: &AgentMcpToolProvenance,
    schema: Value,
) -> Result<Value, McpToolRegistrationDiagnostic> {
    let serialized = serde_json::to_vec(&schema).map_err(|_| {
        McpToolRegistrationDiagnostic::for_tool(provenance, McpToolDiagnosticCode::InvalidSchema)
    })?;
    if serialized.len() > MAX_MCP_PROVIDER_SCHEMA_BYTES {
        return Err(McpToolRegistrationDiagnostic::for_tool(
            provenance,
            McpToolDiagnosticCode::SchemaTooLarge,
        ));
    }
    let Value::Object(mut root) = schema else {
        return Err(McpToolRegistrationDiagnostic::for_tool(
            provenance,
            McpToolDiagnosticCode::InvalidSchema,
        ));
    };
    if !root.contains_key("type") {
        root.insert("type".to_string(), Value::String("object".to_string()));
    }
    let normalized = Value::Object(root);
    if schema_requests_model_supplied_credentials(&normalized) {
        return Err(McpToolRegistrationDiagnostic::for_tool(
            provenance,
            McpToolDiagnosticCode::InvalidSchema,
        ));
    }
    validate_portable_tool_input_schema(&provenance.model_tool_name, &normalized).map_err(
        |_| {
            McpToolRegistrationDiagnostic::for_tool(
                provenance,
                McpToolDiagnosticCode::InvalidSchema,
            )
        },
    )?;
    Ok(normalized)
}

fn schema_requests_model_supplied_credentials(schema: &Value) -> bool {
    let mut pending = vec![schema];
    let mut visited = 0_usize;
    while let Some(value) = pending.pop() {
        visited = visited.saturating_add(1);
        if visited > 4_096 {
            return true;
        }
        match value {
            Value::Object(object) => {
                if object
                    .get("properties")
                    .and_then(Value::as_object)
                    .is_some_and(|properties| {
                        properties
                            .keys()
                            .any(|name| is_sensitive_mcp_argument_name(name))
                    })
                {
                    return true;
                }
                pending.extend(object.values());
            }
            Value::Array(array) => pending.extend(array),
            Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => {}
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{AgentApprovalStatus, AgentToolCall, AgentToolIdentity};
    use crate::tools::{
        AgentToolCallCheckpointPersistence, AgentToolExposure, AgentToolPermissionPolicy,
        EffectiveToolSet, ToolRegistry,
    };
    use std::collections::BTreeSet;
    use std::sync::Mutex;
    use std::time::Duration;

    #[derive(Clone)]
    enum MockBehavior {
        Return(McpToolInvocationResult),
        WaitForCancellation,
    }

    struct MockMcpToolInvoker {
        catalog: Vec<McpAgentToolDescriptor>,
        behavior: MockBehavior,
        invocations: Mutex<Vec<McpToolInvocation>>,
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
                invocations: Mutex::new(Vec::new()),
                cancellation_tokens: Mutex::new(Vec::new()),
                diagnostics: Mutex::new(Vec::new()),
            })
        }

        fn waiting_for_cancellation(catalog: Vec<McpAgentToolDescriptor>) -> Arc<Self> {
            Arc::new(Self {
                catalog,
                behavior: MockBehavior::WaitForCancellation,
                invocations: Mutex::new(Vec::new()),
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

        fn invoke<'a>(
            &'a self,
            invocation: McpToolInvocation,
            cancellation: AgentCancellationToken,
        ) -> McpToolInvocationFuture<'a> {
            self.invocations
                .lock()
                .expect("invocations mutex")
                .push(invocation);
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
        AgentMcpToolProvenance {
            server_id: "4f4763c4-61a8-4a5a-9455-429f701548e1".to_string(),
            scope: AgentMcpServerScope::Project {
                project_id: "project-fixture".to_string(),
            },
            raw_tool_name: raw_name.to_string(),
            model_tool_name: model_name.to_string(),
            config_digest: "a".repeat(64),
            catalog_generation: 7,
            catalog_digest: "b".repeat(64),
        }
    }

    fn descriptor(raw_name: &str, model_name: &str, input_schema: Value) -> McpAgentToolDescriptor {
        McpAgentToolDescriptor {
            provenance: provenance(raw_name, model_name),
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
            host_allows_unapproved_read_only_invocation: true,
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
            id: "mcp-call-1".to_string(),
            tool: tool.to_string(),
            args,
            approval_status: AgentApprovalStatus::NotRequired,
            reason: None,
        }
    }

    async fn execute(
        registry: ToolRegistry,
        tool: &str,
        args: Value,
    ) -> AgentResult<crate::protocol::AgentToolResult> {
        let cancellation = AgentCancellationToken::new();
        let context =
            ToolExecutionContext::from_run_context(None).with_cancellation(cancellation.clone());
        Arc::new(registry)
            .execute_async(context, call(tool, args), cancellation)
            .await
    }

    #[test]
    fn catalog_descriptor_registers_provider_definition_and_typed_identity() {
        let model_name = "mcp__fixture__echo_text";
        let expected_provenance = provenance("echo_text", model_name);
        let invoker = MockMcpToolInvoker::returning(
            vec![descriptor(
                "echo_text",
                model_name,
                json!({
                    "type": "object",
                    "properties": {"text": {"type": "string"}},
                    "required": ["text"],
                    "additionalProperties": false
                }),
            )],
            empty_result(),
        );

        let registry = registry_with(invoker);
        let definition = registry
            .definition_for(model_name)
            .expect("MCP tool definition");

        assert_eq!(definition.name, model_name);
        assert_eq!(definition.description, "echo_text fixture tool");
        assert_eq!(definition.input_schema["type"], "object");
        assert_eq!(definition.input_schema["required"], json!(["text"]));
        assert_eq!(definition.safety, AgentToolSafety::ReadOnly);
        assert!(!definition.requires_approval);
        assert_eq!(definition.approval_mode, AgentToolApprovalMode::Never);
        assert_eq!(
            registry.identity(model_name),
            Some(&AgentToolIdentity::Mcp {
                provenance: expected_provenance,
            })
        );
        assert_eq!(
            registry.exposure(model_name),
            Some(&AgentToolExposure::Dynamic)
        );
    }

    #[test]
    fn runtime_catalog_budget_disables_all_mcp_tools_without_affecting_builtins() {
        let too_many = (0..=MCP_RUNTIME_MAX_TOOL_DEFINITIONS)
            .map(|index| {
                descriptor(
                    &format!("raw_{index}"),
                    &format!("mcp__fixture__tool_{index}"),
                    json!({"type": "object"}),
                )
            })
            .collect();
        let invoker = MockMcpToolInvoker::returning(too_many, empty_result());
        let runtime = McpToolRuntime::capture(invoker.clone());
        assert!(runtime.is_empty());
        assert_eq!(
            runtime.initial_diagnostics(),
            &[McpToolRegistrationDiagnostic::global(
                McpToolDiagnosticCode::CatalogBudgetExceeded
            )]
        );

        let oversized_schema = descriptor(
            "oversized",
            "mcp__fixture__oversized",
            json!({
                "type": "object",
                "description": "x".repeat(MCP_RUNTIME_MAX_CATALOG_BYTES + 1)
            }),
        );
        let oversized_invoker =
            MockMcpToolInvoker::returning(vec![oversized_schema], empty_result());
        let oversized_runtime = McpToolRuntime::capture(oversized_invoker);
        assert!(oversized_runtime.is_empty());
        assert_eq!(
            oversized_runtime.initial_diagnostics()[0].code,
            McpToolDiagnosticCode::CatalogBudgetExceeded
        );

        let mut registry = ToolRegistry::empty();
        registry.register_test_tool(TestBuiltinTool {
            name: "trusted_builtin",
            description: "trusted builtin",
        });
        registry.register_mcp_runtime(&runtime);
        assert!(registry.definition_for("trusted_builtin").is_some());
        assert_eq!(registry.definitions().len(), 1);
    }

    #[test]
    fn mcp_name_collision_cannot_replace_builtin_identity_or_definition() {
        let model_name = "mcp__fixture__echo_text";
        let mut registry = ToolRegistry::empty();
        registry.register_test_tool(TestBuiltinTool {
            name: model_name,
            description: "trusted builtin",
        });
        let invoker = MockMcpToolInvoker::returning(
            vec![descriptor(
                "echo_text",
                model_name,
                json!({"type": "object"}),
            )],
            empty_result(),
        );
        let runtime = McpToolRuntime::capture(invoker.clone());

        registry.register_mcp_runtime(&runtime);

        assert_eq!(
            registry.definition_for(model_name).unwrap().description,
            "trusted builtin"
        );
        assert_eq!(
            registry.identity(model_name),
            Some(&AgentToolIdentity::Builtin {
                tool_name: model_name.to_string(),
            })
        );
        assert_eq!(
            registry.mcp_diagnostics(),
            &[McpToolRegistrationDiagnostic::for_tool(
                &provenance("echo_text", model_name),
                McpToolDiagnosticCode::NameCollision,
            )]
        );
        assert_eq!(
            *invoker.diagnostics.lock().unwrap(),
            registry.mcp_diagnostics()
        );
    }

    #[test]
    fn invalid_and_non_object_schemas_are_isolated_from_valid_catalog_entries() {
        let valid_name = "mcp__fixture__valid";
        let invoker = MockMcpToolInvoker::returning(
            vec![
                descriptor(
                    "array_root",
                    "mcp__fixture__array_root",
                    json!({"type": "array", "items": {"type": "string"}}),
                ),
                descriptor(
                    "unsupported_root_keyword",
                    "mcp__fixture__unsupported_root_keyword",
                    json!({
                        "type": "object",
                        "properties": {},
                        "anyOf": [{"required": []}]
                    }),
                ),
                descriptor(
                    "credential_schema",
                    "mcp__fixture__credential_schema",
                    json!({
                        "type": "object",
                        "properties": {"api_token": {"type": "string"}}
                    }),
                ),
                descriptor("valid", valid_name, json!({"type": "object"})),
            ],
            empty_result(),
        );

        let registry = registry_with(invoker);

        assert!(registry.definition_for(valid_name).is_some());
        assert!(registry
            .definition_for("mcp__fixture__array_root")
            .is_none());
        assert!(registry
            .definition_for("mcp__fixture__unsupported_root_keyword")
            .is_none());
        assert_eq!(
            registry
                .mcp_diagnostics()
                .iter()
                .map(|diagnostic| diagnostic.code)
                .collect::<Vec<_>>(),
            vec![
                McpToolDiagnosticCode::InvalidSchema,
                McpToolDiagnosticCode::InvalidSchema,
                McpToolDiagnosticCode::InvalidSchema,
            ]
        );
    }

    #[test]
    fn missing_root_type_is_normalized_to_object() {
        let model_name = "mcp__fixture__implicit_object";
        let registry = registry_with(MockMcpToolInvoker::returning(
            vec![descriptor(
                "implicit_object",
                model_name,
                json!({
                    "properties": {"value": {"type": "integer"}},
                    "required": ["value"],
                    "additionalProperties": false
                }),
            )],
            empty_result(),
        ));

        let definition = registry.definition_for(model_name).unwrap();
        assert_eq!(definition.input_schema["type"], "object");
        assert_eq!(definition.input_schema["required"], json!(["value"]));
        assert!(registry.mcp_diagnostics().is_empty());
    }

    #[test]
    fn server_hints_cannot_replace_host_read_only_authorization() {
        let model_name = "mcp__fixture__mutating";
        let mut untrusted = descriptor("mutating", model_name, json!({"type": "object"}));
        untrusted.annotations.read_only_hint = None;
        let mut server_claim_only = descriptor(
            "server_claim_only",
            "mcp__fixture__server_claim_only",
            json!({"type": "object"}),
        );
        server_claim_only.host_allows_unapproved_read_only_invocation = false;
        let mut destructive = descriptor(
            "destructive",
            "mcp__fixture__destructive",
            json!({"type": "object"}),
        );
        destructive.annotations.destructive_hint = Some(true);
        let registry = registry_with(MockMcpToolInvoker::returning(
            vec![untrusted, server_claim_only, destructive],
            empty_result(),
        ));

        assert!(registry.definition_for(model_name).is_none());
        assert!(registry
            .definition_for("mcp__fixture__server_claim_only")
            .is_none());
        assert!(registry
            .definition_for("mcp__fixture__destructive")
            .is_none());
        assert_eq!(
            registry
                .mcp_diagnostics()
                .iter()
                .map(|diagnostic| diagnostic.code)
                .collect::<Vec<_>>(),
            vec![
                McpToolDiagnosticCode::ApprovalRequiredUnsupported,
                McpToolDiagnosticCode::ApprovalRequiredUnsupported,
                McpToolDiagnosticCode::ApprovalRequiredUnsupported,
            ]
        );
    }

    #[test]
    fn invocation_debug_redacts_arguments_and_call_projections_remove_credentials() {
        let model_name = "mcp__fixture__credential_projection";
        let invoker = MockMcpToolInvoker::returning(
            vec![descriptor(
                "credential_projection",
                model_name,
                json!({"type": "object"}),
            )],
            empty_result(),
        );
        let registry = registry_with(invoker);
        let secret = "fixture-secret-that-must-not-appear";
        let call = call(
            model_name,
            json!({
                "query": "safe",
                "api_token": secret,
                "nested": {"password": secret},
            }),
        );

        for projected in [
            registry.trace_call_projection(&call),
            registry.event_call_projection(&call),
            registry.model_call_projection(&call),
        ] {
            assert_eq!(projected.args, json!({}));
            assert_eq!(projected.reason, None);
            assert!(!serde_json::to_string(&projected).unwrap().contains(secret));
        }
        assert_eq!(
            registry.checkpoint_persistence(model_name),
            AgentToolCallCheckpointPersistence::DeniedMcp
        );

        let raw_result = crate::protocol::AgentToolResult {
            call_id: call.id.clone(),
            tool: model_name.to_string(),
            ok: true,
            result: Some(json!({
                "content": [{"type": "text", "text": secret}],
                "provenance": provenance("credential_projection", model_name),
            })),
            error: None,
            exact_archive_file: None,
        };
        assert!(
            serde_json::to_string(&registry.model_projection(&raw_result))
                .unwrap()
                .contains(secret)
        );
        for durable in [
            registry.trace_projection(&raw_result),
            registry.archive_projection(&raw_result),
            registry.checkpoint_projection(&raw_result),
        ] {
            let rendered = serde_json::to_string(&durable).unwrap();
            assert!(!rendered.contains(secret));
            assert!(rendered.contains("externalToolOutputNotPersisted"));
        }

        let invocation = McpToolInvocation {
            provenance: provenance("credential_projection", model_name),
            arguments: call.args,
            caller: McpToolCatalogContext::default(),
        };
        assert!(!format!("{invocation:?}").contains(secret));
    }

    #[tokio::test]
    async fn structured_content_and_typed_route_survive_successful_execution() {
        let model_name = "mcp__fixture__structured";
        let expected_provenance = provenance("structured_result", model_name);
        let invoker = MockMcpToolInvoker::returning(
            vec![descriptor(
                "structured_result",
                model_name,
                json!({
                    "type": "object",
                    "properties": {"left": {"type": "integer"}},
                    "additionalProperties": false
                }),
            )],
            McpToolInvocationResult {
                content: vec![McpToolContentBlock::Text {
                    text: "sum ready".to_string(),
                }],
                structured_content: Some(json!({"sum": 5, "operands": [2, 3]})),
                is_error: false,
                truncated_at_source: false,
            },
        );

        let result = execute(
            registry_with(invoker.clone()),
            model_name,
            json!({"left": 2}),
        )
        .await
        .unwrap();

        assert!(result.ok);
        let value = result.result.as_ref().unwrap();
        assert_eq!(value["content"][0]["text"], "sum ready");
        assert_eq!(
            value["structuredContent"],
            json!({"sum": 5, "operands": [2, 3]})
        );
        assert_eq!(value["isError"], false);
        assert_eq!(
            value["provenance"],
            serde_json::to_value(&expected_provenance).unwrap()
        );
        let projection_registry = registry_with(invoker.clone());
        let trace_projection = projection_registry.trace_projection(&result);
        assert_eq!(
            trace_projection.result.as_ref().unwrap()["provenance"],
            serde_json::to_value(&expected_provenance).unwrap()
        );
        let model_projection = projection_registry.model_projection(&result);
        assert!(
            model_projection
                .result
                .as_ref()
                .unwrap()
                .get("provenance")
                .is_none(),
            "server identity, config digest and generation must not enter the model observation"
        );
        let projected_json = serde_json::to_string(&model_projection).unwrap();
        assert!(!projected_json.contains(&expected_provenance.server_id));
        assert!(!projected_json.contains(&expected_provenance.config_digest));
        let invocations = invoker.invocations.lock().unwrap();
        assert_eq!(invocations.len(), 1);
        assert_eq!(invocations[0].provenance, expected_provenance);
        assert_eq!(invocations[0].arguments, json!({"left": 2}));
    }

    #[tokio::test]
    async fn server_is_error_becomes_failed_tool_result_not_transport_failure() {
        let model_name = "mcp__fixture__tool_error";
        let invoker = MockMcpToolInvoker::returning(
            vec![descriptor(
                "return_tool_error",
                model_name,
                json!({"type": "object"}),
            )],
            McpToolInvocationResult {
                content: vec![McpToolContentBlock::Text {
                    text: "fixture rejected the request".to_string(),
                }],
                structured_content: Some(json!({"reason": "fixture_error"})),
                is_error: true,
                truncated_at_source: false,
            },
        );

        let result = execute(registry_with(invoker), model_name, json!({}))
            .await
            .expect("isError is a settled Tool result, not a transport error");

        assert!(!result.ok);
        assert!(result.error.unwrap().contains("MCP server reported"));
        let value = result.result.expect("structured MCP error result");
        assert_eq!(value["isError"], true);
        assert_eq!(value["content"][0]["text"], "fixture rejected the request");
        assert_eq!(value["structuredContent"]["reason"], "fixture_error");
        assert_eq!(value["errorCode"], "mcp.tool_execution_error");
    }

    #[tokio::test]
    async fn non_object_arguments_fail_before_invoker_is_called() {
        let model_name = "mcp__fixture__echo_text";
        let invoker = MockMcpToolInvoker::returning(
            vec![descriptor(
                "echo_text",
                model_name,
                json!({"type": "object"}),
            )],
            empty_result(),
        );

        let result = execute(
            registry_with(invoker.clone()),
            model_name,
            json!(["not", "an", "object"]),
        )
        .await
        .expect("invalid model arguments are a settled Tool result");

        assert!(!result.ok);
        assert!(invoker.invocations.lock().unwrap().is_empty());
        assert_eq!(
            result.result.unwrap()["errorCode"],
            "mcp.invalid_tool_arguments"
        );
    }

    #[tokio::test]
    async fn runtime_cancellation_token_is_passed_to_and_stops_invoker() {
        let model_name = "mcp__fixture__slow";
        let invoker = MockMcpToolInvoker::waiting_for_cancellation(vec![descriptor(
            "slow_tool",
            model_name,
            json!({"type": "object"}),
        )]);
        let registry = registry_with(invoker.clone());
        let cancellation = AgentCancellationToken::new();
        let context =
            ToolExecutionContext::from_run_context(None).with_cancellation(cancellation.clone());
        let task = tokio::spawn(Arc::new(registry).execute_async(
            context,
            call(model_name, json!({})),
            cancellation.clone(),
        ));

        tokio::time::timeout(Duration::from_secs(1), async {
            while invoker.cancellation_tokens.lock().unwrap().is_empty() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("mock invoker should start");
        let received = invoker.cancellation_tokens.lock().unwrap()[0].clone();
        assert!(received.shares_state_with(&cancellation));

        cancellation.cancel();
        let error = task
            .await
            .expect("execution task should not panic")
            .expect_err("cancelled MCP execution must cancel the Agent run");
        assert!(error.is_cancelled());
    }

    #[tokio::test]
    async fn oversized_text_and_structured_content_are_bounded_with_diagnostics() {
        let model_name = "mcp__fixture__large_result";
        let oversized_text = format!(
            "{}TAIL_MUST_NOT_SURVIVE",
            "x".repeat(MAX_MCP_TEXT_RESULT_BYTES + 1_024)
        );
        let oversized_structured =
            json!({"blob": "y".repeat(MAX_MCP_STRUCTURED_RESULT_BYTES + 1_024)});
        let invoker = MockMcpToolInvoker::returning(
            vec![descriptor(
                "large_result",
                model_name,
                json!({"type": "object"}),
            )],
            McpToolInvocationResult {
                content: vec![McpToolContentBlock::Text {
                    text: oversized_text,
                }],
                structured_content: Some(oversized_structured),
                is_error: false,
                truncated_at_source: false,
            },
        );

        let result = execute(registry_with(invoker), model_name, json!({}))
            .await
            .unwrap();
        let value = result.result.unwrap();
        let retained_text = value["content"][0]["text"].as_str().unwrap();

        assert!(result.ok);
        assert!(!retained_text.contains("TAIL_MUST_NOT_SURVIVE"));
        assert!(retained_text.len() <= MAX_MCP_TEXT_RESULT_BYTES + 128);
        assert_eq!(value["diagnostics"]["textTruncated"], true);
        assert_eq!(value["diagnostics"]["structuredContentTruncated"], true);
        assert_eq!(
            value["structuredContent"]["_mycopilot"]["reason"],
            "structured_content_limit"
        );
        assert!(
            value["structuredContent"]["_mycopilot"]["originalBytes"]
                .as_u64()
                .unwrap()
                > MAX_MCP_STRUCTURED_RESULT_BYTES as u64
        );
    }

    #[test]
    fn dynamic_mcp_registration_does_not_change_stable_tool_revision() {
        let mut registry = ToolRegistry::empty();
        registry.register_test_tool(TestBuiltinTool {
            name: "builtin_stable",
            description: "stable definition",
        });
        let before = EffectiveToolSet::from_permitted_definitions(
            &registry,
            registry.definitions(),
            &BTreeSet::new(),
        )
        .unwrap();
        let runtime = McpToolRuntime::capture(MockMcpToolInvoker::returning(
            vec![descriptor(
                "echo_text",
                "mcp__fixture__echo_text",
                json!({"type": "object"}),
            )],
            empty_result(),
        ));
        registry.register_mcp_runtime(&runtime);
        let after = EffectiveToolSet::from_permitted_definitions(
            &registry,
            registry.definitions(),
            &BTreeSet::new(),
        )
        .unwrap();

        assert_eq!(before.stable_revision(), after.stable_revision());
        assert_ne!(before.dynamic_revision(), after.dynamic_revision());
        assert_eq!(
            after
                .stable_definitions()
                .iter()
                .map(|definition| definition.name.as_str())
                .collect::<Vec<_>>(),
            vec!["builtin_stable"]
        );
        assert_eq!(
            after
                .dynamic_definitions()
                .iter()
                .map(|definition| definition.name.as_str())
                .collect::<Vec<_>>(),
            vec!["mcp__fixture__echo_text"]
        );

        let mut changed_descriptor = descriptor(
            "echo_text",
            "mcp__fixture__echo_text",
            json!({"type": "object"}),
        );
        changed_descriptor.provenance.catalog_generation += 1;
        changed_descriptor.provenance.catalog_digest = "c".repeat(64);
        let mut changed_registry = ToolRegistry::empty();
        changed_registry.register_test_tool(TestBuiltinTool {
            name: "builtin_stable",
            description: "stable definition",
        });
        changed_registry.register_mcp_runtime(&McpToolRuntime::capture(
            MockMcpToolInvoker::returning(vec![changed_descriptor], empty_result()),
        ));
        let changed = EffectiveToolSet::from_permitted_definitions(
            &changed_registry,
            changed_registry.definitions(),
            &BTreeSet::new(),
        )
        .unwrap();
        assert_eq!(after.stable_revision(), changed.stable_revision());
        assert_ne!(after.dynamic_revision(), changed.dynamic_revision());
    }
}
