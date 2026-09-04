use super::*;

/// Version of the presentation-safe image-generation result returned by the Agent Tool.
///
/// This contract deliberately excludes provider endpoints, credentials, ephemeral or signed
/// output URLs, managed-store paths, and input image bytes. Those values remain inside the
/// configuration, execution, and Artifact boundaries respectively.
pub const AGENT_IMAGE_GENERATION_RESULT_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Deserialize, Serialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum AgentImageGenerationResultStatus {
    Succeeded,
    Failed,
    Cancelled,
    OutcomeIndeterminate,
    CommitIndeterminate,
}

/// Model-visible image operation. Provider status probes are intentionally not Agent results.
#[derive(Debug, Deserialize, Serialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum AgentImageGenerationOperation {
    Generate,
    Edit,
}

#[derive(Debug, Deserialize, Serialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum AgentImageGenerationArtifactKind {
    Image,
}

/// Immutable, presentation-safe identity for one successfully published generated image.
///
/// The URI is an application-owned `image-artifact://` capability, never a provider URL or a
/// filesystem path. A terminal result may expose this structure only when publication completed
/// successfully.
#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AgentImageGenerationArtifact {
    pub artifact_id: String,
    pub uri: String,
    pub kind: AgentImageGenerationArtifactKind,
    pub format: crate::image_generation::ImageArtifactFormat,
    pub mime_type: String,
    pub width: u32,
    pub height: u32,
    pub size_bytes: u64,
    pub sha256: String,
}

/// Correlation and execution metadata safe to persist in Agent traces and emit to clients.
#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AgentImageGenerationAudit {
    pub execution_id: String,
    pub request_fingerprint: String,
    pub provider_profile_id: String,
    pub adapter_id: String,
    pub profile_revision: u64,
    pub model_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_request_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub http_status: Option<u16>,
    pub created_at: i64,
    pub completed_at: i64,
    pub duration_ms: u64,
}

/// Stable failure details and explicit uncertainty markers for terminal execution outcomes.
#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AgentImageGenerationFailure {
    pub code: crate::image_generation::ImageGenerationExecutionFailureCode,
    pub phase: crate::image_generation::ImageGenerationExecutionPhase,
    pub message: String,
    pub recovery: String,
    pub retryable: bool,
    pub generation_may_have_succeeded: bool,
    pub provider_succeeded: bool,
    pub artifact_commit_may_have_succeeded: bool,
}

/// Terminal Tool result for image generation and image editing.
///
/// `succeeded` requires exactly one `artifact` and no `failure`. Every other status requires a
/// `failure` and must not contain an `artifact`. Producers enforce this invariant before the
/// result is emitted or persisted.
#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AgentImageGenerationResult {
    pub schema_version: u32,
    pub status: AgentImageGenerationResultStatus,
    pub operation: AgentImageGenerationOperation,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub artifact: Option<AgentImageGenerationArtifact>,
    pub audit: AgentImageGenerationAudit,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub failure: Option<AgentImageGenerationFailure>,
}

/// Frozen Renderer presentation for one exact Direct or Staged FileChange.
///
/// `inline_diff` is present only for a current Direct change whose complete authoritative diff is
/// safely bounded. A Staged change keeps it `null`; the Renderer must page the frozen diff through
/// the authenticated transaction-scoped RPC instead of treating a truncated preview as approval
/// authority.
#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentFileChangeProposal {
    #[serde(deserialize_with = "deserialize_file_change_schema_version")]
    pub schema_version: u32,
    pub id: String,
    pub transaction_id: String,
    pub operation: AgentFileChangeOperation,
    #[serde(deserialize_with = "deserialize_required_nullable")]
    pub update_strategy: Option<AgentFileChangeUpdateStrategy>,
    pub file_path: String,
    #[serde(deserialize_with = "deserialize_required_nullable")]
    pub inline_diff: Option<AgentGitDiffSnapshot>,
    #[serde(deserialize_with = "deserialize_required_nullable")]
    pub base_revision: Option<String>,
    #[serde(deserialize_with = "deserialize_required_nullable")]
    pub summary: Option<String>,
    pub additions: u64,
    pub deletions: u64,
    pub line_count: u64,
    pub byte_count: u64,
    pub approval_status: AgentApprovalStatus,
    /// Host-private, versioned execution authority for Direct and Staged changes.
    ///
    /// Core Server removes this field from every Renderer projection. Presentation fields above
    /// are never sufficient authority for execution.
    pub execution: Box<crate::file_change::FileChangeDirectBinding>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct AgentFileChangeProposalWire {
    #[serde(deserialize_with = "deserialize_file_change_schema_version")]
    schema_version: u32,
    id: String,
    transaction_id: String,
    operation: AgentFileChangeOperation,
    #[serde(deserialize_with = "deserialize_required_nullable")]
    update_strategy: Option<AgentFileChangeUpdateStrategy>,
    file_path: String,
    #[serde(deserialize_with = "deserialize_required_nullable")]
    inline_diff: Option<AgentGitDiffSnapshot>,
    #[serde(deserialize_with = "deserialize_required_nullable")]
    base_revision: Option<String>,
    #[serde(deserialize_with = "deserialize_required_nullable")]
    summary: Option<String>,
    additions: u64,
    deletions: u64,
    line_count: u64,
    byte_count: u64,
    approval_status: AgentApprovalStatus,
    execution: Box<crate::file_change::FileChangeDirectBinding>,
}

