//! Public MCP runtime contracts, bounded content types, and registration diagnostics.

use super::*;

/// Host-owned hard limits for the MCP values that may cross into Agent/provider projections.
///
/// Deserialization supplies the conservative defaults for omitted fields and rejects unknown
/// fields so a misspelled security setting cannot be silently ignored. `validate` permits a Host
/// to tighten a limit, but never to exceed the audited hard maxima represented by `SAFE_DEFAULT`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields, rename_all = "camelCase")]
pub struct McpRuntimeProjectionLimits {
    pub max_tool_definitions: usize,
    pub max_catalog_bytes: usize,
    pub max_model_tool_name_bytes: usize,
    pub max_raw_tool_name_bytes: usize,
    pub max_provider_description_bytes: usize,
    pub max_provider_schema_bytes: usize,
    pub max_model_text_bytes: usize,
    pub max_model_structured_bytes: usize,
    pub max_model_structured_depth: usize,
    pub max_model_structured_nodes: usize,
    pub max_content_blocks: usize,
    pub max_server_display_name_bytes: usize,
    pub max_raw_arguments_bytes: usize,
    pub max_argument_depth: usize,
    pub max_argument_nodes: usize,
    pub max_argument_object_properties: usize,
    pub max_argument_summary_depth: usize,
    pub max_argument_summary_nodes: usize,
    pub max_mime_type_bytes: usize,
}

impl McpRuntimeProjectionLimits {
    pub const SAFE_DEFAULT: Self = Self {
        max_tool_definitions: 64,
        max_catalog_bytes: 128 * 1_024,
        max_model_tool_name_bytes: 64,
        max_raw_tool_name_bytes: 1_024,
        max_provider_description_bytes: 1_024,
        max_provider_schema_bytes: 256 * 1_024,
        max_model_text_bytes: 16 * 1_024,
        max_model_structured_bytes: 8 * 1_024,
        max_model_structured_depth: 32,
        max_model_structured_nodes: 4_096,
        max_content_blocks: 128,
        max_server_display_name_bytes: 128,
        max_raw_arguments_bytes: 64 * 1_024,
        max_argument_depth: 32,
        max_argument_nodes: 4_096,
        max_argument_object_properties: 256,
        max_argument_summary_depth: 32,
        max_argument_summary_nodes: 4_096,
        max_mime_type_bytes: 128,
    };

    /// Rejects zero/invalid budgets and any attempt to weaken the audited Host hard limits.
    pub fn validate(&self) -> AgentResult<()> {
        let configured = [
            self.max_tool_definitions,
            self.max_catalog_bytes,
            self.max_model_tool_name_bytes,
            self.max_raw_tool_name_bytes,
            self.max_provider_description_bytes,
            self.max_provider_schema_bytes,
            self.max_model_text_bytes,
            self.max_model_structured_bytes,
            self.max_model_structured_depth,
            self.max_model_structured_nodes,
            self.max_content_blocks,
            self.max_server_display_name_bytes,
            self.max_raw_arguments_bytes,
            self.max_argument_depth,
            self.max_argument_nodes,
            self.max_argument_object_properties,
            self.max_argument_summary_depth,
            self.max_argument_summary_nodes,
            self.max_mime_type_bytes,
        ];
        let hard_maxima = [
            Self::SAFE_DEFAULT.max_tool_definitions,
            Self::SAFE_DEFAULT.max_catalog_bytes,
            Self::SAFE_DEFAULT.max_model_tool_name_bytes,
            Self::SAFE_DEFAULT.max_raw_tool_name_bytes,
            Self::SAFE_DEFAULT.max_provider_description_bytes,
            Self::SAFE_DEFAULT.max_provider_schema_bytes,
            Self::SAFE_DEFAULT.max_model_text_bytes,
            Self::SAFE_DEFAULT.max_model_structured_bytes,
            Self::SAFE_DEFAULT.max_model_structured_depth,
            Self::SAFE_DEFAULT.max_model_structured_nodes,
            Self::SAFE_DEFAULT.max_content_blocks,
            Self::SAFE_DEFAULT.max_server_display_name_bytes,
            Self::SAFE_DEFAULT.max_raw_arguments_bytes,
            Self::SAFE_DEFAULT.max_argument_depth,
            Self::SAFE_DEFAULT.max_argument_nodes,
            Self::SAFE_DEFAULT.max_argument_object_properties,
            Self::SAFE_DEFAULT.max_argument_summary_depth,
            Self::SAFE_DEFAULT.max_argument_summary_nodes,
            Self::SAFE_DEFAULT.max_mime_type_bytes,
        ];
        if configured
            .into_iter()
            .zip(hard_maxima)
            .any(|(value, hard_maximum)| value == 0 || value > hard_maximum)
        {
            return Err(AgentError::structured(
                "mcp.invalid_runtime_projection_limits",
                "MCP runtime projection limits must be non-zero and no greater than the Host hard limits.",
                json!({
                    "type": "mcp_security_policy",
                    "code": "invalidRuntimeProjectionLimits",
                    "retryable": false,
                }),
            ));
        }
        Ok(())
    }
}

