use super::*;

/// Schema version for the best-effort artifact observation attached to `run_command` results.
///
/// Observation is telemetry, not an authorization capability. The host still authorizes and
/// executes the command exclusively through the existing command permission and safety policy.
pub const AGENT_COMMAND_ARTIFACT_OBSERVATION_SCHEMA_VERSION: u32 = 3;

#[derive(Debug, Deserialize, Serialize, Clone, Copy, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum AgentCommandArtifactObservationKind {
    Office,
}

/// Optional, model-authored hint describing which command side effects should be observed.
///
/// A selected workspace is always included when this hint is present. Expected outputs and
/// additional roots are resolved relative to the command cwd. Expected outputs are observation
/// and validation hints, not write authorization. The trusted host validates every path against
/// the current permission snapshot; this request cannot widen it.
#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentCommandArtifactObservationRequest {
    pub kinds: Vec<AgentCommandArtifactObservationKind>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub expected_outputs: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub additional_roots: Vec<String>,
}

#[derive(Debug, Deserialize, Serialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AgentCommandArtifactObservationStatus {
    Complete,
    Partial,
    Failed,
}

#[derive(Debug, Deserialize, Serialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AgentCommandArtifactObservationPhase {
    Setup,
    Before,
    After,
}

#[derive(Debug, Deserialize, Serialize, Clone, Copy, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum AgentCommandArtifactKind {
    Document,
    Spreadsheet,
    Presentation,
}

#[derive(Debug, Deserialize, Serialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AgentCommandArtifactScope {
    Workspace,
    External,
}

#[derive(Debug, Deserialize, Serialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AgentCommandArtifactChangeKind {
    Created,
    Modified,
    Replaced,
    Deleted,
    Renamed,
}

#[derive(Debug, Deserialize, Serialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AgentCommandExpectedArtifactOutcomeKind {
    Created,
    Modified,
    Replaced,
    Renamed,
    Unchanged,
    Missing,
    Unobserved,
    Invalid,
}

#[derive(Debug, Deserialize, Serialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AgentCommandArtifactValidationStatus {
    Valid,
    Invalid,
    NotApplicable,
    Unchecked,
}

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentCommandArtifactValidation {
    pub status: AgentCommandArtifactValidationStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentCommandArtifactMetadata {
    pub size_bytes: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
    pub validation: AgentCommandArtifactValidation,
}

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentCommandArtifactChange {
    pub kind: AgentCommandArtifactChangeKind,
    pub artifact_kind: AgentCommandArtifactKind,
    pub path: String,
    pub scope: AgentCommandArtifactScope,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub previous_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub previous_scope: Option<AgentCommandArtifactScope>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub before: Option<AgentCommandArtifactMetadata>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub after: Option<AgentCommandArtifactMetadata>,
}

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentCommandExpectedArtifactOutcome {
    pub requested_path: String,
    pub outcome: AgentCommandExpectedArtifactOutcomeKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scope: Option<AgentCommandArtifactScope>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub artifact_kind: Option<AgentCommandArtifactKind>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<AgentCommandArtifactMetadata>,
}

#[derive(Debug, Deserialize, Serialize, Clone, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentCommandArtifactSnapshotCoverage {
    pub roots_scanned: u64,
    pub directory_entries_scanned: u64,
    pub office_files_seen: u64,
    pub files_hashed: u64,
    pub files_unhashed: u64,
    pub bytes_hashed: u64,
    pub symlinks_skipped: u64,
    pub excluded_directories: u64,
    pub duration_ms: u64,
    pub time_budget_exceeded: bool,
    pub cancelled: bool,
    pub truncated: bool,
}

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentCommandArtifactObservationCoverage {
    pub workspace_included: bool,
    pub expected_output_count: u64,
    pub additional_root_count: u64,
    pub before: AgentCommandArtifactSnapshotCoverage,
    pub after: AgentCommandArtifactSnapshotCoverage,
}

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentCommandArtifactObservationWarning {
    pub phase: AgentCommandArtifactObservationPhase,
    pub code: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    pub message: String,
}

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentCommandArtifactObservation {
    pub schema_version: u32,
    pub status: AgentCommandArtifactObservationStatus,
    /// Unified completeness flag for model, UI, audit, and checkpoint consumers.
    ///
    /// This remains redundant with `status` on purpose and is required by schema v3.
    pub partial: bool,
    /// Stable backend reason codes explaining why `partial` is true.
    pub stop_reasons: Vec<String>,
    /// Office files considered across the before and after snapshots.
    pub scanned: u64,
    /// Change records included in this observation.
    pub returned: u64,
    /// Known change records omitted by the bounded report projection.
    ///
    /// A partial snapshot can additionally have an unknown unobserved suffix; `partial` and
    /// `stopReasons` prevent this known count from being mistaken for complete coverage.
    pub omitted: u64,
    pub coverage: AgentCommandArtifactObservationCoverage,
    pub changes: Vec<AgentCommandArtifactChange>,
    pub changes_truncated: bool,
    pub changes_omitted: u64,
    pub expected_outputs: Vec<AgentCommandExpectedArtifactOutcome>,
    pub warnings: Vec<AgentCommandArtifactObservationWarning>,
}