impl<'de> Deserialize<'de> for AgentFileChangeProposal {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let wire = AgentFileChangeProposalWire::deserialize(deserializer)?;
        let proposal = Self {
            schema_version: wire.schema_version,
            id: wire.id,
            transaction_id: wire.transaction_id,
            operation: wire.operation,
            update_strategy: wire.update_strategy,
            file_path: wire.file_path,
            inline_diff: wire.inline_diff,
            base_revision: wire.base_revision,
            summary: wire.summary,
            additions: wire.additions,
            deletions: wire.deletions,
            line_count: wire.line_count,
            byte_count: wire.byte_count,
            approval_status: wire.approval_status,
            execution: wire.execution,
        };
        proposal.validate().map_err(serde::de::Error::custom)?;
        Ok(proposal)
    }
}

impl AgentFileChangeProposal {
    pub fn validate(&self) -> Result<(), &'static str> {
        let domain_operation = match self.operation {
            AgentFileChangeOperation::Create => crate::file_change::FileChangeOperation::Create,
            AgentFileChangeOperation::Update => crate::file_change::FileChangeOperation::Update,
            AgentFileChangeOperation::Delete => crate::file_change::FileChangeOperation::Delete,
        };
        let staged = self.execution.staged_transaction_id.is_some();
        let strategy_valid = match self.operation {
            AgentFileChangeOperation::Update if staged => self.update_strategy.is_some(),
            AgentFileChangeOperation::Update => self.update_strategy.is_none(),
            AgentFileChangeOperation::Create | AgentFileChangeOperation::Delete => {
                self.update_strategy.is_none()
            }
        };
        let base_revision_valid = match self.operation {
            AgentFileChangeOperation::Create => self.base_revision.is_none(),
            AgentFileChangeOperation::Update | AgentFileChangeOperation::Delete => self
                .base_revision
                .as_deref()
                .is_some_and(|value| !value.is_empty()),
        };
        let diff_valid = match (&self.inline_diff, staged, self.operation) {
            (Some(diff), false, _) => {
                !diff.truncated
                    && crate::file_change::diff_digest(&diff.patch)
                        == self.execution.proposal.diff_digest
            }
            (None, true, AgentFileChangeOperation::Create | AgentFileChangeOperation::Update) => {
                true
            }
            _ => false,
        };
        let target_content = self.execution.target_content.as_deref().unwrap_or_default();
        let expected_line_count = if target_content.is_empty() {
            0
        } else {
            target_content.bytes().filter(|byte| *byte == b'\n').count() as u64
                + u64::from(!target_content.ends_with('\n'))
        };
        if self.schema_version != AGENT_FILE_CHANGE_PROTOCOL_SCHEMA_VERSION
            || self.id.trim().is_empty()
            || self.transaction_id.trim().is_empty()
            || self.file_path.trim().is_empty()
            || !matches!(
                self.approval_status,
                AgentApprovalStatus::Required | AgentApprovalStatus::Approved
            )
            || !strategy_valid
            || !base_revision_valid
            || !diff_valid
            || self.execution.validate().is_err()
            || self.execution.source_call_id != self.id
            || self.execution.transaction.id != self.transaction_id
            || self.execution.transaction.operation != domain_operation
            || self.execution.transaction.file_path != self.file_path
            || self.execution.proposal.additions != self.additions
            || self.execution.proposal.deletions != self.deletions
            || self.byte_count != target_content.len() as u64
            || self.line_count != expected_line_count
            || self.execution.observation.state.revision() != self.base_revision.as_deref()
        {
            return Err("invalid Agent FileChange proposal");
        }
        Ok(())
    }
}

