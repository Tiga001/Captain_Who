use crate::protocol::AgentGuidanceStatus;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone)]
pub struct AgentUsageRecordInsert {
    pub id: String,
    pub conversation_id: String,
    pub message_id: String,
    pub run_id: String,
    pub project_id: Option<String>,
    pub model_id: String,
    pub model_name: String,
    pub started_at: Option<i64>,
    pub completed_at: Option<i64>,
    pub status: Option<String>,
    pub error: Option<String>,
    pub created_at: i64,
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub output_thinking_tokens: Option<u64>,
    pub total_tokens: Option<u64>,
    pub cached_input_tokens: Option<u64>,
    pub cache_creation_input_tokens: Option<u64>,
    pub billable_request_count: u64,
    pub input_price: Option<String>,
    pub cached_input_price: Option<String>,
    pub output_price: Option<String>,
    pub estimated_cost: Option<f64>,
}

/// A maintenance operation owns its lifecycle and billing independently of chat messages.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ManualContextCompactionOperation {
    pub operation_id: String,
    pub request_id: String,
    pub conversation_id: String,
    pub status: String,
    pub phase: String,
    pub assistant_message_id: Option<String>,
    pub covered_through_message_id: Option<String>,
    pub model_id: Option<String>,
    pub summary_id: Option<String>,
    pub source_input_tokens: Option<u64>,
    pub replacement_input_tokens: Option<u64>,
    pub error: Option<String>,
    pub started_at: i64,
    pub updated_at: i64,
    pub completed_at: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ManualContextCompactionUsageRecord {
    pub operation_id: String,
    pub conversation_id: String,
    pub project_id: Option<String>,
    pub model_id: String,
    pub model_name: String,
    pub started_at: Option<i64>,
    pub completed_at: Option<i64>,
    pub status: Option<String>,
    pub error: Option<String>,
    pub created_at: i64,
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub output_thinking_tokens: Option<u64>,
    pub total_tokens: Option<u64>,
    pub cached_input_tokens: Option<u64>,
    pub cache_creation_input_tokens: Option<u64>,
    pub billable_request_count: u64,
    pub input_price: Option<String>,
    pub cached_input_price: Option<String>,
    pub output_price: Option<String>,
    pub estimated_cost: Option<f64>,
}

#[derive(Clone)]
pub struct AgentActionAuditRecord {
    pub action_id: String,
    pub run_id: String,
    pub conversation_id: Option<String>,
    pub assistant_message_id: Option<String>,
    pub action_type: String,
    pub tool_name: String,
    pub decision: Option<String>,
    pub status: String,
    pub action_json: String,
    pub file_change_result_json: Option<String>,
    pub command_result_json: Option<String>,
    pub tool_result_json: Option<String>,
    pub error: Option<String>,
    pub created_at: i64,
    pub decided_at: Option<i64>,
    pub completed_at: Option<i64>,
    pub effective_permissions_json: Option<String>,
    pub path_scope: Option<String>,
    pub command_cwd_scope: Option<String>,
    pub blocked_reason: Option<String>,
    pub decision_source: Option<String>,
}

impl std::fmt::Debug for AgentActionAuditRecord {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("AgentActionAuditRecord([REDACTED])")
    }
}

/// Persisted file-producing action whose process outcome cannot be proven after restart.
///
/// An `executing` claim is intentionally treated as effects-may-have-occurred. Conversation or
/// project deletion must not erase it until a separate recovery workflow settles the receipt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentUnsettledFileEffect {
    pub project_id: Option<String>,
    pub conversation_id: String,
    pub run_id: String,
    pub action_id: String,
}

#[derive(Clone, PartialEq, Eq)]
pub struct AgentPendingActionRecord {
    pub action_id: String,
    pub run_id: String,
    pub conversation_id: Option<String>,
    pub assistant_message_id: Option<String>,
    pub action_type: String,
    pub tool_name: String,
    pub tool_call_id: Option<String>,
    pub status: String,
    /// Durable write-ahead outcome produced by the action executor.
    ///
    /// This is intentionally independent from the assistant continuation run status: a failed
    /// action may still be followed by a successfully persisted assistant explanation. Startup
    /// reconciliation uses this value when a crash occurs before the lifecycle CAS is committed.
    pub target_status: Option<String>,
    pub action_json: String,
    pub agent_input_json: String,
    pub created_at: i64,
    pub updated_at: i64,
}

