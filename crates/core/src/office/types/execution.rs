use super::deserialize_required_nullable;
use super::errors::{OfficeEngineError, OfficeEngineErrorCode, OfficeEngineRecovery};
use super::operations::{
    OfficeDocumentKind, OfficeOperation, OfficeOperationAccess, OfficeOperationParameters,
    OfficePathScope, OfficePresentationRenderPlan, OfficeViewMode,
};
use super::outputs::{
    OfficeExecutionResult, OfficeManagedScriptOutputResult, OfficePresentationEditResult,
};
use crate::{
    file_input::AgentFileInputExecutionContext, AgentAttachmentLibraryContext,
    AgentCancellationToken, AgentFileInputBinding, AgentFileInputSpec, AgentPermissions,
    AgentRunContext,
};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

pub const OFFICECLI_PROVIDER_ID: &str = "officecli";
pub const OFFICE_ENGINE_STATUS_SCHEMA_VERSION: u32 = 1;
pub const OFFICE_PREPARED_EXECUTION_SCHEMA_VERSION: u32 = 6;

pub(crate) const OFFICE_AGENT_INPUT_PLACEHOLDER_PREFIX: &str = "__mycopilot_agent_input__/";

pub(crate) fn office_agent_input_placeholder(mount_path: &str) -> String {
    format!("{OFFICE_AGENT_INPUT_PLACEHOLDER_PREFIX}{mount_path}")
}

/// Host-owned request for one bounded edit of an existing presentation.
///
/// The fixed MJS facade emits only the existing provider-neutral mutation parameter variants.
/// Raw OfficeCLI argv, batch JSON, OOXML, executable paths and staging locations never enter this
/// contract. The Host applies the whole vector to one private source copy and publishes once.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OfficePresentationEditRequest {
    pub source_path: String,
    pub source_binding: AgentFileInputBinding,
    pub destination_path: String,
    pub destination_binding: OfficeManagedScriptBinding,
    pub inputs: Vec<AgentFileInputSpec>,
    pub input_bindings: Vec<AgentFileInputBinding>,
    pub operations: Vec<OfficeOperationParameters>,
    pub timeout_ms: Option<u64>,
}

/// Purpose and immutable publication target for one provenance-bound Office Skill script.
///
/// The script itself and every declared input are frozen separately as ordinary
/// [`AgentFileInputBinding`] values on the command request. This binding exists so the final
/// Office destination is frozen before approval as well, then revalidated immediately before
/// atomic publication.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum OfficeManagedScriptPurpose {
    Create,
    Edit,
    EditPresentationPlan,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OfficeManagedScriptBinding {
    pub schema_version: u32,
    pub document_kind: OfficeDocumentKind,
    pub purpose: OfficeManagedScriptPurpose,
    pub script_mount_path: String,
    #[serde(deserialize_with = "deserialize_required_nullable")]
    pub source_mount_path: Option<String>,
    pub destination: OfficeFrozenPath,
}