impl std::fmt::Debug for AgentFileChangeProposal {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("AgentFileChangeProposal([REDACTED])")
    }
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentFileChangeSnapshot {
    #[serde(deserialize_with = "deserialize_file_change_schema_version")]
    pub schema_version: u32,
    pub transaction_id: String,
    pub conversation_id: String,
    #[serde(deserialize_with = "deserialize_required_nullable")]
    pub project_id: Option<String>,
    pub file_path: String,
    pub operation: AgentFileChangeOperation,
    #[serde(deserialize_with = "deserialize_required_nullable")]
    pub update_strategy: Option<AgentFileChangeUpdateStrategy>,
    pub status: AgentFileChangeStatus,
    #[serde(deserialize_with = "deserialize_required_nullable")]
    pub base_revision: Option<String>,
    pub additions: u64,
    pub deletions: u64,
    pub line_count: u64,
    pub byte_count: u64,
    pub mutation_count: u64,
    pub next_mutation_index: u64,
    pub stats_final: bool,
    #[serde(deserialize_with = "deserialize_required_nullable")]
    pub summary: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct AgentFileChangeSnapshotWire {
    #[serde(deserialize_with = "deserialize_file_change_schema_version")]
    schema_version: u32,
    transaction_id: String,
    conversation_id: String,
    #[serde(deserialize_with = "deserialize_required_nullable")]
    project_id: Option<String>,
    file_path: String,
    operation: AgentFileChangeOperation,
    #[serde(deserialize_with = "deserialize_required_nullable")]
    update_strategy: Option<AgentFileChangeUpdateStrategy>,
    status: AgentFileChangeStatus,
    #[serde(deserialize_with = "deserialize_required_nullable")]
    base_revision: Option<String>,
    additions: u64,
    deletions: u64,
    line_count: u64,
    byte_count: u64,
    mutation_count: u64,
    next_mutation_index: u64,
    stats_final: bool,
    #[serde(deserialize_with = "deserialize_required_nullable")]
    summary: Option<String>,
    created_at: i64,
    updated_at: i64,
}

impl<'de> Deserialize<'de> for AgentFileChangeSnapshot {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let wire = AgentFileChangeSnapshotWire::deserialize(deserializer)?;
        let snapshot = Self {
            schema_version: wire.schema_version,
            transaction_id: wire.transaction_id,
            conversation_id: wire.conversation_id,
            project_id: wire.project_id,
            file_path: wire.file_path,
            operation: wire.operation,
            update_strategy: wire.update_strategy,
            status: wire.status,
            base_revision: wire.base_revision,
            additions: wire.additions,
            deletions: wire.deletions,
            line_count: wire.line_count,
            byte_count: wire.byte_count,
            mutation_count: wire.mutation_count,
            next_mutation_index: wire.next_mutation_index,
            stats_final: wire.stats_final,
            summary: wire.summary,
            created_at: wire.created_at,
            updated_at: wire.updated_at,
        };
        snapshot.validate().map_err(serde::de::Error::custom)?;
        Ok(snapshot)
    }
}

