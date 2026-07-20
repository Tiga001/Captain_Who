use crate::{
    AgentAttachmentLibraryContext, AgentCancellationToken, AgentPermissions, AgentRunContext,
};
use serde::{Deserialize, Serialize};
use std::error::Error;
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

pub const OFFICECLI_PROVIDER_ID: &str = "officecli";
pub const OFFICE_ENGINE_STATUS_SCHEMA_VERSION: u32 = 1;
pub const OFFICE_PREPARED_EXECUTION_SCHEMA_VERSION: u32 = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum OfficeDocumentKind {
    Document,
    Spreadsheet,
    Presentation,
}

impl OfficeDocumentKind {
    pub fn accepts_path(self, path: &Path) -> bool {
        let Some(extension) = path.extension().and_then(|value| value.to_str()) else {
            return false;
        };
        matches!(
            (self, extension.to_ascii_lowercase().as_str()),
            (Self::Document, "docx")
                | (Self::Spreadsheet, "xlsx" | "xlsm" | "csv")
                | (Self::Presentation, "pptx")
        )
    }

    pub fn accepted_extensions(self) -> &'static [&'static str] {
        match self {
            Self::Document => &["docx"],
            Self::Spreadsheet => &["xlsx", "xlsm", "csv"],
            Self::Presentation => &["pptx"],
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum OfficeOperation {
    Help,
    Create,
    View,
    Get,
    Query,
    Validate,
    Set,
    Add,
    Remove,
    Move,
    Swap,
}

impl OfficeOperation {
    pub fn cli_name(self) -> &'static str {
        match self {
            Self::Help => "help",
            Self::Create => "create",
            Self::View => "view",
            Self::Get => "get",
            Self::Query => "query",
            Self::Validate => "validate",
            Self::Set => "set",
            Self::Add => "add",
            Self::Remove => "remove",
            Self::Move => "move",
            Self::Swap => "swap",
        }
    }

    pub fn access(self, has_output: bool) -> OfficeOperationAccess {
        match self {
            Self::Help | Self::Get | Self::Query | Self::Validate => {
                OfficeOperationAccess::ReadOnly
            }
            Self::View if !has_output => OfficeOperationAccess::ReadOnly,
            Self::View
            | Self::Create
            | Self::Set
            | Self::Add
            | Self::Remove
            | Self::Move
            | Self::Swap => OfficeOperationAccess::FileWrite,
        }
    }

    /// Parses only the deliberately supported command surface.
    ///
    /// Administrative, network-listening, resident, raw-XML mutation, and
    /// path-ambiguous commands are rejected rather than passed through.
    pub fn parse_supported(value: &str) -> Result<Self, OfficeEngineError> {
        match value.trim().to_ascii_lowercase().as_str() {
            "help" | "--help" => Ok(Self::Help),
            "create" => Ok(Self::Create),
            "view" => Ok(Self::View),
            "get" => Ok(Self::Get),
            "query" => Ok(Self::Query),
            "validate" => Ok(Self::Validate),
            "set" => Ok(Self::Set),
            "add" => Ok(Self::Add),
            "remove" => Ok(Self::Remove),
            "move" => Ok(Self::Move),
            "swap" => Ok(Self::Swap),
            "install" | "config" | "watch" | "open" | "close" | "mcp" | "serve" | "server"
            | "raw" | "raw-set" | "add-part" | "batch" | "dump" | "merge" => {
                Err(OfficeEngineError::new(
                    OfficeEngineErrorCode::UnsafeOperation,
                    OfficeEngineRecovery::ChangeRequest,
                    format!(
                        "Office operation `{value}` is outside the supported safe command surface."
                    ),
                ))
            }
            _ => Err(OfficeEngineError::new(
                OfficeEngineErrorCode::UnsupportedOperation,
                OfficeEngineRecovery::ChangeRequest,
                format!("Office operation `{value}` is not supported."),
            )),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum OfficeOperationAccess {
    ReadOnly,
    /// A structured file mutation. Its location is described independently by
    /// each frozen path's [`OfficePathScope`].
    #[serde(alias = "workspaceWrite")]
    FileWrite,
}

/// Host-owned execution inputs used to resolve and authorize Office paths.
///
/// This context is deliberately not embedded in a prepared action. A host must
/// supply the current effective permissions again when executing an approved
/// action, allowing permission revocation to take effect before side effects.
#[derive(Debug, Clone)]
pub struct OfficeExecutionContext {
    workspace_root: Option<PathBuf>,
    permissions: AgentPermissions,
    attachment_library: Option<AgentAttachmentLibraryContext>,
}

impl OfficeExecutionContext {
    pub fn new(
        workspace_root: Option<PathBuf>,
        permissions: AgentPermissions,
        attachment_library: Option<AgentAttachmentLibraryContext>,
    ) -> Self {
        Self {
            workspace_root,
            permissions,
            attachment_library,
        }
    }

    pub fn from_run_context(context: Option<&AgentRunContext>) -> Self {
        let workspace_root = context
            .and_then(|context| context.workspace.as_ref())
            .and_then(|workspace| workspace.root_path.as_deref())
            .map(PathBuf::from);
        let permissions = context
            .map(|context| context.permissions)
            .unwrap_or_default();
        let attachment_library = context.and_then(|context| context.attachment_library.clone());
        Self::new(workspace_root, permissions, attachment_library)
    }

    pub fn workspace_root(&self) -> Option<&Path> {
        self.workspace_root.as_deref()
    }

    pub fn permissions(&self) -> AgentPermissions {
        self.permissions
    }

    pub fn attachment_library(&self) -> Option<&AgentAttachmentLibraryContext> {
        self.attachment_library.as_ref()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum OfficeEngineAvailability {
    Available,
    Unavailable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum OfficeEngineSource {
    Configured,
    PackagedComponent,
    DevelopmentPath,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OfficeEngineCapabilities {
    pub provider_id: String,
    pub document_kinds: Vec<OfficeDocumentKind>,
    pub operations: Vec<OfficeOperation>,
    pub supports_rendering: bool,
    pub supports_validation: bool,
    pub supports_structured_output: bool,
}

impl OfficeEngineCapabilities {
    pub(crate) fn office_cli() -> Self {
        Self {
            provider_id: OFFICECLI_PROVIDER_ID.to_string(),
            document_kinds: vec![
                OfficeDocumentKind::Document,
                OfficeDocumentKind::Spreadsheet,
                OfficeDocumentKind::Presentation,
            ],
            operations: vec![
                OfficeOperation::Help,
                OfficeOperation::Create,
                OfficeOperation::View,
                OfficeOperation::Get,
                OfficeOperation::Query,
                OfficeOperation::Validate,
                OfficeOperation::Set,
                OfficeOperation::Add,
                OfficeOperation::Remove,
                OfficeOperation::Move,
                OfficeOperation::Swap,
            ],
            supports_rendering: true,
            supports_validation: true,
            supports_structured_output: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OfficeEngineStatus {
    pub schema_version: u32,
    pub provider_id: String,
    pub availability: OfficeEngineAvailability,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<OfficeEngineSource>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub engine_revision: Option<String>,
    pub capabilities: OfficeEngineCapabilities,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_code: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OfficeExecutionRequest {
    pub document_kind: OfficeDocumentKind,
    pub operation: OfficeOperation,
    /// Primary document path. Relative paths are workspace-relative; absolute
    /// paths, system aliases, and registered attachment paths are authorized by
    /// the host-owned [`OfficeExecutionContext`]. `help` is the only operation
    /// for which this must be absent.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub document_path: Option<String>,
    /// Structured process arguments after the operation and primary document.
    /// They are never interpreted by a shell.
    #[serde(default)]
    pub arguments: Vec<String>,
    /// Managed render output. The engine owns `-o` construction so callers
    /// cannot smuggle an unvalidated output path through `arguments`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_path: Option<String>,
    /// Managed save-as target for mutation operations. The provider edits a
    /// private copy of `document_path`; only this separately frozen destination
    /// is published. Omit for an in-place mutation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub destination_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,
}

impl OfficeExecutionRequest {
    pub fn access(&self) -> OfficeOperationAccess {
        self.operation.access(self.output_path.is_some())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OfficeExecutionResult {
    pub provider_id: String,
    pub engine_revision: String,
    pub document_kind: OfficeDocumentKind,
    pub operation: OfficeOperation,
    /// Exact argv passed after the executable. This is diagnostic data, not a
    /// command string and cannot be replayed through a shell.
    pub argv: Vec<String>,
    pub cwd: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    pub timed_out: bool,
    pub cancelled: bool,
    pub duration_ms: u64,
    pub stdout_truncated: bool,
    pub stderr_truncated: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_code: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum OfficeFileState {
    Missing,
    Present,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OfficeFilePrecondition {
    pub path: String,
    pub state: OfficeFileState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content_revision: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum OfficePathSlot {
    Document,
    Output,
    Destination,
    Resource { index: u32 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum OfficePathPurpose {
    ReadSource,
    WriteTarget,
    InPlaceTarget,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum OfficePathScope {
    Workspace,
    External,
    Attachment,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum OfficeWriteDisposition {
    CreateNew,
    ReplaceExisting,
}

/// An opaque filesystem object identity. `revision` binds the normalized
/// canonical location and stable metadata; Unix hosts additionally bind the
/// device/inode pair so same-content replacements are detected.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OfficePathIdentity {
    pub revision: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inode: Option<u64>,
}

/// One path slot from the normalized, permission-checked execution plan.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OfficeFrozenPath {
    pub slot: OfficePathSlot,
    pub logical_path: String,
    pub purpose: OfficePathPurpose,
    pub scope: OfficePathScope,
    pub normalized_path: String,
    pub state: OfficeFileState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub object_identity: Option<OfficePathIdentity>,
    pub parent_identity: OfficePathIdentity,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_revision: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub write_disposition: Option<OfficeWriteDisposition>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OfficePreparedExecution {
    pub schema_version: u32,
    pub provider_id: String,
    pub engine_revision: String,
    /// A non-reversible digest of the canonical workspace identity. This
    /// binds approvals to one workspace without exposing its host path.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_revision: Option<String>,
    pub access: OfficeOperationAccess,
    pub request: OfficeExecutionRequest,
    /// Logical, user-reviewable argv. Runtime-private staging paths never
    /// replace these values in the frozen plan.
    pub argv: Vec<String>,
    /// Schema-v3 normalized path plan. It is rebuilt from the logical request
    /// and the current execution context immediately before execution.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub paths: Vec<OfficeFrozenPath>,
    // The schema-v2 fields remain deserializable so persisted pending actions
    // receive an explicit unsupported-schema error instead of a decode error.
    // They are not trusted or populated by schema-v3 preparation.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub document_precondition: Option<OfficeFilePrecondition>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_precondition: Option<OfficeFilePrecondition>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub destination_precondition: Option<OfficeFilePrecondition>,
    /// Immutable identities for workspace resources referenced by path-bearing
    /// properties. Runtime copies these resources to a private snapshot before
    /// the provider starts.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub resource_preconditions: Vec<OfficeFilePrecondition>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum OfficeEngineErrorCode {
    Unavailable,
    InvalidConfiguration,
    InvalidRequest,
    UnsafeOperation,
    UnsupportedOperation,
    WorkspaceViolation,
    PreconditionFailed,
    InvalidOutput,
    CommitIndeterminate,
    Io,
    ProcessFailure,
}

impl OfficeEngineErrorCode {
    pub fn stable_name(self) -> &'static str {
        match self {
            Self::Unavailable => "office.engine_unavailable",
            Self::InvalidConfiguration => "office.invalid_configuration",
            Self::InvalidRequest => "office.invalid_request",
            Self::UnsafeOperation => "office.unsafe_operation",
            Self::UnsupportedOperation => "office.unsupported_operation",
            Self::WorkspaceViolation => "office.workspace_violation",
            Self::PreconditionFailed => "office.precondition_failed",
            Self::InvalidOutput => "office.invalid_output",
            Self::CommitIndeterminate => "office.commit_indeterminate",
            Self::Io => "office.io",
            Self::ProcessFailure => "office.process_failure",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum OfficeEngineRecovery {
    InstallComponent,
    ChangeConfiguration,
    ChangeRequest,
    Retry,
    InspectState,
}

impl OfficeEngineRecovery {
    pub fn stable_name(self) -> &'static str {
        match self {
            Self::InstallComponent => "installComponent",
            Self::ChangeConfiguration => "changeConfiguration",
            Self::ChangeRequest => "changeRequest",
            Self::Retry => "retry",
            Self::InspectState => "inspectState",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OfficeEngineError {
    code: OfficeEngineErrorCode,
    recovery: OfficeEngineRecovery,
    message: String,
}

impl OfficeEngineError {
    /// Creates a stable provider-facing error for an [`OfficeEngine`]
    /// implementation. Callers should choose the narrowest stable code and a
    /// recovery action that can be safely shown to the user.
    pub fn new(
        code: OfficeEngineErrorCode,
        recovery: OfficeEngineRecovery,
        message: impl Into<String>,
    ) -> Self {
        Self {
            code,
            recovery,
            message: message.into(),
        }
    }

    pub fn code(&self) -> OfficeEngineErrorCode {
        self.code
    }

    pub fn recovery(&self) -> OfficeEngineRecovery {
        self.recovery
    }

    pub fn message(&self) -> &str {
        &self.message
    }
}

impl fmt::Display for OfficeEngineError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.message.fmt(formatter)
    }
}

impl Error for OfficeEngineError {}

pub trait OfficeEngine: Send + Sync {
    fn capabilities(&self) -> OfficeEngineCapabilities;

    fn status(&self, cancellation: AgentCancellationToken) -> OfficeEngineStatus;

    fn prepare(
        &self,
        context: &OfficeExecutionContext,
        request: &OfficeExecutionRequest,
    ) -> Result<OfficePreparedExecution, OfficeEngineError>;

    fn execute_prepared(
        &self,
        context: &OfficeExecutionContext,
        prepared: &OfficePreparedExecution,
        cancellation: AgentCancellationToken,
        action_cancel_flag: Option<Arc<AtomicBool>>,
    ) -> Result<OfficeExecutionResult, OfficeEngineError>;

    fn execute(
        &self,
        context: &OfficeExecutionContext,
        request: &OfficeExecutionRequest,
        cancellation: AgentCancellationToken,
        action_cancel_flag: Option<Arc<AtomicBool>>,
    ) -> Result<OfficeExecutionResult, OfficeEngineError> {
        let prepared = self.prepare(context, request)?;
        self.execute_prepared(context, &prepared, cancellation, action_cancel_flag)
    }
}
