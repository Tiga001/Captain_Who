//! Model-facing Office intent contracts and their trusted canonical compiler.
//!
//! The semantic layer deliberately covers a finite set of common authoring
//! operations. It never exposes OfficeCLI argv, provider property names, or
//! document DOM paths to a model. Every accepted intent is compiled into the
//! existing [`OfficeExecutionRequest`] contract before permission checks,
//! preparation, approval, staging, and host-side execution take place.

use super::{
    types::office_agent_input_placeholder, validate_office_request, OfficeDocumentKind,
    OfficeElementPosition, OfficeExecutionRequest, OfficeGridLayout, OfficeOperation,
    OfficeOperationParameters, OfficePageRange, OfficePropertyMap, OfficeRequestParameters,
    OfficeTextReplacement, OfficeViewMode, OfficeViewRenderMode, OfficeViewport,
};
use crate::{AgentFileInputRef, AgentFileInputSpec};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::fmt;

/// Version of the provider-neutral model-facing Office intent contract.
pub const OFFICE_SEMANTIC_REQUEST_SCHEMA_VERSION: u32 = 1;

const MANAGED_SCRIPT_ROUTE: &str = "managedScript";
const MAX_TABLE_ROWS: usize = 2_000;
const MAX_TABLE_COLUMNS: usize = 256;
const MAX_CELL_TEXT_CHARS: usize = 32_768;

/// One trusted, versioned Office intent.
///
/// `reason` is deliberately not part of this domain object. It remains
/// untrusted display and audit metadata on the surrounding tool call.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OfficeSemanticRequest {
    pub schema_version: u32,
    pub document_kind: OfficeDocumentKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub destination_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,
    pub intent: OfficeSemanticIntent,
}

impl OfficeSemanticRequest {
    pub fn new(
        document_kind: OfficeDocumentKind,
        file_path: Option<String>,
        destination_path: Option<String>,
        timeout_ms: Option<u64>,
        intent: OfficeSemanticIntent,
    ) -> Self {
        Self {
            schema_version: OFFICE_SEMANTIC_REQUEST_SCHEMA_VERSION,
            document_kind,
            file_path,
            destination_path,
            timeout_ms,
            intent,
        }
    }

    pub fn operation_name(&self) -> &'static str {
        self.intent.stable_name()
    }
}

/// Finite semantic operation surface shared by all model providers.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "operation",
    content = "parameters",
    rename_all = "camelCase",
    deny_unknown_fields
)]
pub enum OfficeSemanticIntent {
    Create(OfficeCreateIntent),
    Inspect(OfficeInspectIntent),
    Validate,
    Render(OfficeRenderIntent),

    DocumentAddText(OfficeDocumentTextIntent),
    DocumentAddImage(OfficeImageIntent),
    DocumentAddTable(OfficeTableIntent),
    DocumentAddHeader(OfficeHeaderFooterIntent),
    DocumentAddFooter(OfficeHeaderFooterIntent),
    DocumentReplaceText(OfficeReplaceTextIntent),
    DocumentFormatText(OfficeDocumentFormatIntent),
    DocumentRemoveBlock(OfficeDocumentBlockIntent),
    DocumentMoveBlock(OfficeDocumentMoveIntent),

    SpreadsheetAddSheet(OfficeSpreadsheetSheetIntent),
    SpreadsheetWriteCell(OfficeSpreadsheetCellIntent),
    SpreadsheetSetFormula(OfficeSpreadsheetFormulaIntent),
    SpreadsheetFormatRange(OfficeSpreadsheetRangeFormatIntent),
    SpreadsheetFreezePanes(OfficeSpreadsheetFreezeIntent),
    SpreadsheetAddConditionalFormat(OfficeSpreadsheetConditionalFormatIntent),
    SpreadsheetAddTable(OfficeSpreadsheetTableIntent),
    SpreadsheetAddChart(OfficeSpreadsheetChartIntent),
    SpreadsheetAddImage(OfficeSpreadsheetImageIntent),
    SpreadsheetRemoveSheet(OfficeSpreadsheetSheetIntent),
    SpreadsheetMoveSheet(OfficeSpreadsheetMoveSheetIntent),

    PresentationAddSlide(OfficePresentationSlideIntent),
    PresentationAddText(OfficePresentationTextIntent),
    PresentationAddImage(OfficePresentationImageIntent),
    PresentationAddTable(OfficePresentationTableIntent),
    PresentationAddChart(OfficePresentationChartIntent),
    PresentationAddShape(OfficePresentationShapeIntent),
    PresentationAddFooter(OfficePresentationFooterIntent),
    PresentationRemoveSlide(OfficePresentationSlideIndexIntent),
    PresentationMoveSlide(OfficePresentationMoveSlideIntent),
}

