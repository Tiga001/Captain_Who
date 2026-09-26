use super::deserialize_required_nullable;
use super::errors::{OfficeEngineError, OfficeEngineErrorCode, OfficeEngineRecovery};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum OfficeDocumentKind {
    Document,
    Spreadsheet,
    Presentation,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum OfficePathScope {
    Workspace,
    External,
    Attachment,
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
    /// Host-managed direct DOCX-to-PDF conversion. This mode never crosses the
    /// OfficeCLI provider boundary; execution is delegated to the pinned Word
    /// PDF render runtime.
    Pdf,
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
            Self::Pdf => "pdf",
            Self::Forms => "forms",
        }
    }

    pub fn writes_output(self) -> bool {
        matches!(self, Self::Html | Self::Svg | Self::Screenshot | Self::Pdf)
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

/// Host-resolved presentation screenshot plan frozen into an approved Office
/// execution. The original model request remains unchanged; this records the
/// deterministic slide set and geometry used to compile provider argv and to
/// verify that the resulting layout fits the final PNG viewport.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OfficePresentationRenderPlan {
    pub requested_pages: Vec<u32>,
    pub slide_width_emu: u32,
    pub slide_height_emu: u32,
    pub viewport: OfficeViewport,
    #[serde(deserialize_with = "deserialize_required_nullable")]
    pub grid: Option<OfficeGridLayout>,
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
/// `OfficeExecutionRequest.operation` is repeated outside this enum for compact UI routing. The
/// trusted compiler requires the envelope and parameter discriminants to agree.
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