impl std::fmt::Debug for AgentPendingActionRecord {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("AgentPendingActionRecord([REDACTED])")
    }
}

/// Authenticated ciphertext envelope for one pending MCP approval payload.
///
/// Core storage accepts only the opaque invocation/action binding, the AEAD nonce and ciphertext,
/// the digest of Host-reconstructed authenticated metadata, and lifecycle timestamps. It cannot
/// accept raw MCP arguments, the authenticated metadata itself, encryption-key references, or a
/// serializable plaintext payload type.
#[derive(Clone, PartialEq, Eq)]
pub struct McpApprovalEnvelopeRecord {
    pub invocation_id: String,
    pub action_id: String,
    pub envelope_version: i64,
    pub nonce_base64: String,
    pub ciphertext_base64: String,
    pub aad_digest: String,
    pub created_at: i64,
    pub expires_at: i64,
}

impl std::fmt::Debug for McpApprovalEnvelopeRecord {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("McpApprovalEnvelopeRecord([REDACTED])")
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentRunGuidanceRecord {
    pub guidance_id: String,
    pub client_message_id: String,
    pub run_id: String,
    pub conversation_id: String,
    pub assistant_message_id: String,
    pub content: String,
    pub status: AgentGuidanceStatus,
    pub attachment_ids: Vec<String>,
    /// JSON array of Host-owned folder references. Kept path-bearing in storage, sanitized when
    /// projected to model context.
    pub folder_references_json: String,
    pub applied_trace_sequence: Option<u64>,
    pub terminal_reason: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Clone, PartialEq, Eq)]
pub struct AgentFileChangeRecord {
    pub schema_version: u32,
    pub id: String,
    pub conversation_id: String,
    pub project_id: Option<String>,
    pub run_id: String,
    pub source_tool_name: String,
    pub source_tool_call_id: String,
    pub source_tool_arguments_digest: String,
    pub permission_revision: String,
    pub tool_set_revision: String,
    pub provider_wire_revision: String,
    pub observation_id: String,
    pub observation_json: String,
    pub file_path: String,
    pub operation: String,
    pub strategy: Option<String>,
    pub status: String,
    pub base_revision: Option<String>,
    pub base_content: String,
    pub content: String,
    pub draft_revision: u64,
    pub next_mutation_index: u64,
    pub additions: u64,
    pub deletions: u64,
    pub line_count: u64,
    pub byte_count: u64,
    pub mutation_count: u64,
    pub stats_final: bool,
    pub summary: Option<String>,
    pub final_action_id: Option<String>,
    pub final_action_arguments_digest: Option<String>,
    pub final_permission_revision: Option<String>,
    pub final_tool_set_revision: Option<String>,
    pub final_provider_wire_revision: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
    pub expires_at: i64,
}

impl std::fmt::Debug for AgentFileChangeRecord {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("AgentFileChangeRecord([REDACTED])")
    }
}

/// Metadata needed to render and enforce the current Run's FileTransactionState.
///
/// This projection deliberately excludes draft bodies, observations, and action bindings. It is
/// not an authorization record; mutations still load and validate the full transaction record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentFileChangeRuntimeState {
    pub id: String,
    pub conversation_id: String,
    pub project_id: Option<String>,
    pub run_id: String,
    pub source_tool_name: String,
    pub file_path: String,
    pub operation: String,
    pub strategy: Option<String>,
    pub status: String,
    pub draft_revision: u64,
    pub next_mutation_index: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentFileChangeChunkRecord {
    pub transaction_id: String,
    pub mutation_index: u64,
    pub content_digest: String,
    pub byte_count: u64,
    pub created_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentFileChangeOperationRecord {
    pub transaction_id: String,
    pub mutation_index: u64,
    pub source_tool_call_id: String,
    pub source_tool_arguments_digest: String,
    pub action: String,
    pub payload_digest: String,
    pub draft_revision: u64,
    pub receipt_json: String,
    pub created_at: i64,
}