impl OfficeSemanticIntent {
    pub fn stable_name(&self) -> &'static str {
        match self {
            Self::Create(_) => "create",
            Self::Inspect(_) => "inspect",
            Self::Validate => "validate",
            Self::Render(_) => "render",
            Self::DocumentAddText(_) => "addText",
            Self::DocumentAddImage(_) => "addImage",
            Self::DocumentAddTable(_) => "addTable",
            Self::DocumentAddHeader(_) => "addHeader",
            Self::DocumentAddFooter(_) => "addFooter",
            Self::DocumentReplaceText(_) => "replaceText",
            Self::DocumentFormatText(_) => "formatText",
            Self::DocumentRemoveBlock(_) => "removeBlock",
            Self::DocumentMoveBlock(_) => "moveBlock",
            Self::SpreadsheetAddSheet(_) => "addSheet",
            Self::SpreadsheetWriteCell(_) => "writeCell",
            Self::SpreadsheetSetFormula(_) => "setFormula",
            Self::SpreadsheetFormatRange(_) => "formatRange",
            Self::SpreadsheetFreezePanes(_) => "freezePanes",
            Self::SpreadsheetAddConditionalFormat(_) => "addConditionalFormat",
            Self::SpreadsheetAddTable(_) => "addTable",
            Self::SpreadsheetAddChart(_) => "addChart",
            Self::SpreadsheetAddImage(_) => "addImage",
            Self::SpreadsheetRemoveSheet(_) => "removeSheet",
            Self::SpreadsheetMoveSheet(_) => "moveSheet",
            Self::PresentationAddSlide(_) => "addSlide",
            Self::PresentationAddText(_) => "addText",
            Self::PresentationAddImage(_) => "addImage",
            Self::PresentationAddTable(_) => "addTable",
            Self::PresentationAddChart(_) => "addChart",
            Self::PresentationAddShape(_) => "addShape",
            Self::PresentationAddFooter(_) => "addFooter",
            Self::PresentationRemoveSlide(_) => "removeSlide",
            Self::PresentationMoveSlide(_) => "moveSlide",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OfficeCreateIntent {
    #[serde(default)]
    pub overwrite_existing: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub locale: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OfficeInspectIntent {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub block_index: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sheet_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub range: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub slide_number: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub depth: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OfficeRenderIntent {
    pub output_path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub page_or_slide: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sheet_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub range: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub viewport: Option<OfficeViewport>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum OfficeDocumentBlockKind {
    Paragraph,
    Heading1,
    Heading2,
    Heading3,
    Bullet,
    Numbered,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum OfficeHorizontalAlignment {
    Left,
    Center,
    Right,
    Justify,
}

impl OfficeHorizontalAlignment {
    fn provider_name(self) -> &'static str {
        match self {
            Self::Left => "left",
            Self::Center => "center",
            Self::Right => "right",
            Self::Justify => "justify",
        }
    }
}

/// A deliberately small cross-format style vocabulary.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OfficeSemanticStyle {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub font_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub font_size: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub font_color: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fill_color: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bold: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub italic: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub alignment: Option<OfficeHorizontalAlignment>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wrap_text: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub number_format: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line_spacing: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OfficeDocumentTextIntent {
    pub block_kind: OfficeDocumentBlockKind,
    pub text: String,
    #[serde(default)]
    pub style: OfficeSemanticStyle,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OfficeImageIntent {
    pub source: AgentFileInputRef,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub alt_text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub width: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub height: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OfficeTableIntent {
    pub data: Vec<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub style: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub width: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub header_fill: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OfficeHeaderFooterIntent {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(default)]
    pub page_number: bool,
    #[serde(default)]
    pub style: OfficeSemanticStyle,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OfficeReplaceTextIntent {
    pub find: String,
    pub replace: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OfficeDocumentFormatIntent {
    pub block_index: u32,
    pub style: OfficeSemanticStyle,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OfficeDocumentBlockIntent {
    pub block_index: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OfficeDocumentMoveIntent {
    pub block_index: u32,
    pub new_index: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OfficeSpreadsheetSheetIntent {
    pub sheet_name: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OfficeSpreadsheetCellIntent {
    pub sheet_name: String,
    pub cell: String,
    pub value: Value,
    #[serde(default)]
    pub style: OfficeSemanticStyle,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OfficeSpreadsheetFormulaIntent {
    pub sheet_name: String,
    pub cell: String,
    pub formula: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub number_format: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OfficeSpreadsheetRangeFormatIntent {
    pub sheet_name: String,
    pub range: String,
    pub style: OfficeSemanticStyle,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OfficeSpreadsheetFreezeIntent {
    pub sheet_name: String,
    pub freeze_at: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum OfficeConditionalFormatKind {
    CellValue,
    ColorScale,
    DataBar,
    ContainsText,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OfficeSpreadsheetConditionalFormatIntent {
    pub sheet_name: String,
    pub range: String,
    pub kind: OfficeConditionalFormatKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub operator: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub second_value: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fill_color: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub min_color: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mid_color: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_color: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OfficeSpreadsheetTableIntent {
    pub sheet_name: String,
    pub range: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub style: Option<String>,
    #[serde(default = "default_true")]
    pub header_row: bool,
    #[serde(default)]
    pub total_row: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum OfficeChartKind {
    Column,
    Bar,
    Line,
    Pie,
    Area,
    Scatter,
}

impl OfficeChartKind {
    fn provider_name(self) -> &'static str {
        match self {
            Self::Column => "column",
            Self::Bar => "bar",
            Self::Line => "line",
            Self::Pie => "pie",
            Self::Area => "area",
            Self::Scatter => "scatter",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OfficeChartSeries {
    pub name: String,
    pub values: Vec<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OfficeSpreadsheetChartIntent {
    pub sheet_name: String,
    pub chart_type: OfficeChartKind,
    pub title: String,
    pub data_range: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub category_range: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub anchor: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OfficeSpreadsheetImageIntent {
    pub sheet_name: String,
    pub source: AgentFileInputRef,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub alt_text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub anchor: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub width: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub height: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OfficeSpreadsheetMoveSheetIntent {
    pub sheet_name: String,
    pub new_index: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OfficePresentationSlideIntent {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub layout: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub background_color: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OfficePresentationTextIntent {
    pub slide_number: u32,
    pub text: String,
    pub x: String,
    pub y: String,
    pub width: String,
    pub height: String,
    #[serde(default)]
    pub style: OfficeSemanticStyle,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OfficePresentationImageIntent {
    pub slide_number: u32,
    pub source: AgentFileInputRef,
    pub x: String,
    pub y: String,
    pub width: String,
    pub height: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub alt_text: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OfficePresentationTableIntent {
    pub slide_number: u32,
    pub data: Vec<Vec<String>>,
    pub x: String,
    pub y: String,
    pub width: String,
    pub height: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub style: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub header_fill: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OfficePresentationChartIntent {
    pub slide_number: u32,
    pub chart_type: OfficeChartKind,
    pub title: String,
    pub series: Vec<OfficeChartSeries>,
    #[serde(default)]
    pub categories: Vec<String>,
    pub x: String,
    pub y: String,
    pub width: String,
    pub height: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OfficePresentationShapeIntent {
    pub slide_number: u32,
    pub shape_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    pub x: String,
    pub y: String,
    pub width: String,
    pub height: String,
    #[serde(default)]
    pub style: OfficeSemanticStyle,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line_color: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OfficePresentationFooterIntent {
    pub slide_number: u32,
    pub text: String,
    #[serde(default)]
    pub style: OfficeSemanticStyle,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OfficePresentationSlideIndexIntent {
    pub slide_number: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OfficePresentationMoveSlideIntent {
    pub slide_number: u32,
    pub new_index: u32,
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OfficeSemanticErrorCode {
    InvalidRequest,
    KindMismatch,
    CapabilityNotSupported,
}

impl OfficeSemanticErrorCode {
    pub fn stable_name(self) -> &'static str {
        match self {
            Self::InvalidRequest => "office.semantic_invalid_request",
            Self::KindMismatch => "office.semantic_kind_mismatch",
            Self::CapabilityNotSupported => "office.capability_not_supported",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OfficeSemanticError {
    code: OfficeSemanticErrorCode,
    message: String,
    capability: Option<String>,
}

impl OfficeSemanticError {
    pub fn invalid(message: impl Into<String>) -> Self {
        Self {
            code: OfficeSemanticErrorCode::InvalidRequest,
            message: message.into(),
            capability: None,
        }
    }

    fn kind(expected: OfficeDocumentKind, operation: &str) -> Self {
        Self {
            code: OfficeSemanticErrorCode::KindMismatch,
            message: format!(
                "Office semantic operation `{operation}` is not available for {expected:?} files."
            ),
            capability: Some(operation.to_string()),
        }
    }

    pub fn unsupported(capability: impl Into<String>, detail: impl Into<String>) -> Self {
        Self {
            code: OfficeSemanticErrorCode::CapabilityNotSupported,
            message: detail.into(),
            capability: Some(capability.into()),
        }
    }

    pub fn code(&self) -> OfficeSemanticErrorCode {
        self.code
    }

    pub fn message(&self) -> &str {
        &self.message
    }

    pub fn capability(&self) -> Option<&str> {
        self.capability.as_deref()
    }

    pub fn recommended_route(&self) -> Option<&'static str> {
        (self.code == OfficeSemanticErrorCode::CapabilityNotSupported)
            .then_some(MANAGED_SCRIPT_ROUTE)
    }
}

impl fmt::Display for OfficeSemanticError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for OfficeSemanticError {}

/// Compile one semantic intent into the existing canonical Office request.
pub fn compile_office_semantic_request(
    semantic: &OfficeSemanticRequest,
) -> Result<OfficeExecutionRequest, OfficeSemanticError> {
    if semantic.schema_version != OFFICE_SEMANTIC_REQUEST_SCHEMA_VERSION {
        return Err(OfficeSemanticError::invalid(format!(
            "Unsupported Office semantic request schema {}; expected {}.",
            semantic.schema_version, OFFICE_SEMANTIC_REQUEST_SCHEMA_VERSION
        )));
    }

    let file_path = semantic
        .file_path
        .as_deref()
        .map(str::trim)
        .filter(|path| !path.is_empty())
        .map(str::to_string);
    let destination_path = semantic
        .destination_path
        .as_deref()
        .map(str::trim)
        .filter(|path| !path.is_empty())
        .map(str::to_string);

    let (operation, parameters, output_path) = match &semantic.intent {
        OfficeSemanticIntent::Create(intent) => (
            OfficeOperation::Create,
            OfficeOperationParameters::Create {
                locale: match semantic.document_kind {
                    OfficeDocumentKind::Document => clean_optional(&intent.locale),
                    _ if intent.locale.is_some() => {
                        return Err(OfficeSemanticError::invalid(
                            "locale is supported only when creating a document.",
                        ))
                    }
                    _ => None,
                },
                minimal: false,
                overwrite: intent.overwrite_existing,
            },
            None,
        ),
        OfficeSemanticIntent::Inspect(intent) => (
            OfficeOperation::Get,
            OfficeOperationParameters::Get {
                target: inspect_target(semantic.document_kind, intent)?,
                depth: intent.depth,
            },
            None,
        ),
        OfficeSemanticIntent::Validate => (
            OfficeOperation::Validate,
            OfficeOperationParameters::Validate,
            None,
        ),
        OfficeSemanticIntent::Render(intent) => {
            let output = non_empty(&intent.output_path, "outputPath")?.to_string();
            let mut pages = Vec::new();
            if let Some(page) = intent.page_or_slide {
                require_one_based(page, "pageOrSlide")?;
                pages.push(OfficePageRange {
                    start: page,
                    end: Some(page),
                });
            }
            (
                OfficeOperation::View,
                OfficeOperationParameters::View {
                    mode: OfficeViewMode::Screenshot,
                    start: None,
                    end: None,
                    max_lines: None,
                    issue_type: None,
                    limit: None,
                    columns: Vec::new(),
                    pages,
                    range: render_range(semantic.document_kind, intent)?,
                    viewport: intent.viewport.clone(),
                    grid: (semantic.document_kind != OfficeDocumentKind::Spreadsheet)
                        .then_some(OfficeGridLayout::Auto),
                    render_mode: (semantic.document_kind != OfficeDocumentKind::Spreadsheet)
                        .then_some(OfficeViewRenderMode::Auto),
                    page_count: false,
                },
                Some(output),
            )
        }
        OfficeSemanticIntent::DocumentAddText(intent) => {
            require_kind(
                semantic.document_kind,
                OfficeDocumentKind::Document,
                semantic.operation_name(),
            )?;
            let text = non_empty(&intent.text, "text")?;
            let mut properties = document_text_properties(intent.block_kind, &intent.style)?;
            properties.insert("text".to_string(), json!(text));
            add("/body", "paragraph", properties)
        }
        OfficeSemanticIntent::DocumentAddImage(intent) => {
            require_kind(
                semantic.document_kind,
                OfficeDocumentKind::Document,
                semantic.operation_name(),
            )?;
            add("/body", "picture", image_properties(intent)?)
        }
        OfficeSemanticIntent::DocumentAddTable(intent) => {
            require_kind(
                semantic.document_kind,
                OfficeDocumentKind::Document,
                semantic.operation_name(),
            )?;
            add("/body", "table", table_properties(intent)?)
        }
        OfficeSemanticIntent::DocumentAddHeader(intent) => {
            require_kind(
                semantic.document_kind,
                OfficeDocumentKind::Document,
                semantic.operation_name(),
            )?;
            add("/", "header", header_footer_properties(intent)?)
        }
        OfficeSemanticIntent::DocumentAddFooter(intent) => {
            require_kind(
                semantic.document_kind,
                OfficeDocumentKind::Document,
                semantic.operation_name(),
            )?;
            add("/", "footer", header_footer_properties(intent)?)
        }
        OfficeSemanticIntent::DocumentReplaceText(intent) => {
            require_kind(
                semantic.document_kind,
                OfficeDocumentKind::Document,
                semantic.operation_name(),
            )?;
            let find = non_empty(&intent.find, "findText")?.to_string();
            (
                OfficeOperation::Set,
                OfficeOperationParameters::Set {
                    target: "/body".to_string(),
                    properties: BTreeMap::new(),
                    replacement: Some(OfficeTextReplacement {
                        find,
                        replace: intent.replace.clone(),
                    }),
                    force: false,
                },
                None,
            )
        }
        OfficeSemanticIntent::DocumentFormatText(intent) => {
            require_kind(
                semantic.document_kind,
                OfficeDocumentKind::Document,
                semantic.operation_name(),
            )?;
            require_one_based(intent.block_index, "blockIndex")?;
            let properties = common_style_properties(&intent.style, StyleTarget::Document)?;
            if properties.is_empty() {
                return Err(OfficeSemanticError::invalid(
                    "formatText requires at least one style field.",
                ));
            }
            set(format!("/body/p[{}]", intent.block_index), properties)
        }
        OfficeSemanticIntent::DocumentRemoveBlock(intent) => {
            require_kind(
                semantic.document_kind,
                OfficeDocumentKind::Document,
                semantic.operation_name(),
            )?;
            require_one_based(intent.block_index, "blockIndex")?;
            remove(format!("/body/*[{}]", intent.block_index))
        }
        OfficeSemanticIntent::DocumentMoveBlock(intent) => {
            require_kind(
                semantic.document_kind,
                OfficeDocumentKind::Document,
                semantic.operation_name(),
            )?;
            require_one_based(intent.block_index, "blockIndex")?;
            require_one_based(intent.new_index, "newIndex")?;
            move_element(
                format!("/body/*[{}]", intent.block_index),
                OfficeElementPosition::Index {
                    index: intent.new_index - 1,
                },
            )
        }
        OfficeSemanticIntent::SpreadsheetAddSheet(intent) => {
            require_kind(
                semantic.document_kind,
                OfficeDocumentKind::Spreadsheet,
                semantic.operation_name(),
            )?;
            let sheet = validate_sheet_name(&intent.sheet_name)?;
            add(
                "/",
                "sheet",
                [("name".to_string(), json!(sheet))].into_iter().collect(),
            )
        }
        OfficeSemanticIntent::SpreadsheetWriteCell(intent) => {
            require_kind(
                semantic.document_kind,
                OfficeDocumentKind::Spreadsheet,
                semantic.operation_name(),
            )?;
            validate_cell_value(&intent.value)?;
            let target = worksheet_target(&intent.sheet_name, &intent.cell)?;
            let mut properties = common_style_properties(&intent.style, StyleTarget::Spreadsheet)?;
            properties.insert("value".to_string(), intent.value.clone());
            set(target, properties)
        }
        OfficeSemanticIntent::SpreadsheetSetFormula(intent) => {
            require_kind(
                semantic.document_kind,
                OfficeDocumentKind::Spreadsheet,
                semantic.operation_name(),
            )?;
            let formula = non_empty(&intent.formula, "formula")?
                .trim_start_matches('=')
                .to_string();
            if formula.is_empty() {
                return Err(OfficeSemanticError::invalid(
                    "formula must contain an expression after '='.",
                ));
            }
            let mut properties = OfficePropertyMap::new();
            properties.insert("formula".to_string(), json!(formula));
            if let Some(format) = clean_optional(&intent.number_format) {
                properties.insert("numberformat".to_string(), json!(format));
            }
            set(
                worksheet_target(&intent.sheet_name, &intent.cell)?,
                properties,
            )
        }
        OfficeSemanticIntent::SpreadsheetFormatRange(intent) => {
            require_kind(
                semantic.document_kind,
                OfficeDocumentKind::Spreadsheet,
                semantic.operation_name(),
            )?;
            let properties = common_style_properties(&intent.style, StyleTarget::Spreadsheet)?;
            if properties.is_empty() {
                return Err(OfficeSemanticError::invalid(
                    "formatRange requires at least one style field.",
                ));
            }
            set(
                worksheet_target(&intent.sheet_name, &intent.range)?,
                properties,
            )
        }
        OfficeSemanticIntent::SpreadsheetFreezePanes(intent) => {
            require_kind(
                semantic.document_kind,
                OfficeDocumentKind::Spreadsheet,
                semantic.operation_name(),
            )?;
            validate_a1_reference(&intent.freeze_at, false)?;
            set(
                worksheet_root(&intent.sheet_name)?,
                [("freeze".to_string(), json!(intent.freeze_at.trim()))]
                    .into_iter()
                    .collect(),
            )
        }
        OfficeSemanticIntent::SpreadsheetAddConditionalFormat(intent) => {
            require_kind(
                semantic.document_kind,
                OfficeDocumentKind::Spreadsheet,
                semantic.operation_name(),
            )?;
            let parent = worksheet_root(&intent.sheet_name)?;
            validate_a1_reference(&intent.range, true)?;
            let properties = conditional_format_properties(intent)?;
            add(parent, "conditionalformatting", properties)
        }
        OfficeSemanticIntent::SpreadsheetAddTable(intent) => {
            require_kind(
                semantic.document_kind,
                OfficeDocumentKind::Spreadsheet,
                semantic.operation_name(),
            )?;
            validate_a1_reference(&intent.range, true)?;
            let mut properties = OfficePropertyMap::new();
            properties.insert("ref".to_string(), json!(intent.range.trim()));
            properties.insert("headerRow".to_string(), json!(intent.header_row));
            properties.insert("totalRow".to_string(), json!(intent.total_row));
            if let Some(name) = clean_optional(&intent.name) {
                properties.insert("displayName".to_string(), json!(name));
            }
            if let Some(style) = clean_optional(&intent.style) {
                properties.insert("style".to_string(), json!(style));
            }
            add(worksheet_root(&intent.sheet_name)?, "table", properties)
        }
        OfficeSemanticIntent::SpreadsheetAddChart(intent) => {
            require_kind(
                semantic.document_kind,
                OfficeDocumentKind::Spreadsheet,
                semantic.operation_name(),
            )?;
            validate_sheet_name(&intent.sheet_name)?;
            validate_a1_reference(&intent.data_range, true)?;
            let mut properties = OfficePropertyMap::new();
            properties.insert(
                "chartType".to_string(),
                json!(intent.chart_type.provider_name()),
            );
            properties.insert(
                "title".to_string(),
                json!(non_empty(&intent.title, "title")?),
            );
            properties.insert(
                "dataRange".to_string(),
                json!(qualify_range(&intent.sheet_name, &intent.data_range)?),
            );
            if let Some(categories) = clean_optional(&intent.category_range) {
                validate_a1_reference(&categories, true)?;
                properties.insert(
                    "categories".to_string(),
                    json!(qualify_range(&intent.sheet_name, &categories)?),
                );
            }
            if let Some(anchor) = clean_optional(&intent.anchor) {
                properties.insert("anchor".to_string(), json!(anchor));
            }
            add(worksheet_root(&intent.sheet_name)?, "chart", properties)
        }
        OfficeSemanticIntent::SpreadsheetAddImage(intent) => {
            require_kind(
                semantic.document_kind,
                OfficeDocumentKind::Spreadsheet,
                semantic.operation_name(),
            )?;
            let mut properties = image_source_properties(&intent.source)?;
            insert_optional_string(&mut properties, "alt", &intent.alt_text);
            insert_optional_string(&mut properties, "anchor", &intent.anchor);
            insert_optional_string(&mut properties, "width", &intent.width);
            insert_optional_string(&mut properties, "height", &intent.height);
            add(worksheet_root(&intent.sheet_name)?, "picture", properties)
        }
        OfficeSemanticIntent::SpreadsheetRemoveSheet(intent) => {
            require_kind(
                semantic.document_kind,
                OfficeDocumentKind::Spreadsheet,
                semantic.operation_name(),
            )?;
            remove(worksheet_root(&intent.sheet_name)?)
        }
        OfficeSemanticIntent::SpreadsheetMoveSheet(intent) => {
            require_kind(
                semantic.document_kind,
                OfficeDocumentKind::Spreadsheet,
                semantic.operation_name(),
            )?;
            require_one_based(intent.new_index, "newIndex")?;
            move_element(
                worksheet_root(&intent.sheet_name)?,
                OfficeElementPosition::Index {
                    index: intent.new_index - 1,
                },
            )
        }
        OfficeSemanticIntent::PresentationAddSlide(intent) => {
            require_kind(
                semantic.document_kind,
                OfficeDocumentKind::Presentation,
                semantic.operation_name(),
            )?;
            let mut properties = OfficePropertyMap::new();
            insert_optional_string(&mut properties, "layout", &intent.layout);
            insert_optional_string(&mut properties, "title", &intent.title);
            insert_optional_string(&mut properties, "text", &intent.body);
            insert_optional_string(&mut properties, "background", &intent.background_color);
            add("/", "slide", properties)
        }
        OfficeSemanticIntent::PresentationAddText(intent) => {
            require_kind(
                semantic.document_kind,
                OfficeDocumentKind::Presentation,
                semantic.operation_name(),
            )?;
            let mut properties = common_style_properties(&intent.style, StyleTarget::Presentation)?;
            properties.insert("text".to_string(), json!(non_empty(&intent.text, "text")?));
            insert_box_properties(
                &mut properties,
                &intent.x,
                &intent.y,
                &intent.width,
                &intent.height,
            )?;
            add(slide_root(intent.slide_number)?, "textbox", properties)
        }
        OfficeSemanticIntent::PresentationAddImage(intent) => {
            require_kind(
                semantic.document_kind,
                OfficeDocumentKind::Presentation,
                semantic.operation_name(),
            )?;
            let mut properties = image_source_properties(&intent.source)?;
            insert_optional_string(&mut properties, "alt", &intent.alt_text);
            insert_box_properties(
                &mut properties,
                &intent.x,
                &intent.y,
                &intent.width,
                &intent.height,
            )?;
            add(slide_root(intent.slide_number)?, "picture", properties)
        }
        OfficeSemanticIntent::PresentationAddTable(intent) => {
            require_kind(
                semantic.document_kind,
                OfficeDocumentKind::Presentation,
                semantic.operation_name(),
            )?;
            let mut properties = table_data_properties(&intent.data)?;
            insert_box_properties(
                &mut properties,
                &intent.x,
                &intent.y,
                &intent.width,
                &intent.height,
            )?;
            insert_optional_string(&mut properties, "style", &intent.style);
            insert_optional_string(&mut properties, "headerFill", &intent.header_fill);
            add(slide_root(intent.slide_number)?, "table", properties)
        }
        OfficeSemanticIntent::PresentationAddChart(intent) => {
            require_kind(
                semantic.document_kind,
                OfficeDocumentKind::Presentation,
                semantic.operation_name(),
            )?;
            let mut properties = OfficePropertyMap::new();
            properties.insert(
                "chartType".to_string(),
                json!(intent.chart_type.provider_name()),
            );
            properties.insert(
                "title".to_string(),
                json!(non_empty(&intent.title, "title")?),
            );
            properties.insert(
                "data".to_string(),
                json!(encode_chart_series(&intent.series)?),
            );
            if !intent.categories.is_empty() {
                properties.insert(
                    "categories".to_string(),
                    json!(encode_delimited_values(&intent.categories, "categories")?),
                );
            }
            insert_box_properties(
                &mut properties,
                &intent.x,
                &intent.y,
                &intent.width,
                &intent.height,
            )?;
            add(slide_root(intent.slide_number)?, "chart", properties)
        }
        OfficeSemanticIntent::PresentationAddShape(intent) => {
            require_kind(
                semantic.document_kind,
                OfficeDocumentKind::Presentation,
                semantic.operation_name(),
            )?;
            let mut properties = common_style_properties(&intent.style, StyleTarget::Presentation)?;
            properties.insert(
                "geometry".to_string(),
                json!(non_empty(&intent.shape_type, "shapeType")?),
            );
            insert_optional_string(&mut properties, "text", &intent.text);
            insert_optional_string(&mut properties, "line", &intent.line_color);
            insert_box_properties(
                &mut properties,
                &intent.x,
                &intent.y,
                &intent.width,
                &intent.height,
            )?;
            add(slide_root(intent.slide_number)?, "shape", properties)
        }
        OfficeSemanticIntent::PresentationAddFooter(intent) => {
            require_kind(
                semantic.document_kind,
                OfficeDocumentKind::Presentation,
                semantic.operation_name(),
            )?;
            let mut properties = common_style_properties(&intent.style, StyleTarget::Presentation)?;
            properties.insert("text".to_string(), json!(non_empty(&intent.text, "text")?));
            properties.insert("x".to_string(), json!("0.5in"));
            properties.insert("y".to_string(), json!("7.05in"));
            properties.insert("width".to_string(), json!("12.33in"));
            properties.insert("height".to_string(), json!("0.25in"));
            add(slide_root(intent.slide_number)?, "textbox", properties)
        }
        OfficeSemanticIntent::PresentationRemoveSlide(intent) => {
            require_kind(
                semantic.document_kind,
                OfficeDocumentKind::Presentation,
                semantic.operation_name(),
            )?;
            remove(slide_root(intent.slide_number)?)
        }
        OfficeSemanticIntent::PresentationMoveSlide(intent) => {
            require_kind(
                semantic.document_kind,
                OfficeDocumentKind::Presentation,
                semantic.operation_name(),
            )?;
            require_one_based(intent.new_index, "newIndex")?;
            move_element(
                slide_root(intent.slide_number)?,
                OfficeElementPosition::Index {
                    index: intent.new_index - 1,
                },
            )
        }
    };

    if !matches!(semantic.intent, OfficeSemanticIntent::Create(_)) && file_path.is_none() {
        return Err(OfficeSemanticError::invalid(format!(
            "{} requires a non-empty filePath.",
            semantic.operation_name()
        )));
    }
    if matches!(semantic.intent, OfficeSemanticIntent::Create(_)) && file_path.is_none() {
        return Err(OfficeSemanticError::invalid(
            "create requires a non-empty filePath.",
        ));
    }

    let request = OfficeExecutionRequest {
        document_kind: semantic.document_kind,
        operation,
        document_path: file_path,
        parameters: OfficeRequestParameters::Typed(parameters),
        output_path,
        destination_path,
        inputs: semantic_input_specs(&semantic.intent),
        timeout_ms: semantic.timeout_ms,
    };
    validate_office_request(&request).map_err(|error| {
        OfficeSemanticError::invalid(format!(
            "The semantic Office request could not be compiled safely: {}",
            error.message()
        ))
    })?;
    Ok(request)
}

fn add(
    parent: impl Into<String>,
    element_type: impl Into<String>,
    properties: OfficePropertyMap,
) -> (OfficeOperation, OfficeOperationParameters, Option<String>) {
    (
        OfficeOperation::Add,
        OfficeOperationParameters::Add {
            parent: parent.into(),
            element_type: element_type.into(),
            copy_from: None,
            position: None,
            properties,
            force: false,
        },
        None,
    )
}

fn set(
    target: impl Into<String>,
    properties: OfficePropertyMap,
) -> (OfficeOperation, OfficeOperationParameters, Option<String>) {
    (
        OfficeOperation::Set,
        OfficeOperationParameters::Set {
            target: target.into(),
            properties,
            replacement: None,
            force: false,
        },
        None,
    )
}

fn remove(
    target: impl Into<String>,
) -> (OfficeOperation, OfficeOperationParameters, Option<String>) {
    (
        OfficeOperation::Remove,
        OfficeOperationParameters::Remove {
            target: target.into(),
            shift: None,
            properties: BTreeMap::new(),
        },
        None,
    )
}

fn move_element(
    target: impl Into<String>,
    position: OfficeElementPosition,
) -> (OfficeOperation, OfficeOperationParameters, Option<String>) {
    (
        OfficeOperation::Move,
        OfficeOperationParameters::Move {
            target: target.into(),
            new_parent: None,
            position: Some(position),
            properties: BTreeMap::new(),
        },
        None,
    )
}

fn require_kind(
    actual: OfficeDocumentKind,
    expected: OfficeDocumentKind,
    operation: &str,
) -> Result<(), OfficeSemanticError> {
    if actual == expected {
        Ok(())
    } else {
        Err(OfficeSemanticError::kind(actual, operation))
    }
}

fn inspect_target(
    kind: OfficeDocumentKind,
    intent: &OfficeInspectIntent,
) -> Result<Option<String>, OfficeSemanticError> {
    match kind {
        OfficeDocumentKind::Document => match intent.block_index {
            Some(index) => {
                require_one_based(index, "blockIndex")?;
                Ok(Some(format!("/body/*[{index}]")))
            }
            None => Ok(None),
        },
        OfficeDocumentKind::Spreadsheet => {
            let Some(sheet) = intent.sheet_name.as_deref() else {
                if intent.range.is_some() {
                    return Err(OfficeSemanticError::invalid(
                        "Spreadsheet inspect with range also requires sheetName.",
                    ));
                }
                return Ok(None);
            };
            if let Some(range) = intent.range.as_deref() {
                Ok(Some(worksheet_target(sheet, range)?))
            } else {
                Ok(Some(worksheet_root(sheet)?))
            }
        }
        OfficeDocumentKind::Presentation => match intent.slide_number {
            Some(slide) => Ok(Some(slide_root(slide)?)),
            None => Ok(None),
        },
    }
}

fn render_range(
    kind: OfficeDocumentKind,
    intent: &OfficeRenderIntent,
) -> Result<Option<String>, OfficeSemanticError> {
    if kind != OfficeDocumentKind::Spreadsheet {
        if intent.sheet_name.is_some() || intent.range.is_some() {
            return Err(OfficeSemanticError::invalid(
                "sheetName and range are available only when rendering a spreadsheet.",
            ));
        }
        return Ok(None);
    }
    match (&intent.sheet_name, &intent.range) {
        (Some(sheet), Some(range)) => Ok(Some(qualify_range(sheet, range)?)),
        (Some(sheet), None) => Ok(Some(validate_sheet_name(sheet)?.to_string())),
        (None, Some(_)) => Err(OfficeSemanticError::invalid(
            "Spreadsheet render with range also requires sheetName.",
        )),
        (None, None) => Ok(None),
    }
}

fn document_text_properties(
    kind: OfficeDocumentBlockKind,
    style: &OfficeSemanticStyle,
) -> Result<OfficePropertyMap, OfficeSemanticError> {
    let mut properties = common_style_properties(style, StyleTarget::Document)?;
    match kind {
        OfficeDocumentBlockKind::Paragraph => {}
        OfficeDocumentBlockKind::Heading1 => {
            properties.insert("style".to_string(), json!("Heading1"));
        }
        OfficeDocumentBlockKind::Heading2 => {
            properties.insert("style".to_string(), json!("Heading2"));
        }
        OfficeDocumentBlockKind::Heading3 => {
            properties.insert("style".to_string(), json!("Heading3"));
        }
        OfficeDocumentBlockKind::Bullet => {
            properties.insert("listStyle".to_string(), json!("bullet"));
        }
        OfficeDocumentBlockKind::Numbered => {
            properties.insert("listStyle".to_string(), json!("ordered"));
        }
    }
    Ok(properties)
}

#[derive(Debug, Clone, Copy)]
enum StyleTarget {
    Document,
    Spreadsheet,
    Presentation,
}

fn common_style_properties(
    style: &OfficeSemanticStyle,
    target: StyleTarget,
) -> Result<OfficePropertyMap, OfficeSemanticError> {
    if let Some(size) = style.font_size {
        if !size.is_finite() || !(1.0..=400.0).contains(&size) {
            return Err(OfficeSemanticError::invalid(
                "fontSize must be a finite number between 1 and 400 points.",
            ));
        }
    }
    if let Some(line_spacing) = style.line_spacing {
        if !line_spacing.is_finite() || !(0.5..=10.0).contains(&line_spacing) {
            return Err(OfficeSemanticError::invalid(
                "lineSpacing must be between 0.5 and 10.",
            ));
        }
        if matches!(target, StyleTarget::Spreadsheet) {
            return Err(OfficeSemanticError::unsupported(
                "spreadsheet.lineSpacing",
                "Spreadsheet line spacing is outside the native semantic surface; use the managed script route.",
            ));
        }
    }

    let mut properties = OfficePropertyMap::new();
    if let Some(font) = clean_optional(&style.font_name) {
        let key = match target {
            StyleTarget::Spreadsheet => "font.name",
            StyleTarget::Document | StyleTarget::Presentation => "font",
        };
        properties.insert(key.to_string(), json!(font));
    }
    if let Some(size) = style.font_size {
        let key = match target {
            StyleTarget::Spreadsheet => "font.size",
            StyleTarget::Document | StyleTarget::Presentation => "size",
        };
        properties.insert(key.to_string(), json!(size));
    }
    if let Some(color) = clean_optional(&style.font_color) {
        let key = match target {
            StyleTarget::Spreadsheet => "font.color",
            StyleTarget::Document | StyleTarget::Presentation => "color",
        };
        properties.insert(key.to_string(), json!(color));
    }
    if let Some(fill) = clean_optional(&style.fill_color) {
        properties.insert("fill".to_string(), json!(fill));
    }
    if let Some(bold) = style.bold {
        let key = if matches!(target, StyleTarget::Spreadsheet) {
            "font.bold"
        } else {
            "bold"
        };
        properties.insert(key.to_string(), json!(bold));
    }
    if let Some(italic) = style.italic {
        let key = if matches!(target, StyleTarget::Spreadsheet) {
            "font.italic"
        } else {
            "italic"
        };
        properties.insert(key.to_string(), json!(italic));
    }
    if let Some(alignment) = style.alignment {
        let key = if matches!(target, StyleTarget::Spreadsheet) {
            "alignment.horizontal"
        } else {
            "align"
        };
        properties.insert(key.to_string(), json!(alignment.provider_name()));
    }
    if let Some(wrap) = style.wrap_text {
        if !matches!(target, StyleTarget::Spreadsheet) {
            return Err(OfficeSemanticError::unsupported(
                "text.wrap",
                "wrapText is currently available only for spreadsheet cells and ranges; use the managed script route for advanced document wrapping.",
            ));
        }
        properties.insert("alignment.wrapText".to_string(), json!(wrap));
    }
    if let Some(format) = clean_optional(&style.number_format) {
        if !matches!(target, StyleTarget::Spreadsheet) {
            return Err(OfficeSemanticError::invalid(
                "numberFormat is available only for spreadsheet cells and ranges.",
            ));
        }
        properties.insert("numberformat".to_string(), json!(format));
    }
    if let Some(line_spacing) = style.line_spacing {
        properties.insert("lineSpacing".to_string(), json!(line_spacing));
    }
    Ok(properties)
}

fn image_properties(intent: &OfficeImageIntent) -> Result<OfficePropertyMap, OfficeSemanticError> {
    let mut properties = image_source_properties(&intent.source)?;
    insert_optional_string(&mut properties, "alt", &intent.alt_text);
    insert_optional_string(&mut properties, "width", &intent.width);
    insert_optional_string(&mut properties, "height", &intent.height);
    Ok(properties)
}

fn image_source_properties(
    source: &AgentFileInputRef,
) -> Result<OfficePropertyMap, OfficeSemanticError> {
    validate_image_source_hint(source)?;
    let logical_path = office_agent_input_placeholder(&office_image_mount_path(source));
    Ok(
        [("src".to_string(), json!({ "resourcePath": logical_path }))]
            .into_iter()
            .collect(),
    )
}

fn semantic_input_specs(intent: &OfficeSemanticIntent) -> Vec<AgentFileInputSpec> {
    let source = match intent {
        OfficeSemanticIntent::DocumentAddImage(intent) => Some(&intent.source),
        OfficeSemanticIntent::SpreadsheetAddImage(intent) => Some(&intent.source),
        OfficeSemanticIntent::PresentationAddImage(intent) => Some(&intent.source),
        _ => None,
    };
    source
        .map(|source| {
            vec![AgentFileInputSpec {
                mount_path: office_image_mount_path(source),
                source: source.clone(),
            }]
        })
        .unwrap_or_default()
}

fn validate_image_source_hint(source: &AgentFileInputRef) -> Result<(), OfficeSemanticError> {
    let (value, field) = match source {
        AgentFileInputRef::Attachment { read_path } => (read_path, "source.readPath"),
        AgentFileInputRef::Workspace { path } | AgentFileInputRef::External { path } => {
            (path, "source.path")
        }
        AgentFileInputRef::GeneratedArtifact { uri, path } => {
            non_empty(uri, "source.uri")?;
            (path, "source.path")
        }
        AgentFileInputRef::SkillResource { uri } => (uri, "source.uri"),
    };
    non_empty(value, field)?;
    Ok(())
}

fn office_image_mount_path(source: &AgentFileInputRef) -> String {
    let hint = match source {
        AgentFileInputRef::Attachment { read_path } => read_path.as_str(),
        AgentFileInputRef::Workspace { path }
        | AgentFileInputRef::External { path }
        | AgentFileInputRef::GeneratedArtifact { path, .. } => path.as_str(),
        AgentFileInputRef::SkillResource { uri } => uri.as_str(),
    };
    let extension = hint
        .split(['?', '#'])
        .next()
        .and_then(|value| value.rsplit(['/', '\\']).next())
        .and_then(|name| name.rsplit_once('.').map(|(_, extension)| extension))
        .map(str::trim)
        .filter(|extension| {
            !extension.is_empty()
                && extension.len() <= 10
                && extension
                    .chars()
                    .all(|character| character.is_ascii_alphanumeric())
        })
        .map(str::to_ascii_lowercase)
        .unwrap_or_else(|| "bin".to_string());
    format!("office/image-input-1.{extension}")
}

fn table_properties(intent: &OfficeTableIntent) -> Result<OfficePropertyMap, OfficeSemanticError> {
    let mut properties = table_data_properties(&intent.data)?;
    insert_optional_string(&mut properties, "style", &intent.style);
    insert_optional_string(&mut properties, "width", &intent.width);
    insert_optional_string(&mut properties, "headerFill", &intent.header_fill);
    Ok(properties)
}

fn table_data_properties(data: &[Vec<String>]) -> Result<OfficePropertyMap, OfficeSemanticError> {
    Ok([("data".to_string(), json!(encode_table_data(data)?))]
        .into_iter()
        .collect())
}

fn header_footer_properties(
    intent: &OfficeHeaderFooterIntent,
) -> Result<OfficePropertyMap, OfficeSemanticError> {
    if intent
        .text
        .as_ref()
        .is_none_or(|text| text.trim().is_empty())
        && !intent.page_number
    {
        return Err(OfficeSemanticError::invalid(
            "Header or footer requires text, pageNumber=true, or both.",
        ));
    }
    let mut properties = common_style_properties(&intent.style, StyleTarget::Document)?;
    properties.insert("type".to_string(), json!("default"));
    insert_optional_string(&mut properties, "text", &intent.text);
    if intent.page_number {
        properties.insert("field".to_string(), json!("PAGE"));
    }
    Ok(properties)
}

fn conditional_format_properties(
    intent: &OfficeSpreadsheetConditionalFormatIntent,
) -> Result<OfficePropertyMap, OfficeSemanticError> {
    let mut properties = OfficePropertyMap::new();
    properties.insert("ref".to_string(), json!(intent.range.trim()));
    match intent.kind {
        OfficeConditionalFormatKind::CellValue => {
            let operator = intent
                .operator
                .as_deref()
                .map(str::trim)
                .filter(|v| !v.is_empty());
            let value = intent.value.as_ref();
            if operator.is_none() || value.is_none() {
                return Err(OfficeSemanticError::invalid(
                    "cellValue conditional formatting requires operator and value.",
                ));
            }
            validate_cell_value(value.unwrap())?;
            properties.insert("type".to_string(), json!("cellIs"));
            properties.insert("operator".to_string(), json!(operator.unwrap()));
            properties.insert("value".to_string(), value.unwrap().clone());
            if let Some(second) = &intent.second_value {
                validate_cell_value(second)?;
                properties.insert("value2".to_string(), second.clone());
            }
            insert_optional_string(&mut properties, "fill", &intent.fill_color);
        }
        OfficeConditionalFormatKind::ColorScale => {
            properties.insert("type".to_string(), json!("colorScale"));
            insert_optional_string(&mut properties, "minColor", &intent.min_color);
            insert_optional_string(&mut properties, "midColor", &intent.mid_color);
            insert_optional_string(&mut properties, "maxColor", &intent.max_color);
            if !properties.contains_key("minColor") || !properties.contains_key("maxColor") {
                return Err(OfficeSemanticError::invalid(
                    "colorScale conditional formatting requires minColor and maxColor.",
                ));
            }
        }
        OfficeConditionalFormatKind::DataBar => {
            properties.insert("type".to_string(), json!("dataBar"));
            if let Some(value) = &intent.value {
                validate_cell_value(value)?;
                properties.insert("min".to_string(), value.clone());
            }
            if let Some(max) = &intent.second_value {
                validate_cell_value(max)?;
                properties.insert("max".to_string(), max.clone());
            }
            insert_optional_string(&mut properties, "color", &intent.fill_color);
        }
        OfficeConditionalFormatKind::ContainsText => {
            properties.insert("type".to_string(), json!("containsText"));
            properties.insert(
                "text".to_string(),
                json!(non_empty(
                    intent.text.as_deref().unwrap_or_default(),
                    "text"
                )?),
            );
            insert_optional_string(&mut properties, "fill", &intent.fill_color);
        }
    }
    Ok(properties)
}

fn encode_table_data(data: &[Vec<String>]) -> Result<String, OfficeSemanticError> {
    if data.is_empty() || data.len() > MAX_TABLE_ROWS {
        return Err(OfficeSemanticError::invalid(format!(
            "tableData must contain 1 to {MAX_TABLE_ROWS} rows."
        )));
    }
    let columns = data[0].len();
    if columns == 0 || columns > MAX_TABLE_COLUMNS {
        return Err(OfficeSemanticError::invalid(format!(
            "tableData must contain 1 to {MAX_TABLE_COLUMNS} columns."
        )));
    }
    let mut rows = Vec::with_capacity(data.len());
    for row in data {
        if row.len() != columns {
            return Err(OfficeSemanticError::invalid(
                "Every tableData row must have the same number of columns.",
            ));
        }
        let mut cells = Vec::with_capacity(columns);
        for cell in row {
            if cell.chars().count() > MAX_CELL_TEXT_CHARS {
                return Err(OfficeSemanticError::invalid(
                    "A tableData cell exceeds the supported text limit.",
                ));
            }
            if cell.contains([',', ';', '\r', '\n']) {
                return Err(OfficeSemanticError::unsupported(
                    "tableData.delimitedCell",
                    "Native Office table seeding cannot safely represent cells containing comma, semicolon, or line breaks; use the managed script route.",
                ));
            }
            cells.push(cell.clone());
        }
        rows.push(cells.join(","));
    }
    Ok(rows.join(";"))
}

fn encode_chart_series(series: &[OfficeChartSeries]) -> Result<String, OfficeSemanticError> {
    if series.is_empty() || series.len() > 64 {
        return Err(OfficeSemanticError::invalid(
            "chart series must contain between 1 and 64 entries.",
        ));
    }
    let mut encoded = Vec::with_capacity(series.len());
    for item in series {
        let name = non_empty(&item.name, "series.name")?;
        if name.contains([':', ';', ',', '\r', '\n']) {
            return Err(OfficeSemanticError::unsupported(
                "chart.seriesName",
                "Native chart series names cannot contain ':', ',', ';', or line breaks; use the managed script route.",
            ));
        }
        if item.values.is_empty() || item.values.len() > 10_000 {
            return Err(OfficeSemanticError::invalid(
                "Each chart series must contain between 1 and 10000 values.",
            ));
        }
        if item.values.iter().any(|value| !value.is_finite()) {
            return Err(OfficeSemanticError::invalid(
                "Chart series values must be finite numbers.",
            ));
        }
        let values = item
            .values
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join(",");
        encoded.push(format!("{name}:{values}"));
    }
    Ok(encoded.join(";"))
}

fn encode_delimited_values(values: &[String], field: &str) -> Result<String, OfficeSemanticError> {
    if values.len() > 10_000 {
        return Err(OfficeSemanticError::invalid(format!(
            "{field} contains too many values."
        )));
    }
    for value in values {
        if value.contains([',', ';', '\r', '\n']) {
            return Err(OfficeSemanticError::unsupported(
                format!("{field}.delimitedValue"),
                format!(
                    "Native {field} values cannot contain comma, semicolon, or line breaks; use the managed script route."
                ),
            ));
        }
    }
    Ok(values.join(","))
}

fn insert_box_properties(
    properties: &mut OfficePropertyMap,
    x: &str,
    y: &str,
    width: &str,
    height: &str,
) -> Result<(), OfficeSemanticError> {
    for (name, value) in [("x", x), ("y", y), ("width", width), ("height", height)] {
        properties.insert(name.to_string(), json!(non_empty(value, name)?));
    }
    Ok(())
}

fn insert_optional_string(properties: &mut OfficePropertyMap, name: &str, value: &Option<String>) {
    if let Some(value) = clean_optional(value) {
        properties.insert(name.to_string(), json!(value));
    }
}

fn validate_cell_value(value: &Value) -> Result<(), OfficeSemanticError> {
    match value {
        Value::String(text) if text.chars().count() <= MAX_CELL_TEXT_CHARS => Ok(()),
        Value::Number(number) if number.as_f64().is_some_and(f64::is_finite) => Ok(()),
        Value::Bool(_) => Ok(()),
        Value::String(_) => Err(OfficeSemanticError::invalid(
            "Spreadsheet cell text exceeds the supported limit.",
        )),
        _ => Err(OfficeSemanticError::invalid(
            "Spreadsheet cell values must be a string, finite number, or boolean.",
        )),
    }
}

fn worksheet_root(sheet_name: &str) -> Result<String, OfficeSemanticError> {
    Ok(format!("/{}", validate_sheet_name(sheet_name)?))
}

fn worksheet_target(sheet_name: &str, reference: &str) -> Result<String, OfficeSemanticError> {
    let reference = reference.trim();
    validate_a1_reference(reference, true)?;
    Ok(format!(
        "/{}/{}",
        validate_sheet_name(sheet_name)?,
        reference
    ))
}

fn qualify_range(sheet_name: &str, range: &str) -> Result<String, OfficeSemanticError> {
    validate_a1_reference(range, true)?;
    Ok(format!(
        "{}!{}",
        validate_sheet_name(sheet_name)?,
        range.trim()
    ))
}

fn validate_sheet_name(sheet_name: &str) -> Result<&str, OfficeSemanticError> {
    let sheet_name = non_empty(sheet_name, "sheetName")?;
    if sheet_name.chars().count() > 31
        || sheet_name
            .chars()
            .any(|character| matches!(character, '[' | ']' | ':' | '*' | '?' | '/' | '\\'))
    {
        return Err(OfficeSemanticError::invalid(
            "sheetName must be at most 31 characters and cannot contain []:*?/\\.",
        ));
    }
    Ok(sheet_name)
}

fn validate_a1_reference(reference: &str, allow_range: bool) -> Result<(), OfficeSemanticError> {
    let reference = non_empty(reference, "cell or range")?;
    let parts = reference.split(':').collect::<Vec<_>>();
    if parts.len() > if allow_range { 2 } else { 1 } || parts.iter().any(|part| !is_a1_cell(part)) {
        return Err(OfficeSemanticError::invalid(format!(
            "`{reference}` is not a supported A1 {}.",
            if allow_range {
                "cell or contiguous range"
            } else {
                "cell reference"
            }
        )));
    }
    Ok(())
}

fn is_a1_cell(value: &str) -> bool {
    let value = value.trim().trim_matches('$');
    let mut characters = value.chars().peekable();
    let mut letters = 0usize;
    while characters
        .peek()
        .is_some_and(|character| character.is_ascii_alphabetic() || *character == '$')
    {
        if characters.next() != Some('$') {
            letters += 1;
        }
    }
    let mut digits = 0usize;
    let mut first_digit = None;
    for character in characters {
        if character == '$' && digits == 0 {
            continue;
        }
        if !character.is_ascii_digit() {
            return false;
        }
        first_digit.get_or_insert(character);
        digits += 1;
    }
    (1..=3).contains(&letters) && digits > 0 && first_digit != Some('0')
}

fn slide_root(slide_number: u32) -> Result<String, OfficeSemanticError> {
    require_one_based(slide_number, "slideNumber")?;
    Ok(format!("/slide[{slide_number}]"))
}

fn require_one_based(value: u32, field: &str) -> Result<(), OfficeSemanticError> {
    if value == 0 {
        Err(OfficeSemanticError::invalid(format!(
            "{field} must be one-based and greater than zero."
        )))
    } else {
        Ok(())
    }
}

fn non_empty<'a>(value: &'a str, field: &str) -> Result<&'a str, OfficeSemanticError> {
    let value = value.trim();
    if value.is_empty() {
        Err(OfficeSemanticError::invalid(format!(
            "{field} must be non-empty."
        )))
    } else {
        Ok(value)
    }
}

fn clean_optional(value: &Option<String>) -> Option<String> {
    value
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(kind: OfficeDocumentKind, intent: OfficeSemanticIntent) -> OfficeSemanticRequest {
        OfficeSemanticRequest::new(
            kind,
            Some(
                match kind {
                    OfficeDocumentKind::Document => "test.docx",
                    OfficeDocumentKind::Spreadsheet => "test.xlsx",
                    OfficeDocumentKind::Presentation => "test.pptx",
                }
                .to_string(),
            ),
            None,
            None,
            intent,
        )
    }

    #[test]
    fn document_image_compiles_to_safe_canonical_add() {
        let semantic = request(
            OfficeDocumentKind::Document,
            OfficeSemanticIntent::DocumentAddImage(OfficeImageIntent {
                source: AgentFileInputRef::Attachment {
                    read_path: "@attachments/a/image.png".to_string(),
                },
                alt_text: Some("Campus".to_string()),
                width: Some("15cm".to_string()),
                height: None,
            }),
        );
        let canonical = compile_office_semantic_request(&semantic).unwrap();
        assert_eq!(canonical.operation, OfficeOperation::Add);
        let OfficeOperationParameters::Add {
            parent,
            element_type,
            properties,
            ..
        } = canonical.typed_parameters().unwrap()
        else {
            panic!("expected add");
        };
        assert_eq!(parent, "/body");
        assert_eq!(element_type, "picture");
        assert_eq!(
            properties["src"]["resourcePath"],
            "__mycopilot_agent_input__/office/image-input-1.png"
        );
        assert_eq!(canonical.inputs.len(), 1);
        assert_eq!(properties["alt"], "Campus");
    }

    #[test]
    fn spreadsheet_formula_and_chart_compile_without_dom_input() {
        let formula = request(
            OfficeDocumentKind::Spreadsheet,
            OfficeSemanticIntent::SpreadsheetSetFormula(OfficeSpreadsheetFormulaIntent {
                sheet_name: "季度预算".to_string(),
                cell: "E2".to_string(),
                formula: "=SUM(B2:D2)".to_string(),
                number_format: Some("¥#,##0.00".to_string()),
            }),
        );
        let canonical = compile_office_semantic_request(&formula).unwrap();
        let OfficeOperationParameters::Set {
            target, properties, ..
        } = canonical.typed_parameters().unwrap()
        else {
            panic!("expected set");
        };
        assert_eq!(target, "/季度预算/E2");
        assert_eq!(properties["formula"], "SUM(B2:D2)");

        let chart = request(
            OfficeDocumentKind::Spreadsheet,
            OfficeSemanticIntent::SpreadsheetAddChart(OfficeSpreadsheetChartIntent {
                sheet_name: "季度预算".to_string(),
                chart_type: OfficeChartKind::Column,
                title: "各部门 Q1 预算".to_string(),
                data_range: "E2:E5".to_string(),
                category_range: Some("A2:A5".to_string()),
                anchor: Some("G2:N18".to_string()),
            }),
        );
        let canonical = compile_office_semantic_request(&chart).unwrap();
        let OfficeOperationParameters::Add {
            element_type,
            properties,
            ..
        } = canonical.typed_parameters().unwrap()
        else {
            panic!("expected add");
        };
        assert_eq!(element_type, "chart");
        assert_eq!(properties["dataRange"], "季度预算!E2:E5");
        assert_eq!(properties["categories"], "季度预算!A2:A5");
    }

    #[test]
    fn presentation_slide_and_image_compile_without_provider_properties() {
        let slide = request(
            OfficeDocumentKind::Presentation,
            OfficeSemanticIntent::PresentationAddSlide(OfficePresentationSlideIntent {
                layout: Some("Title and Content".to_string()),
                title: Some("核心能力".to_string()),
                body: Some("对话、文件、终端和浏览器".to_string()),
                background_color: Some("1E2A3A".to_string()),
            }),
        );
        let canonical = compile_office_semantic_request(&slide).unwrap();
        let OfficeOperationParameters::Add {
            parent,
            element_type,
            properties,
            ..
        } = canonical.typed_parameters().unwrap()
        else {
            panic!("expected add");
        };
        assert_eq!(parent, "/");
        assert_eq!(element_type, "slide");
        assert_eq!(properties["title"], "核心能力");

        let image = request(
            OfficeDocumentKind::Presentation,
            OfficeSemanticIntent::PresentationAddImage(OfficePresentationImageIntent {
                slide_number: 1,
                source: AgentFileInputRef::Workspace {
                    path: "assets/hero.png".to_string(),
                },
                x: "1in".to_string(),
                y: "1in".to_string(),
                width: "6in".to_string(),
                height: "3in".to_string(),
                alt_text: Some("Hero".to_string()),
            }),
        );
        let canonical = compile_office_semantic_request(&image).unwrap();
        let OfficeOperationParameters::Add {
            parent, properties, ..
        } = canonical.typed_parameters().unwrap()
        else {
            panic!("expected add");
        };
        assert_eq!(parent, "/slide[1]");
        assert_eq!(
            properties["src"]["resourcePath"],
            "__mycopilot_agent_input__/office/image-input-1.png"
        );
        assert_eq!(canonical.inputs.len(), 1);
        assert_eq!(
            canonical.inputs[0].source,
            AgentFileInputRef::Workspace {
                path: "assets/hero.png".to_string()
            }
        );
    }

    #[test]
    fn unsupported_table_delimiters_recommend_managed_script() {
        let semantic = request(
            OfficeDocumentKind::Document,
            OfficeSemanticIntent::DocumentAddTable(OfficeTableIntent {
                data: vec![vec!["a,b".to_string()]],
                style: None,
                width: None,
                header_fill: None,
            }),
        );
        let error = compile_office_semantic_request(&semantic).unwrap_err();
        assert_eq!(
            error.code(),
            OfficeSemanticErrorCode::CapabilityNotSupported
        );
        assert_eq!(error.recommended_route(), Some("managedScript"));
    }

    #[test]
    fn kind_mismatch_is_rejected_before_canonical_execution() {
        let semantic = request(
            OfficeDocumentKind::Document,
            OfficeSemanticIntent::SpreadsheetAddSheet(OfficeSpreadsheetSheetIntent {
                sheet_name: "Sheet2".to_string(),
            }),
        );
        let error = compile_office_semantic_request(&semantic).unwrap_err();
        assert_eq!(error.code(), OfficeSemanticErrorCode::KindMismatch);
    }
}