impl AgentFileChangeSnapshot {
    pub fn validate(&self) -> Result<(), &'static str> {
        let operation_valid = match self.operation {
            AgentFileChangeOperation::Create => {
                self.update_strategy.is_none() && self.base_revision.is_none()
            }
            AgentFileChangeOperation::Update => {
                self.update_strategy.is_some()
                    && self
                        .base_revision
                        .as_deref()
                        .is_some_and(|value| !value.is_empty())
            }
            AgentFileChangeOperation::Delete => {
                self.update_strategy.is_none()
                    && self
                        .base_revision
                        .as_deref()
                        .is_some_and(|value| !value.is_empty())
            }
        };
        let stats_final_valid = match self.status {
            AgentFileChangeStatus::Drafting | AgentFileChangeStatus::Ready => !self.stats_final,
            AgentFileChangeStatus::WaitingApproval
            | AgentFileChangeStatus::Applying
            | AgentFileChangeStatus::Applied
            | AgentFileChangeStatus::AlreadyApplied
            | AgentFileChangeStatus::Rejected
            | AgentFileChangeStatus::Conflict
            | AgentFileChangeStatus::Failed
            | AgentFileChangeStatus::OutcomeUnknown
            | AgentFileChangeStatus::Aborted
            | AgentFileChangeStatus::Expired => self.stats_final,
        };
        if self.schema_version != AGENT_FILE_CHANGE_PROTOCOL_SCHEMA_VERSION
            || self.transaction_id.trim().is_empty()
            || self.conversation_id.trim().is_empty()
            || self
                .project_id
                .as_deref()
                .is_some_and(|value| value.trim().is_empty())
            || self.file_path.trim().is_empty()
            || !operation_valid
            || !stats_final_valid
            || (self.operation == AgentFileChangeOperation::Delete
                && (self.line_count != 0 || self.byte_count != 0))
            || self.next_mutation_index != self.mutation_count
            || self.created_at < 0
            || self.updated_at < self.created_at
        {
            return Err("invalid Agent FileChange snapshot");
        }
        Ok(())
    }
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentFileChangePreview {
    #[serde(deserialize_with = "deserialize_file_change_schema_version")]
    pub schema_version: u32,
    pub preview_id: String,
    pub stream_id: String,
    pub attempt: usize,
    pub tool_call_index: usize,
    /// Present only after an application-owned canonical Tool Call ID exists.
    ///
    /// Streaming previews are provisional, so the runtime intentionally leaves this unset instead
    /// of exposing a provider's raw correlation ID.
    #[serde(deserialize_with = "deserialize_required_nullable")]
    pub tool_call_id: Option<String>,
    pub transaction_id: String,
    pub file_path: String,
    pub additions: u64,
    pub deletions: u64,
    pub line_count: u64,
    pub byte_count: u64,
    pub generated_bytes: u64,
    pub content_offset_bytes: u64,
    pub content_delta: String,
    pub updated_at: i64,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct AgentFileChangePreviewWire {
    #[serde(deserialize_with = "deserialize_file_change_schema_version")]
    schema_version: u32,
    preview_id: String,
    stream_id: String,
    attempt: usize,
    tool_call_index: usize,
    #[serde(deserialize_with = "deserialize_required_nullable")]
    tool_call_id: Option<String>,
    transaction_id: String,
    file_path: String,
    additions: u64,
    deletions: u64,
    line_count: u64,
    byte_count: u64,
    generated_bytes: u64,
    content_offset_bytes: u64,
    content_delta: String,
    updated_at: i64,
}

impl<'de> Deserialize<'de> for AgentFileChangePreview {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let wire = AgentFileChangePreviewWire::deserialize(deserializer)?;
        let preview = Self {
            schema_version: wire.schema_version,
            preview_id: wire.preview_id,
            stream_id: wire.stream_id,
            attempt: wire.attempt,
            tool_call_index: wire.tool_call_index,
            tool_call_id: wire.tool_call_id,
            transaction_id: wire.transaction_id,
            file_path: wire.file_path,
            additions: wire.additions,
            deletions: wire.deletions,
            line_count: wire.line_count,
            byte_count: wire.byte_count,
            generated_bytes: wire.generated_bytes,
            content_offset_bytes: wire.content_offset_bytes,
            content_delta: wire.content_delta,
            updated_at: wire.updated_at,
        };
        preview.validate().map_err(serde::de::Error::custom)?;
        Ok(preview)
    }
}

impl AgentFileChangePreview {
    pub fn validate(&self) -> Result<(), &'static str> {
        let delta_end = self
            .content_offset_bytes
            .checked_add(self.content_delta.len() as u64);
        if self.schema_version != AGENT_FILE_CHANGE_PROTOCOL_SCHEMA_VERSION
            || self.preview_id.trim().is_empty()
            || self.stream_id.trim().is_empty()
            || self.attempt == 0
            || self
                .tool_call_id
                .as_deref()
                .is_some_and(|value| value.trim().is_empty())
            || self.transaction_id.trim().is_empty()
            || self.file_path.trim().is_empty()
            || delta_end.is_none_or(|end| end > self.generated_bytes)
            || self.updated_at < 0
        {
            return Err("invalid Agent FileChange preview");
        }
        Ok(())
    }
}