impl Default for McpRuntimeProjectionLimits {
    fn default() -> Self {
        Self::SAFE_DEFAULT
    }
}

pub type McpToolInvocationFuture<'a> =
    Pin<Box<dyn Future<Output = AgentResult<McpToolInvocationResult>> + Send + 'a>>;

/// Identity of the exact normalized MCP input schema exposed to a model provider.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpNormalizedInputSchemaIdentity {
    pub schema_digest: String,
    pub normalizer_version: u32,
}

/// Computes the versioned identity of the exact input schema Captain Who would expose to a model.
///
/// The raw Catalog digest is intentionally not accepted here: the same raw schema can produce a
/// different Provider-facing schema when Host normalization rules evolve.
pub fn mcp_normalized_input_schema_identity(
    model_tool_name: &str,
    input_schema: &Value,
) -> AgentResult<McpNormalizedInputSchemaIdentity> {
    let normalized = normalize_input_schema_value(model_tool_name, input_schema.clone())?;
    normalized_input_schema_identity(&normalized)
}

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
    /// Host-owned invocation policy captured with this exact Catalog/config snapshot.
    pub approval_mode: AgentMcpApprovalMode,
    /// Host-supplied display label. It is normalized and bounded before entering an approval DTO.
    pub server_display_name: String,
    pub description: Option<String>,
    pub input_schema: Value,
    /// Retained for validation, provenance and future output handling. It is intentionally not
    /// projected into either provider's Tool definition in this round.
    pub output_schema: Option<Value>,
    pub annotations: McpAgentToolAnnotations,
}

impl fmt::Debug for McpAgentToolDescriptor {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("McpAgentToolDescriptor")
            .field("provenance", &self.provenance)
            .field("approval_mode", &self.approval_mode)
            .field("server_display_name_bytes", &self.server_display_name.len())
            .field(
                "description_bytes",
                &self.description.as_ref().map(String::len),
            )
            .field("input_schema", &"<redacted>")
            .field("has_output_schema", &self.output_schema.is_some())
            .field("annotations", &self.annotations)
            .finish()
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct McpToolCatalogContext {
    /// Backend-authoritative active project. A project-scoped Server is never visible without an
    /// exact match. Plugin visibility remains fail-closed until the Host supplies plugin context.
    pub project_id: Option<String>,
}

/// Non-serializable preparation request delivered to the trusted Host before an approval is
/// published.
///
/// The Host must seal `arguments`, keyed by the high-entropy invocation id, before returning the
/// safe approval DTO. The Host may only replace `payload_persistence` with the actual backend
/// capability; every other frozen field must remain unchanged. Neither this request nor its raw
/// arguments may be placed in an AgentEvent, trace, checkpoint, log, or error.
pub struct McpToolApprovalRequest {
    pub(super) approval: AgentMcpToolApproval,
    pub(super) arguments: Value,
    pub(super) caller: McpToolCatalogContext,
}

impl McpToolApprovalRequest {
    pub fn approval(&self) -> &AgentMcpToolApproval {
        &self.approval
    }