#[derive(Debug, Deserialize, Serialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum AgentCommandRuntimeKind {
    Node,
    Python,
}

/// Stable identifier for an application-owned artifact runtime profile.
///
/// A profile selects a reproducible capability family. It is deliberately not a package
/// request: package names, exact versions, provider identity, and runtime integrity evidence are
/// resolved by the trusted host and frozen in [`AgentCommandRuntimeBinding`]. `Pdf` is an
/// internal-only binding selected from the exact activated bundled Skill identity; it is
/// intentionally absent from the model-facing `run_command.runtimeProfile` schema.
#[derive(Debug, Deserialize, Serialize, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "camelCase")]
pub enum AgentCommandRuntimeProfile {
    Documents,
    Spreadsheets,
    Presentations,
    Pdf,
}

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentCommandRuntimeResolvedPackage {
    pub name: String,
    pub version: String,
}

pub const AGENT_COMMAND_RUNTIME_BINDING_SCHEMA_VERSION: u32 = 1;

/// Approval-time identity of a host-resolved runtime profile.
///
/// This value is persisted with the frozen command action. It intentionally excludes executable
/// paths, environment variables, bootstrap paths, and other host-private launch authority. The
/// host resolves the profile again immediately before execution and requires an exact identity
/// match before it starts a process.
#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentCommandRuntimeBinding {
    pub schema_version: u32,
    pub profile: AgentCommandRuntimeProfile,
    pub profile_revision: String,
    pub provider_id: String,
    pub bundle_version: String,
    pub bundle_revision: String,
    pub kind: AgentCommandRuntimeKind,
    pub runtime_version: String,
    pub runtime_fingerprint: String,
    pub resolved_packages: Vec<AgentCommandRuntimeResolvedPackage>,
}

pub const AGENT_COMMAND_RUNTIME_RESOLUTION_SCHEMA_VERSION: u32 = 2;

/// Public execution evidence for a managed command runtime.
///
/// Executable and component paths are intentionally absent: they are private
/// host implementation details and are never persisted into model-visible
/// tool results.
#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentCommandRuntimeResolution {
    pub schema_version: u32,
    pub provider_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub profile: Option<AgentCommandRuntimeProfile>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub profile_revision: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bundle_version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bundle_revision: Option<String>,
    pub kind: AgentCommandRuntimeKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub runtime_version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub runtime_fingerprint: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub resolved_packages: Vec<AgentCommandRuntimeResolvedPackage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_code: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub recovery: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentCommandRequest {
    pub id: String,
    pub command: String,
    #[serde(deserialize_with = "deserialize_required_nullable")]
    pub cwd: Option<String>,
    #[serde(deserialize_with = "deserialize_required_nullable")]
    pub timeout_ms: Option<u64>,
    pub approval_status: AgentApprovalStatus,
    #[serde(deserialize_with = "deserialize_required_nullable")]
    pub risk_level: Option<AgentCommandRiskLevel>,
    #[serde(deserialize_with = "deserialize_required_nullable")]
    pub reason: Option<String>,
    #[serde(deserialize_with = "deserialize_required_nullable")]
    pub observe: Option<AgentCommandArtifactObservationRequest>,
    pub inputs: Vec<AgentFileInputBinding>,
    #[serde(deserialize_with = "deserialize_required_nullable")]
    pub runtime_binding: Option<Box<AgentCommandRuntimeBinding>>,
    /// Backend-only transaction identity for an exact bundled Office Skill script. The model
    /// cannot supply this field; `run_command` derives it from a run-scoped materialization
    /// receipt and freezes the destination before approval.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub managed_office_script: Option<Box<crate::office::OfficeManagedScriptBinding>>,
}
