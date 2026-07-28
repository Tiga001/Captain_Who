use crate::{
    file_input::AgentFileInputExecutionContext, AgentAttachmentLibraryContext,
    AgentCancellationToken, AgentFileInputBinding, AgentFileInputRef, AgentFileInputSpec,
    AgentPermissions, AgentRunContext,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::error::Error;
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

pub const OFFICECLI_PROVIDER_ID: &str = "officecli";
pub const OFFICE_ENGINE_STATUS_SCHEMA_VERSION: u32 = 1;
pub const OFFICE_PREPARED_EXECUTION_SCHEMA_VERSION: u32 = 5;

pub(crate) const OFFICE_AGENT_INPUT_PLACEHOLDER_PREFIX: &str = "__mycopilot_agent_input__/";

pub(crate) fn office_agent_input_placeholder(mount_path: &str) -> String {
    format!("{OFFICE_AGENT_INPUT_PLACEHOLDER_PREFIX}{mount_path}")
}

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

/// Provider-neutral Office help topics exposed to the model.
///
/// `status`, `help`, `create`, `view`, `validate`, `move`, and `swap` describe Host-managed
/// operations and never become provider argv. The remaining topics may be used for OfficeCLI's
/// element-oriented schema help. The document format is always derived from the selected Office
/// tool, so callers never repeat `docx`, `xlsx`, or `pptx` as a provider token.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum OfficeHelpVerb {
    Status,
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

impl OfficeHelpVerb {
    pub fn stable_name(self) -> &'static str {
        match self {
            Self::Status => "status",
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

    /// Returns the provider verb only for OfficeCLI's element-oriented help contract.
    /// Host-managed topics deliberately return `None` so they cannot cross the provider boundary.
    pub fn provider_element_cli_name(self) -> Option<&'static str> {
        match self {
            Self::Get | Self::Query | Self::Set | Self::Add | Self::Remove => {
                Some(self.stable_name())
            }
            Self::Status
            | Self::Help
            | Self::Create
            | Self::View
            | Self::Validate
            | Self::Move
            | Self::Swap => None,
        }
    }

    pub fn is_host_managed(self) -> bool {
        self.provider_element_cli_name().is_none()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum OfficeViewMode {
    Text,
    Annotated,
    Outline,
    Stats,
    Issues,
    Html,
    Svg,
    Screenshot,
    Forms,
}

impl OfficeViewMode {
    pub fn cli_name(self) -> &'static str {
        match self {
            Self::Text => "text",
            Self::Annotated => "annotated",
            Self::Outline => "outline",
            Self::Stats => "stats",
            Self::Issues => "issues",
            Self::Html => "html",
            Self::Svg => "svg",
            Self::Screenshot => "screenshot",
            Self::Forms => "forms",
        }
    }

    pub fn writes_output(self) -> bool {
        matches!(self, Self::Html | Self::Svg | Self::Screenshot)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum OfficeViewRenderMode {
    Auto,
    Html,
}

impl OfficeViewRenderMode {
    pub fn cli_name(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Html => "html",
        }
    }
}

/// Contact-sheet layout for document and presentation screenshots.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "camelCase", deny_unknown_fields)]
pub enum OfficeGridLayout {
    Auto,
    Columns { columns: u16 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum OfficeCellShift {
    Left,
    Up,
}

impl OfficeCellShift {
    pub fn cli_name(self) -> &'static str {
        match self {
            Self::Left => "left",
            Self::Up => "up",
        }
    }
}

/// A mutually exclusive insertion or movement anchor.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase", deny_unknown_fields)]
pub enum OfficeElementPosition {
    Index { index: u32 },
    After { target: String },
    Before { target: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OfficeTextReplacement {
    pub find: String,
    pub replace: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OfficePageRange {
    pub start: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub end: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OfficeViewport {
    pub width: u32,
    pub height: u32,
}

/// Provider-neutral Office property values.
///
/// Nested JSON is deliberately excluded by the canonical compiler. OfficeCLI's property surface
/// accepts scalar `key=value` pairs; representing that surface as a sorted map removes quoting,
/// ordering, and repeated-flag decisions from the model while keeping element-specific properties
/// extensible.
pub type OfficePropertyMap = BTreeMap<String, Value>;

/// Typed, provider-neutral parameters for one managed Office operation.
///
/// `OfficeExecutionRequest.operation` is repeated outside this enum for compact UI routing and
/// legacy protocol compatibility. The trusted compiler requires the two discriminants to agree.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum OfficeOperationParameters {
    Help {
        #[serde(skip_serializing_if = "Option::is_none")]
        verb: Option<OfficeHelpVerb>,
        #[serde(skip_serializing_if = "Option::is_none")]
        element: Option<String>,
    },
    Create {
        #[serde(skip_serializing_if = "Option::is_none")]
        locale: Option<String>,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        minimal: bool,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        overwrite: bool,
    },
    View {
        mode: OfficeViewMode,
        #[serde(skip_serializing_if = "Option::is_none")]
        start: Option<u32>,
        #[serde(skip_serializing_if = "Option::is_none")]
        end: Option<u32>,
        #[serde(skip_serializing_if = "Option::is_none")]
        max_lines: Option<u32>,
        #[serde(skip_serializing_if = "Option::is_none")]
        issue_type: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        limit: Option<u32>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        columns: Vec<String>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        pages: Vec<OfficePageRange>,
        #[serde(skip_serializing_if = "Option::is_none")]
        range: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        viewport: Option<OfficeViewport>,
        #[serde(skip_serializing_if = "Option::is_none")]
        grid: Option<OfficeGridLayout>,
        #[serde(skip_serializing_if = "Option::is_none")]
        render_mode: Option<OfficeViewRenderMode>,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        page_count: bool,
    },
    Get {
        #[serde(skip_serializing_if = "Option::is_none")]
        target: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        depth: Option<u32>,
    },
    Query {
        selector: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        contains: Option<String>,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        compact: bool,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        fields: Vec<String>,
    },
    Validate,
    Set {
        target: String,
        #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
        properties: OfficePropertyMap,
        #[serde(skip_serializing_if = "Option::is_none")]
        replacement: Option<OfficeTextReplacement>,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        force: bool,
    },
    Add {
        parent: String,
        element_type: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        copy_from: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        position: Option<OfficeElementPosition>,
        #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
        properties: OfficePropertyMap,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        force: bool,
    },
    Remove {
        target: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        shift: Option<OfficeCellShift>,
        #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
        properties: OfficePropertyMap,
    },
    Move {
        target: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        new_parent: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        position: Option<OfficeElementPosition>,
        #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
        properties: OfficePropertyMap,
    },
    Swap {
        first_target: String,
        second_target: String,
    },
}

impl OfficeOperationParameters {
    pub fn operation(&self) -> OfficeOperation {
        match self {
            Self::Help { .. } => OfficeOperation::Help,
            Self::Create { .. } => OfficeOperation::Create,
            Self::View { .. } => OfficeOperation::View,
            Self::Get { .. } => OfficeOperation::Get,
            Self::Query { .. } => OfficeOperation::Query,
            Self::Validate => OfficeOperation::Validate,
            Self::Set { .. } => OfficeOperation::Set,
            Self::Add { .. } => OfficeOperation::Add,
            Self::Remove { .. } => OfficeOperation::Remove,
            Self::Move { .. } => OfficeOperation::Move,
            Self::Swap { .. } => OfficeOperation::Swap,
        }
    }

    pub fn view_mode(&self) -> Option<OfficeViewMode> {
        match self {
            Self::View { mode, .. } => Some(*mode),
            _ => None,
        }
    }
}

/// Version bridge for persisted schema-v3 actions.
///
/// New actions always serialize the typed object. The legacy vector remains deserializable only
/// so startup and audit code can load old pending actions and reject their schema explicitly; it
/// is never accepted by the schema-v4 compiler.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum OfficeRequestParameters {
    Typed(OfficeOperationParameters),
    Legacy(Vec<String>),
}

impl OfficeRequestParameters {
    pub fn typed(&self) -> Option<&OfficeOperationParameters> {
        match self {
            Self::Typed(parameters) => Some(parameters),
            Self::Legacy(_) => None,
        }
    }
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
        self.file_inputs = file_inputs;
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
    /// Provider-neutral operation parameters. The model never supplies OfficeCLI argv.
    ///
    /// `arguments` is accepted as a deserialize-only alias for schema-v3 pending actions. Such
    /// actions retain their original outer schema version and are rejected before execution.
    #[serde(rename = "parameters", alias = "arguments")]
    pub parameters: OfficeRequestParameters,
    /// Managed render output. The engine owns provider output-flag construction;
    /// callers cannot inject an unvalidated output target through typed fields.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_path: Option<String>,
    /// Managed save-as target for mutation operations. The provider edits a
    /// private copy of `document_path`; only this separately frozen destination
    /// is published. Omit for an in-place mutation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub destination_path: Option<String>,
    /// Unified, model-declared read inputs used by path-bearing semantic operations.
    ///
    /// The request contains logical sources only. Preparation freezes exact bytes into
    /// `OfficePreparedExecution::input_bindings`; execution re-resolves and materializes them
    /// into a private read-only directory before invoking the provider.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub inputs: Vec<AgentFileInputSpec>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,
}

impl OfficeExecutionRequest {
    pub fn access(&self) -> OfficeOperationAccess {
        self.operation.access(self.output_path.is_some())
    }

    pub fn typed_parameters(&self) -> Option<&OfficeOperationParameters> {
        self.parameters.typed()
    }

    pub fn view_mode(&self) -> Option<OfficeViewMode> {
        self.typed_parameters()
            .and_then(OfficeOperationParameters::view_mode)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OfficeExecutionResult {
    pub provider_id: String,
    pub engine_revision: String,
    pub document_kind: OfficeDocumentKind,
    pub operation: OfficeOperation,
    /// Authoritative files published by this execution.
    ///
    /// This model-actionable field intentionally precedes low-level process
    /// diagnostics in the serialized result. Entries are added only after
    /// validation and atomic publication succeed. `read_path` always
    /// identifies the final target and never a staging or private snapshot
    /// path.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub outputs: Vec<OfficePublishedOutput>,
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
    #[serde(flatten, default)]
    pub output_capture: crate::command::ProcessOutputCaptureMetadata,
    /// Backend-only complete stdout capture consumed by Exact History.
    #[serde(skip, default)]
    pub stdout_spool: crate::command::ProcessOutputSpool,
    /// Backend-only complete stderr capture consumed by Exact History.
    #[serde(skip, default)]
    pub stderr_spool: crate::command::ProcessOutputSpool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_code: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl OfficeExecutionResult {
    pub fn output_spool_substitutions(
        &self,
    ) -> Vec<crate::command::ProcessOutputSpoolSubstitution> {
        crate::command::process_output_spool_substitutions(&self.stdout_spool, &self.stderr_spool)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum OfficePublishedOutputRole {
    Render,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum OfficePublishedOutputKind {
    Image,
    Document,
}

/// The page/slide selection requested for a rendered output.
///
/// This records selection intent, not independently verified per-page
/// coverage. Provider success and artifact validation prove that the published
/// file is valid, but do not prove that every requested page appears in it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum OfficeRenderPageSelection {
    All,
    Explicit { pages: Vec<u32> },
}

/// One validated Office artifact at its final, atomically published location.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OfficePublishedOutput {
    pub role: OfficePublishedOutputRole,
    pub kind: OfficePublishedOutputKind,
    pub mime_type: String,
    /// Typed reference that can be reused by tools accepting
    /// [`AgentFileInputRef`]. It remains subject to the current read policy.
    pub source: AgentFileInputRef,
    /// Logical final path that can be supplied to ordinary file-reading tools.
    /// Every consumer still enforces its own format, size, and delivery limits.
    pub read_path: String,
    pub scope: OfficePathScope,
    /// Whether the current Agent read permission covers `read_path`.
    ///
    /// This is not a promise that a downstream visual/file tool accepts the
    /// artifact's MIME type, dimensions, or byte size.
    pub readable_by_agent: bool,
    pub size_bytes: u64,
    /// Lowercase hexadecimal SHA-256 of the published bytes.
    pub sha256: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub width: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub height: Option<u32>,
    pub page_selection: OfficeRenderPageSelection,
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
    /// Immutable content identities for every model-declared Office input.
    ///
    /// These bindings are prepared before approval and revalidated against current run
    /// authorities immediately before provider execution.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub input_bindings: Vec<AgentFileInputBinding>,
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
    RenderBackendUnavailable,
    RenderBackendInvalid,
    RenderBackendTimeout,
    RenderBackendFailed,
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
            Self::RenderBackendUnavailable => "office.render_backend_unavailable",
            Self::RenderBackendInvalid => "office.render_backend_invalid",
            Self::RenderBackendTimeout => "office.render_backend_timeout",
            Self::RenderBackendFailed => "office.render_backend_failed",
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