    pub fn arguments(&self) -> &Value {
        &self.arguments
    }

    pub fn caller(&self) -> &McpToolCatalogContext {
        &self.caller
    }

    pub fn into_parts(self) -> (AgentMcpToolApproval, Value, McpToolCatalogContext) {
        (self.approval, self.arguments, self.caller)
    }
}

impl fmt::Debug for McpToolApprovalRequest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("McpToolApprovalRequest")
            .field("identity", &self.approval.identity)
            .field("arguments", &"<redacted>")
            .field("caller_project_bound", &self.caller.project_id.is_some())
            .finish()
    }
}

/// One-time execution request for a Host-prepared MCP approval.
///
/// Raw arguments are deliberately absent. The Host retrieves and consumes its sealed payload by
/// `invocation_id`, revalidates the complete frozen identity, and must never replay a consumed or
/// outcome-unknown invocation.
#[derive(Clone, Debug, PartialEq)]
pub struct McpApprovedToolInvocation {
    pub approval: AgentMcpToolApproval,
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

    /// Seals raw arguments before the safe approval crosses a persistence or presentation boundary.
    fn prepare_approval(
        &self,
        _request: McpToolApprovalRequest,
    ) -> AgentResult<AgentMcpToolApproval> {
        Err(approval_host_unavailable("prepareApprovalUnavailable"))
    }

    /// Idempotently discards a prepared but unconsumed sealed payload.
    ///
    /// Hosts call this when pending persistence fails, the user rejects, the approval expires, or a
    /// stale binding is invalidated. A Host that supports preparation must override this hook.
    fn invalidate_prepared_approval(
        &self,
        _identity: &AgentMcpToolInvocationIdentity,
    ) -> AgentResult<()> {
        Err(approval_host_unavailable("invalidateApprovalUnavailable"))
    }

    /// Revalidates one frozen approval while it is still definitely not dispatched.
    ///
    /// The Host must verify current catalog/config/schema identity and prove that the sealed
    /// payload is present, unexpired, authenticated, and argument-bound. Execution hosts call
    /// this immediately before their durable `Dispatching` CAS; `invoke_approved` repeats the
    /// checks as the final TOCTOU defense.
    fn revalidate_approved(&self, _approval: &AgentMcpToolApproval) -> AgentResult<()> {
        Err(approval_host_unavailable("approvalRevalidationUnavailable"))
    }

    /// Consumes one previously prepared approval and invokes it at most once.
    fn invoke_approved<'a>(
        &'a self,
        _invocation: McpApprovedToolInvocation,
        cancellation: AgentCancellationToken,
    ) -> McpToolInvocationFuture<'a> {
        Box::pin(async move {
            if cancellation.is_cancelled() {
                return Err(AgentError::cancelled());
            }
            Err(approval_host_unavailable("approvedInvocationUnavailable"))
        })
    }

    fn report_diagnostics(&self, _diagnostics: &[McpToolRegistrationDiagnostic]) {}
}

fn approval_host_unavailable(code: &'static str) -> AgentError {
    AgentError::structured(
        "mcp.approval_host_unavailable",
        "The MCP approval Host boundary is unavailable.",
        json!({
            "type": "mcp_approval",
            "code": code,
            "retryable": false,
        }),
    )
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
    pub(super) fn global(code: McpToolDiagnosticCode) -> Self {
        Self {
            server_id: None,
            model_tool_name: None,
            code,
            message: diagnostic_message(code).to_string(),
        }
    }

    pub(super) fn for_tool(
        provenance: &AgentMcpToolProvenance,
        code: McpToolDiagnosticCode,
    ) -> Self {
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

    pub(in crate::tools) fn name_collision(provenance: &AgentMcpToolProvenance) -> Self {
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