impl std::fmt::Debug for AgentFileChangePreview {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("AgentFileChangePreview([REDACTED])")
    }
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentFileChangeResult {
    #[serde(deserialize_with = "deserialize_file_change_schema_version")]
    pub schema_version: u32,
    pub status: AgentFileChangeResultStatus,
    pub outcome: AgentFileChangeOutcome,
    pub transaction_id: String,
    pub operation: AgentFileChangeOperation,
    #[serde(deserialize_with = "deserialize_required_nullable")]
    pub update_strategy: Option<AgentFileChangeUpdateStrategy>,
    pub file_path: String,
    pub additions: u64,
    pub deletions: u64,
    pub line_count: u64,
    pub byte_count: u64,
    #[serde(deserialize_with = "deserialize_required_nullable")]
    pub revision: Option<String>,
    #[serde(deserialize_with = "deserialize_required_nullable")]
    pub error_code: Option<String>,
    #[serde(deserialize_with = "deserialize_required_nullable")]
    pub error: Option<String>,
    #[serde(deserialize_with = "deserialize_required_nullable")]
    pub message: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct AgentFileChangeResultWire {
    #[serde(deserialize_with = "deserialize_file_change_schema_version")]
    schema_version: u32,
    status: AgentFileChangeResultStatus,
    outcome: AgentFileChangeOutcome,
    transaction_id: String,
    operation: AgentFileChangeOperation,
    #[serde(deserialize_with = "deserialize_required_nullable")]
    update_strategy: Option<AgentFileChangeUpdateStrategy>,
    file_path: String,
    additions: u64,
    deletions: u64,
    line_count: u64,
    byte_count: u64,
    #[serde(deserialize_with = "deserialize_required_nullable")]
    revision: Option<String>,
    #[serde(deserialize_with = "deserialize_required_nullable")]
    error_code: Option<String>,
    #[serde(deserialize_with = "deserialize_required_nullable")]
    error: Option<String>,
    #[serde(deserialize_with = "deserialize_required_nullable")]
    message: Option<String>,
}

impl<'de> Deserialize<'de> for AgentFileChangeResult {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let wire = AgentFileChangeResultWire::deserialize(deserializer)?;
        let result = Self {
            schema_version: wire.schema_version,
            status: wire.status,
            outcome: wire.outcome,
            transaction_id: wire.transaction_id,
            operation: wire.operation,
            update_strategy: wire.update_strategy,
            file_path: wire.file_path,
            additions: wire.additions,
            deletions: wire.deletions,
            line_count: wire.line_count,
            byte_count: wire.byte_count,
            revision: wire.revision,
            error_code: wire.error_code,
            error: wire.error,
            message: wire.message,
        };
        result.validate().map_err(serde::de::Error::custom)?;
        Ok(result)
    }
}

impl AgentFileChangeResult {
    pub fn validate(&self) -> Result<(), &'static str> {
        let strategy_valid = match self.operation {
            AgentFileChangeOperation::Update => true,
            AgentFileChangeOperation::Create | AgentFileChangeOperation::Delete => {
                self.update_strategy.is_none()
            }
        };
        let success = matches!(
            self.status,
            AgentFileChangeResultStatus::Applied | AgentFileChangeResultStatus::AlreadyApplied
        );
        let unknown = self.status == AgentFileChangeResultStatus::OutcomeUnknown;
        let revision_valid = if success {
            match self.operation {
                AgentFileChangeOperation::Create | AgentFileChangeOperation::Update => self
                    .revision
                    .as_deref()
                    .is_some_and(|value| !value.is_empty()),
                AgentFileChangeOperation::Delete => self.revision.is_none(),
            }
        } else {
            self.revision.is_none()
        };
        let state_valid = if success {
            self.outcome == AgentFileChangeOutcome::Applied
                && self.error_code.is_none()
                && self.error.is_none()
        } else if unknown {
            self.outcome == AgentFileChangeOutcome::OutcomeUnknown
                && self
                    .error_code
                    .as_deref()
                    .is_some_and(is_safe_file_change_error_code)
        } else {
            self.outcome == AgentFileChangeOutcome::DefinitelyNotExecuted
                && (self
                    .error_code
                    .as_deref()
                    .is_some_and(is_safe_file_change_error_code)
                    || self
                        .error
                        .as_deref()
                        .is_some_and(|value| !value.trim().is_empty())
                    || self
                        .message
                        .as_deref()
                        .is_some_and(|value| !value.trim().is_empty()))
        };
        let error_code_valid = self
            .error_code
            .as_deref()
            .is_none_or(is_safe_file_change_error_code);
        if self.schema_version != AGENT_FILE_CHANGE_PROTOCOL_SCHEMA_VERSION
            || self.transaction_id.trim().is_empty()
            || self.file_path.trim().is_empty()
            || !strategy_valid
            || !revision_valid
            || !state_valid
            || !error_code_valid
            || (self.operation == AgentFileChangeOperation::Delete
                && (self.line_count != 0 || self.byte_count != 0))
        {
            return Err("invalid Agent FileChange result");
        }
        Ok(())
    }
}

fn is_safe_file_change_error_code(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'_' | b'-' | b'.')
        })
}

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentGitDiffSnapshot {
    pub patch: String,
    pub truncated: bool,
}