pub const OFFICE_MANAGED_SCRIPT_BINDING_SCHEMA_VERSION: u32 = 1;

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
    file_inputs: AgentFileInputExecutionContext,
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
            file_inputs: AgentFileInputExecutionContext::from_attachment_library(
                attachment_library.clone(),
            ),
            attachment_library,
        }
    }

    /// Adds the trusted, run-scoped authorities required to resolve unified Agent file inputs.
    ///
    /// This authority is execution-only. It is never serialized into a proposed action.
    pub fn with_file_inputs(mut self, file_inputs: AgentFileInputExecutionContext) -> Self {
        self.file_inputs = if file_inputs.workspace_context().is_none() {
            file_inputs.with_workspace(self.file_inputs.workspace_context())
        } else {
            file_inputs
        };
        self
    }

    pub fn with_workspace(mut self, workspace: Option<&crate::AgentWorkspaceContext>) -> Self {
        self.file_inputs = self.file_inputs.with_workspace(workspace);
        self
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
            .with_workspace(context.and_then(|context| context.workspace.as_ref()))
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

    pub(crate) fn file_inputs(&self) -> &AgentFileInputExecutionContext {
        &self.file_inputs
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
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OfficeExecutionRequest {
    pub document_kind: OfficeDocumentKind,
    pub operation: OfficeOperation,
    /// Primary document path. Relative paths are workspace-relative; absolute
    /// paths, system aliases, and registered attachment paths are authorized by
    /// the host-owned [`OfficeExecutionContext`]. `help` is the only operation
    /// for which this must be absent.
    #[serde(deserialize_with = "deserialize_required_nullable")]
    pub document_path: Option<String>,
    /// Provider-neutral operation parameters. The model never supplies OfficeCLI argv.
    pub parameters: OfficeOperationParameters,
    /// Managed render output. The engine owns provider output-flag construction;
    /// callers cannot inject an unvalidated output target through typed fields.
    #[serde(deserialize_with = "deserialize_required_nullable")]
    pub output_path: Option<String>,
    /// Managed save-as target for mutation operations. The provider edits a
    /// private copy of `document_path`; only this separately frozen destination
    /// is published. Omit for an in-place mutation.
    #[serde(deserialize_with = "deserialize_required_nullable")]
    pub destination_path: Option<String>,
    /// Unified, model-declared read inputs used by path-bearing semantic operations.
    ///
    /// The request contains logical sources only. Preparation freezes exact bytes into
    /// `OfficePreparedExecution::input_bindings`; execution re-resolves and materializes them
    /// into a private read-only directory before invoking the provider.
    pub inputs: Vec<AgentFileInputSpec>,
    #[serde(deserialize_with = "deserialize_required_nullable")]
    pub timeout_ms: Option<u64>,
}

impl OfficeExecutionRequest {
    pub fn access(&self) -> OfficeOperationAccess {
        self.operation.access(self.output_path.is_some())
    }

    pub fn typed_parameters(&self) -> &OfficeOperationParameters {
        &self.parameters
    }

    pub fn view_mode(&self) -> Option<OfficeViewMode> {
        self.typed_parameters().view_mode()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum OfficeFileState {
    Missing,
    Present,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase", deny_unknown_fields)]
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
pub enum OfficeWriteDisposition {
    CreateNew,
    ReplaceExisting,
}

/// An opaque filesystem object identity. `revision` binds the normalized
/// canonical location and stable metadata; Unix hosts additionally bind the
/// device/inode pair so same-content replacements are detected.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OfficePathIdentity {
    pub revision: String,
    #[serde(deserialize_with = "deserialize_required_nullable")]
    pub device: Option<u64>,
    #[serde(deserialize_with = "deserialize_required_nullable")]
    pub inode: Option<u64>,
}

/// One path slot from the normalized, permission-checked execution plan.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OfficeFrozenPath {
    pub slot: OfficePathSlot,
    pub logical_path: String,
    pub purpose: OfficePathPurpose,
    pub scope: OfficePathScope,
    pub normalized_path: String,
    pub state: OfficeFileState,
    #[serde(deserialize_with = "deserialize_required_nullable")]
    pub object_identity: Option<OfficePathIdentity>,
    pub parent_identity: OfficePathIdentity,
    #[serde(deserialize_with = "deserialize_required_nullable")]
    pub content_revision: Option<String>,
    #[serde(deserialize_with = "deserialize_required_nullable")]
    pub size: Option<u64>,
    #[serde(deserialize_with = "deserialize_required_nullable")]
    pub write_disposition: Option<OfficeWriteDisposition>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OfficePreparedExecution {
    pub schema_version: u32,
    pub provider_id: String,
    pub engine_revision: String,
    /// A non-reversible digest of the canonical workspace identity. This
    /// binds approvals to one workspace without exposing its host path.
    #[serde(deserialize_with = "deserialize_required_nullable")]
    pub workspace_revision: Option<String>,
    pub access: OfficeOperationAccess,
    pub request: OfficeExecutionRequest,
    /// Logical, user-reviewable argv. Runtime-private staging paths never
    /// replace these values in the frozen plan.
    pub argv: Vec<String>,
    /// Host-owned render geometry. Required for presentation screenshots and
    /// null for every other Office operation.
    #[serde(deserialize_with = "deserialize_required_nullable")]
    pub resolved_render_plan: Option<OfficePresentationRenderPlan>,
    /// Current normalized path plan. It is rebuilt from the logical request
    /// and the current execution context immediately before execution.
    pub paths: Vec<OfficeFrozenPath>,
    /// Immutable content identities for every model-declared Office input.
    ///
    /// These bindings are prepared before approval and revalidated against current run
    /// authorities immediately before provider execution.
    pub input_bindings: Vec<AgentFileInputBinding>,
}

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

    /// Validates and atomically publishes one Host-private candidate produced by a
    /// provenance-bound Office Skill script. Spreadsheet candidates receive the frozen managed
    /// Python identity for the fixed openpyxl reopen gate; Word and PowerPoint retain the pinned
    /// OfficeCLI schema gate. The private candidate is removed when its staging owner is dropped.
    fn commit_managed_script_output(
        &self,
        _context: &OfficeExecutionContext,
        _staging: &mut crate::office::OfficeManagedScriptStaging,
        _managed_python: Option<&crate::artifact_runtime::ArtifactRuntimeInvocation>,
        _cancellation: AgentCancellationToken,
        _action_cancel_flag: Option<Arc<AtomicBool>>,
    ) -> Result<OfficeManagedScriptOutputResult, OfficeEngineError> {
        Err(OfficeEngineError::new(
            OfficeEngineErrorCode::UnsupportedOperation,
            OfficeEngineRecovery::ChangeRequest,
            "This Office engine does not support managed Skill script publication.",
        ))
    }

    /// Applies one fixed-facade presentation edit as a single Host-owned transaction.
    ///
    /// Engines which do not implement the pinned Office transaction surface fail closed. This
    /// default keeps test/fallback engines source-compatible without accidentally emulating batch
    /// edits as several independently published mutations.
    fn execute_presentation_edit(
        &self,
        _context: &OfficeExecutionContext,
        _request: &OfficePresentationEditRequest,
        _cancellation: AgentCancellationToken,
        _action_cancel_flag: Option<Arc<AtomicBool>>,
    ) -> Result<OfficePresentationEditResult, OfficeEngineError> {
        Err(OfficeEngineError::new(
            OfficeEngineErrorCode::UnsupportedOperation,
            OfficeEngineRecovery::ChangeRequest,
            "This Office engine does not support atomic existing-presentation edits.",
        ))
    }

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
