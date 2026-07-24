use super::{
    schema::agent_file_input_ref_schema, AgentTool, AgentToolPermissionPolicy, FileWriteToolAccess,
    ToolExecutionContext,
};
use crate::office::{
    compile_office_semantic_request, OfficeChartKind, OfficeChartSeries,
    OfficeConditionalFormatKind, OfficeCreateIntent, OfficeDocumentBlockIntent,
    OfficeDocumentBlockKind, OfficeDocumentFormatIntent, OfficeDocumentKind,
    OfficeDocumentMoveIntent, OfficeDocumentTextIntent, OfficeEngine, OfficeEngineError,
    OfficeExecutionContext, OfficeExecutionRequest, OfficeExecutionResult,
    OfficeHeaderFooterIntent, OfficeImageIntent, OfficeInspectIntent, OfficeOperationAccess,
    OfficePresentationChartIntent, OfficePresentationFooterIntent, OfficePresentationImageIntent,
    OfficePresentationMoveSlideIntent, OfficePresentationShapeIntent,
    OfficePresentationSlideIndexIntent, OfficePresentationSlideIntent,
    OfficePresentationTableIntent, OfficePresentationTextIntent, OfficeRenderIntent,
    OfficeReplaceTextIntent, OfficeSemanticError, OfficeSemanticIntent, OfficeSemanticRequest,
    OfficeSemanticStyle, OfficeSpreadsheetCellIntent, OfficeSpreadsheetChartIntent,
    OfficeSpreadsheetConditionalFormatIntent, OfficeSpreadsheetFormulaIntent,
    OfficeSpreadsheetFreezeIntent, OfficeSpreadsheetImageIntent, OfficeSpreadsheetMoveSheetIntent,
    OfficeSpreadsheetRangeFormatIntent, OfficeSpreadsheetSheetIntent, OfficeSpreadsheetTableIntent,
    OfficeTableIntent, OfficeViewport, DEFAULT_OFFICE_TIMEOUT_MS, MAX_OFFICE_TIMEOUT_MS,
};
use crate::protocol::{
    has_unsafe_agent_office_reason_character, normalize_agent_office_reason, AgentError,
    AgentOfficeOperationRequest, AgentProposedAction, AgentResult, AgentToolCall,
    AgentToolDefinition, AgentToolSafety, AgentWritePermission,
    AGENT_OFFICE_OPERATION_SCHEMA_VERSION, AGENT_OFFICE_REASON_MAX_CHARS,
};
use crate::{file_input::AgentFileInputExecutionContext, AgentFileInputRef};
use serde::Deserialize;
use serde_json::{json, Map, Value};
use std::sync::Arc;

pub(super) struct OfficeDocumentTool(OfficeTool);
pub(super) struct OfficeSpreadsheetTool(OfficeTool);
pub(super) struct OfficePresentationTool(OfficeTool);

impl OfficeDocumentTool {
    pub(super) fn new(engine: Arc<dyn OfficeEngine>) -> Self {
        Self(OfficeTool::new(OfficeDocumentKind::Document, engine))
    }
}

impl OfficeSpreadsheetTool {
    pub(super) fn new(engine: Arc<dyn OfficeEngine>) -> Self {
        Self(OfficeTool::new(OfficeDocumentKind::Spreadsheet, engine))
    }
}

impl OfficePresentationTool {
    pub(super) fn new(engine: Arc<dyn OfficeEngine>) -> Self {
        Self(OfficeTool::new(OfficeDocumentKind::Presentation, engine))
    }
}

macro_rules! impl_office_tool {
    ($tool:ty) => {
        impl AgentTool for $tool {
            fn definition(&self) -> AgentToolDefinition {
                self.0.definition()
            }

            fn execute(&self, context: &ToolExecutionContext, args: Value) -> AgentResult<Value> {
                self.0.execute(context, args)
            }

            fn permission_policy(&self) -> AgentToolPermissionPolicy {
                AgentToolPermissionPolicy::FileWrite(FileWriteToolAccess::ReadWrite)
            }

            fn proposed_action(
                &self,
                context: &ToolExecutionContext,
                call: &AgentToolCall,
            ) -> AgentResult<AgentProposedAction> {
                self.0.proposed_action(context, call)
            }

            fn requires_approval_for_call(&self, args: &Value) -> bool {
                self.0.requires_approval_for_call(args)
            }

            fn trace_call_projection(&self, call: &AgentToolCall) -> AgentToolCall {
                self.0.trace_call_projection(call)
            }

            fn event_call_projection(&self, call: &AgentToolCall) -> AgentToolCall {
                self.0.event_call_projection(call)
            }
        }
    };
}

impl_office_tool!(OfficeDocumentTool);
impl_office_tool!(OfficeSpreadsheetTool);
impl_office_tool!(OfficePresentationTool);

struct OfficeTool {
    document_kind: OfficeDocumentKind,
    engine: Arc<dyn OfficeEngine>,
}

impl OfficeTool {
    fn new(document_kind: OfficeDocumentKind, engine: Arc<dyn OfficeEngine>) -> Self {
        Self {
            document_kind,
            engine,
        }
    }

    fn definition(&self) -> AgentToolDefinition {
        AgentToolDefinition {
            name: self.tool_name().to_string(),
            description: self.description().to_string(),
            input_schema: office_input_schema(self.document_kind),
            safety: AgentToolSafety::RequiresApproval,
            // Relative paths are checked at execution time and still require a
            // workspace. Absolute paths and supported system aliases may be
            // authorized without one when the resolved read/write policy is `all`.
            requires_workspace: false,
            requires_approval: true,
            approval_mode: crate::protocol::AgentToolApprovalMode::Dynamic,
        }
    }

    fn execute(&self, context: &ToolExecutionContext, value: Value) -> AgentResult<Value> {
        context.check_cancelled()?;
        let args = parse_args(value, self.tool_name())?;
        if args.is_status() {
            args.validate_status_call(self.tool_name())?;
            return serde_json::to_value(self.engine.status(context.cancellation_token())).map_err(
                |error| AgentError::new(format!("cannot serialize Office status: {error}")),
            );
        }

        let request = args.into_request()?;
        if request.access() == OfficeOperationAccess::FileWrite {
            return Err(AgentError::structured(
                "office.approval_required",
                "Office write operations require host approval and cannot execute directly inside the agent runtime.",
                json!({
                    "type": "office_operation",
                    "code": "approvalRequired",
                    "recovery": "requestApproval",
                    "tool": self.tool_name(),
                }),
            ));
        }

        let execution_context = office_execution_context(context)?;
        let result = self
            .engine
            .execute(
                &execution_context,
                &request,
                context.cancellation_token(),
                None,
            )
            .map_err(map_engine_error)?;
        office_execution_value(result)
    }

    fn proposed_action(
        &self,
        context: &ToolExecutionContext,
        call: &AgentToolCall,
    ) -> AgentResult<AgentProposedAction> {
        context.check_cancelled()?;
        let args = parse_args(call.args.clone(), self.tool_name())?;
        if args.is_status() {
            args.validate_status_call(self.tool_name())?;
            return Err(AgentError::new(
                "The Office status operation is read-only and does not create an approval action.",
            ));
        }
        let reason = args.reason.clone();
        let mut semantic_args = call.args.clone();
        semantic_args
            .as_object_mut()
            .expect("validated Office semantic arguments")
            .insert("reason".to_string(), Value::String(reason.clone()));
        let request = args.into_request()?;
        if request.access() != OfficeOperationAccess::FileWrite {
            return Err(AgentError::new(
                "This Office operation is read-only and does not create an approval action.",
            ));
        }
        if context.permissions().write == AgentWritePermission::Denied {
            return Err(AgentError::structured(
                "office.write_permission_denied",
                "The current permission policy does not allow Office file changes.",
                json!({
                    "type": "office_operation_policy",
                    "code": "writePermissionDenied",
                    "recovery": "changePermissions",
                    "tool": self.tool_name(),
                }),
            ));
        }

        let execution_context = office_execution_context(context)?;
        let prepared = self
            .engine
            .prepare(&execution_context, &request)
            .map_err(map_engine_error)?;
        if prepared.access != OfficeOperationAccess::FileWrite {
            return Err(AgentError::structured(
                "office.invalid_prepared_access",
                "The Office engine returned a non-writing plan for a write approval request.",
                json!({
                    "type": "office_operation",
                    "code": "invalidPreparedAccess",
                    "recovery": "retry",
                }),
            ));
        }

        Ok(AgentProposedAction::OfficeOperation {
            office_operation: Box::new(AgentOfficeOperationRequest {
                schema_version: AGENT_OFFICE_OPERATION_SCHEMA_VERSION,
                id: call.id.clone(),
                semantic_args,
                prepared,
                approval_status: call.approval_status,
                reason,
            }),
        })
    }

    fn requires_approval_for_call(&self, value: &Value) -> bool {
        let Ok(args) = parse_args(value.clone(), self.tool_name()) else {
            return true;
        };
        if args.is_status() {
            return args.validate_status_call(self.tool_name()).is_err();
        }
        let Ok(request) = args.into_request() else {
            return true;
        };
        request.access() == OfficeOperationAccess::FileWrite
    }

    fn trace_call_projection(&self, call: &AgentToolCall) -> AgentToolCall {
        let mut projection = call.clone();
        let reason = call
            .args
            .get("reason")
            .and_then(Value::as_str)
            .and_then(normalize_agent_office_reason);
        if let Some(args) = projection.args.as_object_mut() {
            match &reason {
                Some(reason) => {
                    args.insert("reason".to_string(), Value::String(reason.clone()));
                }
                None => {
                    args.remove("reason");
                }
            }
        }
        projection.reason = reason;
        projection
    }

    fn event_call_projection(&self, call: &AgentToolCall) -> AgentToolCall {
        self.trace_call_projection(call)
    }

    fn tool_name(&self) -> &'static str {
        match self.document_kind {
            OfficeDocumentKind::Document => "office_document",
            OfficeDocumentKind::Spreadsheet => "office_spreadsheet",
            OfficeDocumentKind::Presentation => "office_presentation",
        }
    }

    fn description(&self) -> &'static str {
        match self.document_kind {
            OfficeDocumentKind::Document => "Create, inspect, validate, render, and edit Word-compatible .docx documents with a flat provider-neutral semantic request. Use operations such as addText, insertImage, addTable, addHeader, addFooter, replaceText, formatText, removeBlock, and moveBlock. Always include a concise user-facing reason. A successful render returns an authoritative outputs[].source; pass it unchanged to read_image and never guess a path from argv, cwd, stdout, or a file search. Never send OfficeCLI arguments, DOM paths, or arbitrary property maps. Read-only operations run immediately; writes compile into the same frozen, permission-checked Host action. If a requested long-tail capability is unsupported, use the structured recommendedRoute=managedScript recovery.",
            OfficeDocumentKind::Spreadsheet => "Create, inspect, validate, render, and edit Excel-compatible .xlsx/.xlsm/.csv files with a flat provider-neutral semantic request. Use operations such as addSheet, writeCell, setFormula, formatRange, freezePanes, addConditionalFormat, addTable, addChart, insertImage, removeSheet, and moveSheet. Always include a concise user-facing reason. A successful render returns an authoritative outputs[].source; pass it unchanged to read_image and never guess a path from argv, cwd, stdout, or a file search. Never send OfficeCLI arguments, DOM paths, or arbitrary property maps. Read-only operations run immediately; writes compile into the same frozen, permission-checked Host action. If a requested long-tail capability is unsupported, use the structured recommendedRoute=managedScript recovery.",
            OfficeDocumentKind::Presentation => "Create, inspect, validate, render, and edit PowerPoint-compatible .pptx presentations with a flat provider-neutral semantic request. Use operations such as addSlide, addText, insertImage, addTable, addChart, addShape, addFooter, removeSlide, and moveSlide. Always include a concise user-facing reason. A successful render returns an authoritative outputs[].source; pass it unchanged to read_image and never guess a path from argv, cwd, stdout, or a file search. Never send OfficeCLI arguments, DOM paths, or arbitrary property maps. Read-only operations run immediately; writes compile into the same frozen, permission-checked Host action. If a requested long-tail capability is unsupported, use the structured recommendedRoute=managedScript recovery.",
        }
    }
}

fn office_execution_context(context: &ToolExecutionContext) -> AgentResult<OfficeExecutionContext> {
    let file_inputs = AgentFileInputExecutionContext::new(
        context.attachment_library().cloned(),
        context.skill_resources_optional(),
    )
    .with_storage(context.storage_optional());
    Ok(OfficeExecutionContext::new(
        context.workspace_root_optional()?,
        context.permissions(),
        context.attachment_library().cloned(),
    )
    .with_file_inputs(file_inputs))
}

#[derive(Debug)]
struct OfficeToolArgs {
    request: OfficeParsedRequest,
    reason: String,
}

#[derive(Debug)]
enum OfficeParsedRequest {
    Status,
    Semantic(Box<OfficeSemanticRequest>),
}

#[derive(Debug, Deserialize)]
#[serde(
    tag = "operation",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
enum OfficeSemanticOperationWire {
    Status {},
    Create {
        file_path: String,
        #[serde(default)]
        overwrite_existing: bool,
        locale: Option<String>,
        timeout_ms: Option<u64>,
    },
    Inspect {
        file_path: String,
        block_index: Option<u32>,
        sheet_name: Option<String>,
        range: Option<String>,
        slide_number: Option<u32>,
        depth: Option<u32>,
        timeout_ms: Option<u64>,
    },
    Validate {
        file_path: String,
        timeout_ms: Option<u64>,
    },
    Render {
        file_path: String,
        output_path: String,
        page_or_slide: Option<u32>,
        sheet_name: Option<String>,
        range: Option<String>,
        viewport: Option<OfficeViewport>,
        timeout_ms: Option<u64>,
    },
    #[serde(alias = "add_text")]
    AddText {
        file_path: String,
        text: String,
        block_kind: Option<OfficeDocumentBlockKind>,
        slide_number: Option<u32>,
        x: Option<String>,
        y: Option<String>,
        width: Option<String>,
        height: Option<String>,
        #[serde(default)]
        style: OfficeSemanticStyle,
        destination_path: Option<String>,
        timeout_ms: Option<u64>,
    },
    #[serde(
        rename = "insertImage",
        alias = "insert_image",
        alias = "addImage",
        alias = "add_image"
    )]
    InsertImage {
        file_path: String,
        source: AgentFileInputRef,
        alt_text: Option<String>,
        sheet_name: Option<String>,
        slide_number: Option<u32>,
        anchor: Option<String>,
        x: Option<String>,
        y: Option<String>,
        width: Option<String>,
        height: Option<String>,
        destination_path: Option<String>,
        timeout_ms: Option<u64>,
    },
    #[serde(alias = "add_table")]
    AddTable {
        file_path: String,
        #[serde(default)]
        data: Vec<Vec<String>>,
        style: Option<String>,
        width: Option<String>,
        height: Option<String>,
        header_fill: Option<String>,
        sheet_name: Option<String>,
        range: Option<String>,
        name: Option<String>,
        header_row: Option<bool>,
        total_row: Option<bool>,
        slide_number: Option<u32>,
        x: Option<String>,
        y: Option<String>,
        destination_path: Option<String>,
        timeout_ms: Option<u64>,
    },
    #[serde(alias = "add_header")]
    AddHeader {
        file_path: String,
        text: Option<String>,
        #[serde(default)]
        page_number: bool,
        #[serde(default)]
        style: OfficeSemanticStyle,
        destination_path: Option<String>,
        timeout_ms: Option<u64>,
    },
    #[serde(alias = "add_footer")]
    AddFooter {
        file_path: String,
        text: Option<String>,
        #[serde(default)]
        page_number: bool,
        slide_number: Option<u32>,
        #[serde(default)]
        style: OfficeSemanticStyle,
        destination_path: Option<String>,
        timeout_ms: Option<u64>,
    },
    #[serde(alias = "replace_text")]
    ReplaceText {
        file_path: String,
        find_text: String,
        replace_text: String,
        destination_path: Option<String>,
        timeout_ms: Option<u64>,
    },
    #[serde(alias = "format_text")]
    FormatText {
        file_path: String,
        block_index: u32,
        style: OfficeSemanticStyle,
        destination_path: Option<String>,
        timeout_ms: Option<u64>,
    },
    #[serde(alias = "remove_block")]
    RemoveBlock {
        file_path: String,
        block_index: u32,
        destination_path: Option<String>,
        timeout_ms: Option<u64>,
    },
    #[serde(alias = "move_block")]
    MoveBlock {
        file_path: String,
        block_index: u32,
        new_index: u32,
        destination_path: Option<String>,
        timeout_ms: Option<u64>,
    },
    #[serde(alias = "add_sheet")]
    AddSheet {
        file_path: String,
        sheet_name: String,
        destination_path: Option<String>,
        timeout_ms: Option<u64>,
    },
    #[serde(alias = "write_cell")]
    WriteCell {
        file_path: String,
        sheet_name: String,
        cell: String,
        value: Value,
        #[serde(default)]
        style: OfficeSemanticStyle,
        destination_path: Option<String>,
        timeout_ms: Option<u64>,
    },
    #[serde(alias = "set_formula")]
    SetFormula {
        file_path: String,
        sheet_name: String,
        cell: String,
        formula: String,
        number_format: Option<String>,
        destination_path: Option<String>,
        timeout_ms: Option<u64>,
    },
    #[serde(alias = "format_range")]
    FormatRange {
        file_path: String,
        sheet_name: String,
        range: String,
        style: OfficeSemanticStyle,
        destination_path: Option<String>,
        timeout_ms: Option<u64>,
    },
    #[serde(alias = "freeze_panes")]
    FreezePanes {
        file_path: String,
        sheet_name: String,
        freeze_at: String,
        destination_path: Option<String>,
        timeout_ms: Option<u64>,
    },
    #[serde(alias = "add_conditional_format")]
    AddConditionalFormat {
        file_path: String,
        sheet_name: String,
        range: String,
        kind: OfficeConditionalFormatKind,
        operator: Option<String>,
        value: Option<Value>,
        second_value: Option<Value>,
        text: Option<String>,
        fill_color: Option<String>,
        min_color: Option<String>,
        mid_color: Option<String>,
        max_color: Option<String>,
        destination_path: Option<String>,
        timeout_ms: Option<u64>,
    },
    #[serde(alias = "add_chart")]
    AddChart {
        file_path: String,
        chart_type: OfficeChartKind,
        title: String,
        sheet_name: Option<String>,
        data_range: Option<String>,
        category_range: Option<String>,
        anchor: Option<String>,
        slide_number: Option<u32>,
        #[serde(default)]
        series: Vec<OfficeChartSeries>,
        #[serde(default)]
        categories: Vec<String>,
        x: Option<String>,
        y: Option<String>,
        width: Option<String>,
        height: Option<String>,
        destination_path: Option<String>,
        timeout_ms: Option<u64>,
    },
    #[serde(alias = "remove_sheet")]
    RemoveSheet {
        file_path: String,
        sheet_name: String,
        destination_path: Option<String>,
        timeout_ms: Option<u64>,
    },
    #[serde(alias = "move_sheet")]
    MoveSheet {
        file_path: String,
        sheet_name: String,
        new_index: u32,
        destination_path: Option<String>,
        timeout_ms: Option<u64>,
    },
    #[serde(alias = "add_slide")]
    AddSlide {
        file_path: String,
        layout: Option<String>,
        title: Option<String>,
        body: Option<String>,
        background_color: Option<String>,
        destination_path: Option<String>,
        timeout_ms: Option<u64>,
    },
    #[serde(alias = "add_shape")]
    AddShape {
        file_path: String,
        slide_number: u32,
        shape_type: String,
        text: Option<String>,
        x: String,
        y: String,
        width: String,
        height: String,
        #[serde(default)]
        style: OfficeSemanticStyle,
        line_color: Option<String>,
        destination_path: Option<String>,
        timeout_ms: Option<u64>,
    },
    #[serde(alias = "remove_slide")]
    RemoveSlide {
        file_path: String,
        slide_number: u32,
        destination_path: Option<String>,
        timeout_ms: Option<u64>,
    },
    #[serde(alias = "move_slide")]
    MoveSlide {
        file_path: String,
        slide_number: u32,
        new_index: u32,
        destination_path: Option<String>,
        timeout_ms: Option<u64>,
    },
}

impl OfficeSemanticOperationWire {
    fn into_parsed(
        self,
        document_kind: OfficeDocumentKind,
    ) -> Result<OfficeParsedRequest, OfficeSemanticError> {
        let (file_path, destination_path, timeout_ms, intent) = match self {
            Self::Status {} => return Ok(OfficeParsedRequest::Status),
            Self::Create {
                file_path,
                overwrite_existing,
                locale,
                timeout_ms,
            } => (
                file_path,
                None,
                timeout_ms,
                OfficeSemanticIntent::Create(OfficeCreateIntent {
                    overwrite_existing,
                    locale,
                }),
            ),
            Self::Inspect {
                file_path,
                block_index,
                sheet_name,
                range,
                slide_number,
                depth,
                timeout_ms,
            } => (
                file_path,
                None,
                timeout_ms,
                OfficeSemanticIntent::Inspect(OfficeInspectIntent {
                    block_index,
                    sheet_name,
                    range,
                    slide_number,
                    depth,
                }),
            ),
            Self::Validate {
                file_path,
                timeout_ms,
            } => (file_path, None, timeout_ms, OfficeSemanticIntent::Validate),
            Self::Render {
                file_path,
                output_path,
                page_or_slide,
                sheet_name,
                range,
                viewport,
                timeout_ms,
            } => (
                file_path,
                None,
                timeout_ms,
                OfficeSemanticIntent::Render(OfficeRenderIntent {
                    output_path,
                    page_or_slide,
                    sheet_name,
                    range,
                    viewport,
                }),
            ),
            Self::AddText {
                file_path,
                text,
                block_kind,
                slide_number,
                x,
                y,
                width,
                height,
                style,
                destination_path,
                timeout_ms,
            } => {
                let intent = match document_kind {
                    OfficeDocumentKind::Document => {
                        reject_semantic_extras(
                            [
                                ("slideNumber", slide_number.is_some()),
                                ("x", x.is_some()),
                                ("y", y.is_some()),
                                ("width", width.is_some()),
                                ("height", height.is_some()),
                            ],
                            "addText",
                        )?;
                        OfficeSemanticIntent::DocumentAddText(OfficeDocumentTextIntent {
                            block_kind: block_kind.unwrap_or(OfficeDocumentBlockKind::Paragraph),
                            text,
                            style,
                        })
                    }
                    OfficeDocumentKind::Presentation => {
                        reject_semantic_extras(
                            [("blockKind", block_kind.is_some())],
                            "addText",
                        )?;
                        OfficeSemanticIntent::PresentationAddText(OfficePresentationTextIntent {
                            slide_number: required_wire(slide_number, "slideNumber", "addText")?,
                            text,
                            x: required_wire(x, "x", "addText")?,
                            y: required_wire(y, "y", "addText")?,
                            width: required_wire(width, "width", "addText")?,
                            height: required_wire(height, "height", "addText")?,
                            style,
                        })
                    }
                    OfficeDocumentKind::Spreadsheet => {
                        return Err(OfficeSemanticError::unsupported(
                            "spreadsheet.addText",
                            "Spreadsheets use writeCell for text values; use managedScript for rich drawing text.",
                        ))
                    }
                };
                (file_path, destination_path, timeout_ms, intent)
            }
            Self::InsertImage {
                file_path,
                source,
                alt_text,
                sheet_name,
                slide_number,
                anchor,
                x,
                y,
                width,
                height,
                destination_path,
                timeout_ms,
            } => {
                let intent = match document_kind {
                    OfficeDocumentKind::Document => {
                        reject_semantic_extras(
                            [
                                ("sheetName", sheet_name.is_some()),
                                ("slideNumber", slide_number.is_some()),
                                ("anchor", anchor.is_some()),
                                ("x", x.is_some()),
                                ("y", y.is_some()),
                            ],
                            "insertImage",
                        )?;
                        OfficeSemanticIntent::DocumentAddImage(OfficeImageIntent {
                            source,
                            alt_text,
                            width,
                            height,
                        })
                    }
                    OfficeDocumentKind::Spreadsheet => {
                        reject_semantic_extras(
                            [
                                ("slideNumber", slide_number.is_some()),
                                ("x", x.is_some()),
                                ("y", y.is_some()),
                            ],
                            "insertImage",
                        )?;
                        OfficeSemanticIntent::SpreadsheetAddImage(OfficeSpreadsheetImageIntent {
                            sheet_name: required_wire(sheet_name, "sheetName", "insertImage")?,
                            source,
                            alt_text,
                            anchor,
                            width,
                            height,
                        })
                    }
                    OfficeDocumentKind::Presentation => {
                        reject_semantic_extras(
                            [
                                ("sheetName", sheet_name.is_some()),
                                ("anchor", anchor.is_some()),
                            ],
                            "insertImage",
                        )?;
                        OfficeSemanticIntent::PresentationAddImage(OfficePresentationImageIntent {
                            slide_number: required_wire(
                                slide_number,
                                "slideNumber",
                                "insertImage",
                            )?,
                            source,
                            x: required_wire(x, "x", "insertImage")?,
                            y: required_wire(y, "y", "insertImage")?,
                            width: required_wire(width, "width", "insertImage")?,
                            height: required_wire(height, "height", "insertImage")?,
                            alt_text,
                        })
                    }
                };
                (file_path, destination_path, timeout_ms, intent)
            }
            Self::AddTable {
                file_path,
                data,
                style,
                width,
                height,
                header_fill,
                sheet_name,
                range,
                name,
                header_row,
                total_row,
                slide_number,
                x,
                y,
                destination_path,
                timeout_ms,
            } => {
                let intent = match document_kind {
                    OfficeDocumentKind::Document => {
                        reject_semantic_extras(
                            [
                                ("height", height.is_some()),
                                ("sheetName", sheet_name.is_some()),
                                ("range", range.is_some()),
                                ("name", name.is_some()),
                                ("headerRow", header_row.is_some()),
                                ("totalRow", total_row.is_some()),
                                ("slideNumber", slide_number.is_some()),
                                ("x", x.is_some()),
                                ("y", y.is_some()),
                            ],
                            "addTable",
                        )?;
                        OfficeSemanticIntent::DocumentAddTable(OfficeTableIntent {
                            data,
                            style,
                            width,
                            header_fill,
                        })
                    }
                    OfficeDocumentKind::Spreadsheet => {
                        reject_semantic_extras(
                            [
                                ("data", !data.is_empty()),
                                ("width", width.is_some()),
                                ("height", height.is_some()),
                                ("headerFill", header_fill.is_some()),
                                ("slideNumber", slide_number.is_some()),
                                ("x", x.is_some()),
                                ("y", y.is_some()),
                            ],
                            "addTable",
                        )?;
                        OfficeSemanticIntent::SpreadsheetAddTable(OfficeSpreadsheetTableIntent {
                            sheet_name: required_wire(sheet_name, "sheetName", "addTable")?,
                            range: required_wire(range, "range", "addTable")?,
                            name,
                            style,
                            header_row: header_row.unwrap_or(true),
                            total_row: total_row.unwrap_or(false),
                        })
                    }
                    OfficeDocumentKind::Presentation => {
                        reject_semantic_extras(
                            [
                                ("sheetName", sheet_name.is_some()),
                                ("range", range.is_some()),
                                ("name", name.is_some()),
                                ("headerRow", header_row.is_some()),
                                ("totalRow", total_row.is_some()),
                            ],
                            "addTable",
                        )?;
                        OfficeSemanticIntent::PresentationAddTable(OfficePresentationTableIntent {
                            slide_number: required_wire(slide_number, "slideNumber", "addTable")?,
                            data,
                            x: required_wire(x, "x", "addTable")?,
                            y: required_wire(y, "y", "addTable")?,
                            width: required_wire(width, "width", "addTable")?,
                            height: required_wire(height, "height", "addTable")?,
                            style,
                            header_fill,
                        })
                    }
                };
                (file_path, destination_path, timeout_ms, intent)
            }
            Self::AddHeader {
                file_path,
                text,
                page_number,
                style,
                destination_path,
                timeout_ms,
            } => (
                file_path,
                destination_path,
                timeout_ms,
                OfficeSemanticIntent::DocumentAddHeader(OfficeHeaderFooterIntent {
                    text,
                    page_number,
                    style,
                }),
            ),
            Self::AddFooter {
                file_path,
                text,
                page_number,
                slide_number,
                style,
                destination_path,
                timeout_ms,
            } => {
                let intent = match document_kind {
                    OfficeDocumentKind::Document => {
                        reject_semantic_extras(
                            [("slideNumber", slide_number.is_some())],
                            "addFooter",
                        )?;
                        OfficeSemanticIntent::DocumentAddFooter(OfficeHeaderFooterIntent {
                            text,
                            page_number,
                            style,
                        })
                    }
                    OfficeDocumentKind::Presentation => {
                        if page_number {
                            return Err(OfficeSemanticError::unsupported(
                                "presentation.footerPageNumber",
                                "Native presentation footer page-number fields are not in the semantic surface; use managedScript.",
                            ));
                        }
                        OfficeSemanticIntent::PresentationAddFooter(
                            OfficePresentationFooterIntent {
                                slide_number: required_wire(
                                    slide_number,
                                    "slideNumber",
                                    "addFooter",
                                )?,
                                text: required_wire(text, "text", "addFooter")?,
                                style,
                            },
                        )
                    }
                    OfficeDocumentKind::Spreadsheet => {
                        return Err(OfficeSemanticError::unsupported(
                            "spreadsheet.addFooter",
                            "Spreadsheet print footer composition is outside the native semantic surface; use managedScript.",
                        ))
                    }
                };
                (file_path, destination_path, timeout_ms, intent)
            }
            Self::ReplaceText {
                file_path,
                find_text,
                replace_text,
                destination_path,
                timeout_ms,
            } => (
                file_path,
                destination_path,
                timeout_ms,
                OfficeSemanticIntent::DocumentReplaceText(OfficeReplaceTextIntent {
                    find: find_text,
                    replace: replace_text,
                }),
            ),
            Self::FormatText {
                file_path,
                block_index,
                style,
                destination_path,
                timeout_ms,
            } => (
                file_path,
                destination_path,
                timeout_ms,
                OfficeSemanticIntent::DocumentFormatText(OfficeDocumentFormatIntent {
                    block_index,
                    style,
                }),
            ),
            Self::RemoveBlock {
                file_path,
                block_index,
                destination_path,
                timeout_ms,
            } => (
                file_path,
                destination_path,
                timeout_ms,
                OfficeSemanticIntent::DocumentRemoveBlock(OfficeDocumentBlockIntent {
                    block_index,
                }),
            ),
            Self::MoveBlock {
                file_path,
                block_index,
                new_index,
                destination_path,
                timeout_ms,
            } => (
                file_path,
                destination_path,
                timeout_ms,
                OfficeSemanticIntent::DocumentMoveBlock(OfficeDocumentMoveIntent {
                    block_index,
                    new_index,
                }),
            ),
            Self::AddSheet {
                file_path,
                sheet_name,
                destination_path,
                timeout_ms,
            } => (
                file_path,
                destination_path,
                timeout_ms,
                OfficeSemanticIntent::SpreadsheetAddSheet(OfficeSpreadsheetSheetIntent {
                    sheet_name,
                }),
            ),
            Self::WriteCell {
                file_path,
                sheet_name,
                cell,
                value,
                style,
                destination_path,
                timeout_ms,
            } => (
                file_path,
                destination_path,
                timeout_ms,
                OfficeSemanticIntent::SpreadsheetWriteCell(OfficeSpreadsheetCellIntent {
                    sheet_name,
                    cell,
                    value,
                    style,
                }),
            ),
            Self::SetFormula {
                file_path,
                sheet_name,
                cell,
                formula,
                number_format,
                destination_path,
                timeout_ms,
            } => (
                file_path,
                destination_path,
                timeout_ms,
                OfficeSemanticIntent::SpreadsheetSetFormula(OfficeSpreadsheetFormulaIntent {
                    sheet_name,
                    cell,
                    formula,
                    number_format,
                }),
            ),
            Self::FormatRange {
                file_path,
                sheet_name,
                range,
                style,
                destination_path,
                timeout_ms,
            } => (
                file_path,
                destination_path,
                timeout_ms,
                OfficeSemanticIntent::SpreadsheetFormatRange(OfficeSpreadsheetRangeFormatIntent {
                    sheet_name,
                    range,
                    style,
                }),
            ),
            Self::FreezePanes {
                file_path,
                sheet_name,
                freeze_at,
                destination_path,
                timeout_ms,
            } => (
                file_path,
                destination_path,
                timeout_ms,
                OfficeSemanticIntent::SpreadsheetFreezePanes(OfficeSpreadsheetFreezeIntent {
                    sheet_name,
                    freeze_at,
                }),
            ),
            Self::AddConditionalFormat {
                file_path,
                sheet_name,
                range,
                kind,
                operator,
                value,
                second_value,
                text,
                fill_color,
                min_color,
                mid_color,
                max_color,
                destination_path,
                timeout_ms,
            } => (
                file_path,
                destination_path,
                timeout_ms,
                OfficeSemanticIntent::SpreadsheetAddConditionalFormat(
                    OfficeSpreadsheetConditionalFormatIntent {
                        sheet_name,
                        range,
                        kind,
                        operator,
                        value,
                        second_value,
                        text,
                        fill_color,
                        min_color,
                        mid_color,
                        max_color,
                    },
                ),
            ),
            Self::AddChart {
                file_path,
                chart_type,
                title,
                sheet_name,
                data_range,
                category_range,
                anchor,
                slide_number,
                series,
                categories,
                x,
                y,
                width,
                height,
                destination_path,
                timeout_ms,
            } => {
                let intent = match document_kind {
                    OfficeDocumentKind::Spreadsheet => {
                        reject_semantic_extras(
                            [
                                ("slideNumber", slide_number.is_some()),
                                ("series", !series.is_empty()),
                                ("categories", !categories.is_empty()),
                                ("x", x.is_some()),
                                ("y", y.is_some()),
                                ("width", width.is_some()),
                                ("height", height.is_some()),
                            ],
                            "addChart",
                        )?;
                        OfficeSemanticIntent::SpreadsheetAddChart(
                            OfficeSpreadsheetChartIntent {
                                sheet_name: required_wire(
                                    sheet_name,
                                    "sheetName",
                                    "addChart",
                                )?,
                                chart_type,
                                title,
                                data_range: required_wire(
                                    data_range,
                                    "dataRange",
                                    "addChart",
                                )?,
                                category_range,
                                anchor,
                            },
                        )
                    }
                    OfficeDocumentKind::Presentation => {
                        reject_semantic_extras(
                            [
                                ("sheetName", sheet_name.is_some()),
                                ("dataRange", data_range.is_some()),
                                ("categoryRange", category_range.is_some()),
                                ("anchor", anchor.is_some()),
                            ],
                            "addChart",
                        )?;
                        OfficeSemanticIntent::PresentationAddChart(
                            OfficePresentationChartIntent {
                                slide_number: required_wire(
                                    slide_number,
                                    "slideNumber",
                                    "addChart",
                                )?,
                                chart_type,
                                title,
                                series,
                                categories,
                                x: required_wire(x, "x", "addChart")?,
                                y: required_wire(y, "y", "addChart")?,
                                width: required_wire(width, "width", "addChart")?,
                                height: required_wire(height, "height", "addChart")?,
                            },
                        )
                    }
                    OfficeDocumentKind::Document => {
                        return Err(OfficeSemanticError::unsupported(
                            "document.addChart",
                            "Native Word chart creation is outside the semantic surface; use managedScript.",
                        ))
                    }
                };
                (file_path, destination_path, timeout_ms, intent)
            }
            Self::RemoveSheet {
                file_path,
                sheet_name,
                destination_path,
                timeout_ms,
            } => (
                file_path,
                destination_path,
                timeout_ms,
                OfficeSemanticIntent::SpreadsheetRemoveSheet(OfficeSpreadsheetSheetIntent {
                    sheet_name,
                }),
            ),
            Self::MoveSheet {
                file_path,
                sheet_name,
                new_index,
                destination_path,
                timeout_ms,
            } => (
                file_path,
                destination_path,
                timeout_ms,
                OfficeSemanticIntent::SpreadsheetMoveSheet(OfficeSpreadsheetMoveSheetIntent {
                    sheet_name,
                    new_index,
                }),
            ),
            Self::AddSlide {
                file_path,
                layout,
                title,
                body,
                background_color,
                destination_path,
                timeout_ms,
            } => (
                file_path,
                destination_path,
                timeout_ms,
                OfficeSemanticIntent::PresentationAddSlide(OfficePresentationSlideIntent {
                    layout,
                    title,
                    body,
                    background_color,
                }),
            ),
            Self::AddShape {
                file_path,
                slide_number,
                shape_type,
                text,
                x,
                y,
                width,
                height,
                style,
                line_color,
                destination_path,
                timeout_ms,
            } => (
                file_path,
                destination_path,
                timeout_ms,
                OfficeSemanticIntent::PresentationAddShape(OfficePresentationShapeIntent {
                    slide_number,
                    shape_type,
                    text,
                    x,
                    y,
                    width,
                    height,
                    style,
                    line_color,
                }),
            ),
            Self::RemoveSlide {
                file_path,
                slide_number,
                destination_path,
                timeout_ms,
            } => (
                file_path,
                destination_path,
                timeout_ms,
                OfficeSemanticIntent::PresentationRemoveSlide(OfficePresentationSlideIndexIntent {
                    slide_number,
                }),
            ),
            Self::MoveSlide {
                file_path,
                slide_number,
                new_index,
                destination_path,
                timeout_ms,
            } => (
                file_path,
                destination_path,
                timeout_ms,
                OfficeSemanticIntent::PresentationMoveSlide(OfficePresentationMoveSlideIntent {
                    slide_number,
                    new_index,
                }),
            ),
        };
        Ok(OfficeParsedRequest::Semantic(Box::new(
            OfficeSemanticRequest::new(
                document_kind,
                Some(file_path),
                destination_path,
                timeout_ms,
                intent,
            ),
        )))
    }
}

fn required_wire<T>(
    value: Option<T>,
    field: &str,
    operation: &str,
) -> Result<T, OfficeSemanticError> {
    value.ok_or_else(|| {
        OfficeSemanticError::invalid(format!(
            "{operation} requires the `{field}` field for this Office file type."
        ))
    })
}

fn reject_semantic_extras<const N: usize>(
    fields: [(&str, bool); N],
    operation: &str,
) -> Result<(), OfficeSemanticError> {
    let extras = fields
        .into_iter()
        .filter_map(|(name, present)| present.then_some(name))
        .collect::<Vec<_>>();
    if extras.is_empty() {
        Ok(())
    } else {
        Err(OfficeSemanticError::invalid(format!(
            "{operation} does not accept {} for this Office file type.",
            extras.join(", ")
        )))
    }
}

impl OfficeToolArgs {
    fn is_status(&self) -> bool {
        matches!(self.request, OfficeParsedRequest::Status)
    }

    fn validate_status_call(&self, _tool_name: &str) -> AgentResult<()> {
        Ok(())
    }

    fn into_request(self) -> AgentResult<OfficeExecutionRequest> {
        match self.request {
            OfficeParsedRequest::Status => Err(AgentError::structured(
                "office.semantic_invalid_request",
                "The Office status operation does not create an execution request.",
                json!({
                    "type": "office_semantic_request",
                    "code": "semanticInvalidRequest",
                    "recovery": "changeRequest",
                }),
            )),
            OfficeParsedRequest::Semantic(request) => {
                compile_office_semantic_request(&request).map_err(map_semantic_error)
            }
        }
    }
}

fn parse_args(value: Value, tool_name: &str) -> AgentResult<OfficeToolArgs> {
    let object = value
        .as_object()
        .ok_or_else(|| AgentError::new(format!("{tool_name} parameters must be a JSON object.")))?;
    let raw_reason = match object.get("reason") {
        Some(Value::String(reason)) => reason.clone(),
        Some(_) => {
            return Err(AgentError::new(format!(
                "{tool_name}.reason must be a string."
            )))
        }
        None => String::new(),
    };
    if has_unsafe_agent_office_reason_character(&raw_reason) {
        return Err(AgentError::structured(
            "office.reason_unsafe",
            format!(
                "{tool_name}.reason must be a single line without control or bidirectional-control characters."
            ),
            json!({
                "type": "office_operation",
                "code": "reasonUnsafe",
                "recovery": "changeRequest",
            }),
        ));
    }
    let reason = raw_reason.trim();
    if reason.is_empty() {
        return Err(AgentError::structured(
            "office.reason_required",
            format!("{tool_name}.reason must be a non-empty user-facing description."),
            json!({
                "type": "office_operation",
                "code": "reasonRequired",
                "maxLength": AGENT_OFFICE_REASON_MAX_CHARS,
            }),
        ));
    }
    if reason.chars().count() > AGENT_OFFICE_REASON_MAX_CHARS {
        return Err(AgentError::structured(
            "office.reason_too_long",
            format!("{tool_name}.reason cannot exceed {AGENT_OFFICE_REASON_MAX_CHARS} characters."),
            json!({
                "type": "office_operation",
                "code": "reasonTooLong",
                "maxLength": AGENT_OFFICE_REASON_MAX_CHARS,
            }),
        ));
    }
    let reason = reason.to_string();
    let document_kind = document_kind_for_tool_name(tool_name)?;
    let mut semantic_value = value;
    semantic_value
        .as_object_mut()
        .expect("validated object")
        .remove("reason");
    let parsed: OfficeSemanticOperationWire =
        serde_json::from_value(semantic_value).map_err(|error| {
            AgentError::structured(
                "office.semantic_invalid_request",
                format!("{tool_name} semantic parameters are invalid: {error}"),
                json!({
                    "type": "office_semantic_request",
                    "code": "semanticInvalidRequest",
                    "recovery": "changeRequest",
                    "canonicalOperations": semantic_operation_names(document_kind),
                }),
            )
        })?;
    let request = parsed
        .into_parsed(document_kind)
        .map_err(map_semantic_error)?;
    Ok(OfficeToolArgs { request, reason })
}

fn document_kind_for_tool_name(tool_name: &str) -> AgentResult<OfficeDocumentKind> {
    match tool_name {
        "office_document" => Ok(OfficeDocumentKind::Document),
        "office_spreadsheet" => Ok(OfficeDocumentKind::Spreadsheet),
        "office_presentation" => Ok(OfficeDocumentKind::Presentation),
        _ => Err(AgentError::new(format!(
            "Unknown managed Office tool `{tool_name}`."
        ))),
    }
}

fn map_semantic_error(error: OfficeSemanticError) -> AgentError {
    let recovery = error
        .recommended_route()
        .map_or("changeRequest", |_| "useManagedScript");
    AgentError::structured(
        error.code().stable_name(),
        error.message(),
        json!({
            "type": "office_semantic_request",
            "code": match error.code() {
                crate::office::OfficeSemanticErrorCode::InvalidRequest => "semanticInvalidRequest",
                crate::office::OfficeSemanticErrorCode::KindMismatch => "semanticKindMismatch",
                crate::office::OfficeSemanticErrorCode::CapabilityNotSupported => "capabilityNotSupported",
            },
            "recovery": recovery,
            "recommendedRoute": error.recommended_route(),
            "capability": error.capability(),
        }),
    )
}

pub(crate) fn validate_frozen_office_trace_args(
    frozen: &AgentOfficeOperationRequest,
    operation: &Value,
) -> Result<(), String> {
    if operation != &frozen.semantic_args {
        return Err("Office ToolCall differs from the frozen semantic request".to_string());
    }
    let document_kind = frozen.prepared.request.document_kind;
    let tool_name = match document_kind {
        OfficeDocumentKind::Document => "office_document",
        OfficeDocumentKind::Spreadsheet => "office_spreadsheet",
        OfficeDocumentKind::Presentation => "office_presentation",
    };
    let args = parse_args(operation.clone(), tool_name)
        .map_err(|_| format!("{tool_name} frozen ToolCall arguments are invalid"))?;
    let reason = args.reason.clone();
    let request = args
        .into_request()
        .map_err(|_| format!("{tool_name} frozen ToolCall request is invalid"))?;
    if request != frozen.prepared.request || reason != frozen.reason {
        return Err(format!(
            "{tool_name} ToolCall differs from the frozen request"
        ));
    }
    Ok(())
}

fn office_execution_value(result: OfficeExecutionResult) -> AgentResult<Value> {
    let succeeded = result.exit_code == Some(0)
        && !result.timed_out
        && !result.cancelled
        && result.error_code.is_none();
    if succeeded {
        return serde_json::to_value(result).map_err(|error| {
            AgentError::new(format!("cannot serialize Office execution result: {error}"))
        });
    }

    let code = result
        .error_code
        .clone()
        .unwrap_or_else(|| "office.process_failure".to_string());
    let message = result
        .error
        .clone()
        .unwrap_or_else(|| format!("OfficeCLI exited with code {:?}.", result.exit_code));
    let execution = serde_json::to_value(&result).unwrap_or_else(|_| {
        json!({
            "exitCode": result.exit_code,
            "stdout": result.stdout,
            "stderr": result.stderr,
        })
    });
    Err(AgentError::structured(
        code.clone(),
        message,
        json!({
            "type": "office_execution",
            "code": code,
            "recovery": if result.cancelled { "retry" } else { "changeRequestOrInspectOutput" },
            "execution": execution,
        }),
    ))
}

fn map_engine_error(error: OfficeEngineError) -> AgentError {
    AgentError::structured(
        error.code().stable_name(),
        error.message(),
        json!({
            "type": "office_engine",
            "code": error.code().stable_name(),
            "recovery": error.recovery().stable_name(),
        }),
    )
}

fn office_input_schema(document_kind: OfficeDocumentKind) -> Value {
    let mut properties = schema_properties([
        (
            "operation",
            json!({
                "type": "string",
                "enum": semantic_operation_names(document_kind),
                "description": "Select one semantic Office intent. snake_case aliases are accepted at the trusted parser boundary, but use the canonical camelCase name.",
            }),
        ),
        ("reason", reason_schema()),
        ("filePath", file_path_schema(document_kind)),
        ("destinationPath", destination_path_schema(document_kind)),
        ("timeoutMs", timeout_schema()),
        (
            "outputPath",
            json!({
                "type": "string",
                "minLength": 1,
                "description": "Required only for render. The Host owns the render flag, stages and atomically publishes the output, then returns its authoritative model-readable reference in outputs[].source.",
            }),
        ),
        (
            "pageOrSlide",
            positive_integer_schema("Optional one-based page or slide rendered by render."),
        ),
        (
            "depth",
            json!({
                "type": "integer",
                "minimum": 0,
                "maximum": 32,
                "description": "Optional inspection depth.",
            }),
        ),
    ]);

    properties.insert(
        "overwriteExisting".to_string(),
        json!({
            "type": "boolean",
            "description": "create only. Replacing an existing target still uses approval, conflict checks, staging, and atomic publication.",
        }),
    );
    properties.insert(
        "locale".to_string(),
        json!({
            "type": "string",
            "minLength": 1,
            "description": "Document create only. Optional BCP-47 locale such as zh-CN.",
        }),
    );
    properties.insert("style".to_string(), semantic_style_schema());
    properties.insert("source".to_string(), agent_file_input_ref_schema());
    properties.insert(
        "altText".to_string(),
        json!({
            "type": "string",
            "description": "Accessible description for insertImage.",
        }),
    );
    for (name, description) in [
        ("x", "Presentation element x position, for example 1in."),
        ("y", "Presentation element y position, for example 1in."),
        ("width", "Element width, for example 15cm or 6in."),
        ("height", "Element height, for example 3in."),
        (
            "anchor",
            "Spreadsheet chart or image anchor such as G2:N18.",
        ),
    ] {
        properties.insert(
            name.to_string(),
            json!({ "type": "string", "minLength": 1, "description": description }),
        );
    }

    match document_kind {
        OfficeDocumentKind::Document => {
            properties.extend(schema_properties([
                (
                    "blockKind",
                    json!({
                        "type": "string",
                        "enum": ["paragraph", "heading1", "heading2", "heading3", "bullet", "numbered"],
                        "description": "addText block kind; defaults to paragraph.",
                    }),
                ),
                (
                    "text",
                    json!({
                        "type": "string",
                        "description": "Text for addText, addHeader, or addFooter.",
                    }),
                ),
                ("data", table_data_schema()),
                (
                    "headerFill",
                    json!({
                        "type": "string",
                        "description": "Optional table header fill color.",
                    }),
                ),
                (
                    "pageNumber",
                    json!({
                        "type": "boolean",
                        "description": "addHeader/addFooter: include the PAGE field.",
                    }),
                ),
                (
                    "findText",
                    json!({
                        "type": "string",
                        "minLength": 1,
                        "description": "replaceText literal search text.",
                    }),
                ),
                (
                    "replaceText",
                    json!({
                        "type": "string",
                        "description": "replaceText replacement.",
                    }),
                ),
                (
                    "blockIndex",
                    positive_integer_schema(
                        "One-based body block for inspect, formatText, removeBlock, or moveBlock.",
                    ),
                ),
                (
                    "newIndex",
                    positive_integer_schema("One-based destination for moveBlock."),
                ),
            ]));
        }
        OfficeDocumentKind::Spreadsheet => {
            properties.extend(spreadsheet_semantic_schema_properties());
        }
        OfficeDocumentKind::Presentation => {
            properties.extend(presentation_semantic_schema_properties());
        }
    }

    json!({
        "type": "object",
        "description": format!(
            "Versioned provider-neutral semantic {:?} request. Keep operation, filePath, operation fields, and reason at this single flat root. The Host validates conditional requirements, compiles the intent to a canonical frozen Office action, and never accepts OfficeCLI argv, DOM paths, or provider property maps. Unsupported long-tail capabilities return capabilityNotSupported with recommendedRoute=managedScript.",
            document_kind
        ),
        "properties": properties,
        "required": ["operation", "reason"],
        "additionalProperties": false,
    })
}

fn semantic_operation_names(document_kind: OfficeDocumentKind) -> Vec<&'static str> {
    let mut operations = vec!["status", "create", "inspect", "validate", "render"];
    operations.extend(match document_kind {
        OfficeDocumentKind::Document => vec![
            "addText",
            "insertImage",
            "addTable",
            "addHeader",
            "addFooter",
            "replaceText",
            "formatText",
            "removeBlock",
            "moveBlock",
        ],
        OfficeDocumentKind::Spreadsheet => vec![
            "addSheet",
            "writeCell",
            "setFormula",
            "formatRange",
            "freezePanes",
            "addConditionalFormat",
            "addTable",
            "addChart",
            "insertImage",
            "removeSheet",
            "moveSheet",
        ],
        OfficeDocumentKind::Presentation => vec![
            "addSlide",
            "addText",
            "insertImage",
            "addTable",
            "addChart",
            "addShape",
            "addFooter",
            "removeSlide",
            "moveSlide",
        ],
    });
    operations
}

fn semantic_style_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "fontName": { "type": "string", "minLength": 1 },
            "fontSize": { "type": "number", "minimum": 1, "maximum": 400 },
            "fontColor": { "type": "string", "minLength": 1 },
            "fillColor": { "type": "string", "minLength": 1 },
            "bold": { "type": "boolean" },
            "italic": { "type": "boolean" },
            "alignment": {
                "type": "string",
                "enum": ["left", "center", "right", "justify"]
            },
            "wrapText": { "type": "boolean" },
            "numberFormat": { "type": "string", "minLength": 1 },
            "lineSpacing": { "type": "number", "minimum": 0.5, "maximum": 10 }
        },
        "additionalProperties": false,
        "description": "Small cross-format style vocabulary. The backend rejects fields that do not apply to the selected file kind.",
    })
}

fn table_data_schema() -> Value {
    json!({
        "type": "array",
        "minItems": 1,
        "maxItems": 2000,
        "items": {
            "type": "array",
            "minItems": 1,
            "maxItems": 256,
            "items": { "type": "string", "maxLength": 32768 }
        },
        "description": "Rectangular table rows. Cells containing commas, semicolons, or line breaks require managedScript.",
    })
}

fn spreadsheet_semantic_schema_properties() -> Map<String, Value> {
    schema_properties([
        (
            "sheetName",
            json!({
                "type": "string",
                "minLength": 1,
                "maxLength": 31,
                "description": "Worksheet name used by spreadsheet semantic operations.",
            }),
        ),
        (
            "cell",
            json!({
                "type": "string",
                "minLength": 2,
                "description": "A1 cell reference for writeCell or setFormula.",
            }),
        ),
        (
            "range",
            json!({
                "type": "string",
                "minLength": 2,
                "description": "A1 cell or contiguous range for inspect, render, formatRange, addConditionalFormat, or addTable.",
            }),
        ),
        (
            "value",
            json!({
                "oneOf": [
                    { "type": "string" },
                    { "type": "number" },
                    { "type": "boolean" }
                ],
                "description": "writeCell value or a conditional-format threshold.",
            }),
        ),
        (
            "secondValue",
            json!({
                "oneOf": [
                    { "type": "string" },
                    { "type": "number" },
                    { "type": "boolean" }
                ],
                "description": "Optional second conditional-format threshold.",
            }),
        ),
        (
            "formula",
            json!({
                "type": "string",
                "minLength": 1,
                "description": "setFormula expression. Leading '=' is optional.",
            }),
        ),
        (
            "numberFormat",
            json!({
                "type": "string",
                "minLength": 1,
                "description": "Optional setFormula number format.",
            }),
        ),
        (
            "freezeAt",
            json!({
                "type": "string",
                "minLength": 2,
                "description": "freezePanes anchor, for example A2 to freeze the first row.",
            }),
        ),
        (
            "kind",
            json!({
                "type": "string",
                "enum": ["cellValue", "colorScale", "dataBar", "containsText"],
                "description": "addConditionalFormat rule kind.",
            }),
        ),
        (
            "operator",
            json!({
                "type": "string",
                "minLength": 1,
                "description": "cellValue operator such as greaterThan or between.",
            }),
        ),
        (
            "text",
            json!({
                "type": "string",
                "description": "containsText conditional-format text.",
            }),
        ),
        ("fillColor", json!({ "type": "string", "minLength": 1 })),
        ("minColor", json!({ "type": "string", "minLength": 1 })),
        ("midColor", json!({ "type": "string", "minLength": 1 })),
        ("maxColor", json!({ "type": "string", "minLength": 1 })),
        (
            "name",
            json!({
                "type": "string",
                "minLength": 1,
                "description": "Optional spreadsheet table display name.",
            }),
        ),
        (
            "headerRow",
            json!({ "type": "boolean", "description": "addTable; defaults to true." }),
        ),
        (
            "totalRow",
            json!({ "type": "boolean", "description": "addTable; defaults to false." }),
        ),
        ("chartType", chart_type_schema()),
        (
            "title",
            json!({ "type": "string", "minLength": 1, "description": "Chart title." }),
        ),
        (
            "dataRange",
            json!({
                "type": "string",
                "minLength": 2,
                "description": "addChart data range without a sheet prefix; sheetName is authoritative.",
            }),
        ),
        (
            "categoryRange",
            json!({
                "type": "string",
                "minLength": 2,
                "description": "Optional addChart category range without a sheet prefix.",
            }),
        ),
        (
            "newIndex",
            positive_integer_schema("One-based destination for moveSheet."),
        ),
    ])
}

fn presentation_semantic_schema_properties() -> Map<String, Value> {
    schema_properties([
        (
            "slideNumber",
            positive_integer_schema(
                "One-based slide used by inspect and presentation element operations.",
            ),
        ),
        (
            "newIndex",
            positive_integer_schema("One-based destination for moveSlide."),
        ),
        (
            "layout",
            json!({
                "type": "string",
                "minLength": 1,
                "description": "Optional addSlide layout name.",
            }),
        ),
        (
            "title",
            json!({
                "type": "string",
                "description": "addSlide title or addChart title.",
            }),
        ),
        (
            "body",
            json!({
                "type": "string",
                "description": "Optional addSlide body text.",
            }),
        ),
        (
            "backgroundColor",
            json!({
                "type": "string",
                "minLength": 1,
                "description": "Optional addSlide background color.",
            }),
        ),
        (
            "text",
            json!({
                "type": "string",
                "description": "Text for addText, addShape, or addFooter.",
            }),
        ),
        ("data", table_data_schema()),
        ("headerFill", json!({ "type": "string", "minLength": 1 })),
        ("chartType", chart_type_schema()),
        (
            "series",
            json!({
                "type": "array",
                "minItems": 1,
                "maxItems": 64,
                "items": {
                    "type": "object",
                    "properties": {
                        "name": { "type": "string", "minLength": 1 },
                        "values": {
                            "type": "array",
                            "minItems": 1,
                            "maxItems": 10000,
                            "items": { "type": "number" }
                        }
                    },
                    "required": ["name", "values"],
                    "additionalProperties": false
                }
            }),
        ),
        (
            "categories",
            json!({
                "type": "array",
                "maxItems": 10000,
                "items": { "type": "string" }
            }),
        ),
        (
            "shapeType",
            json!({
                "type": "string",
                "minLength": 1,
                "description": "addShape geometry such as rect, roundRect, ellipse, or line.",
            }),
        ),
        ("lineColor", json!({ "type": "string", "minLength": 1 })),
    ])
}

fn chart_type_schema() -> Value {
    json!({
        "type": "string",
        "enum": ["column", "bar", "line", "pie", "area", "scatter"],
    })
}

fn schema_properties<const N: usize>(entries: [(&str, Value); N]) -> Map<String, Value> {
    entries
        .into_iter()
        .map(|(name, schema)| (name.to_string(), schema))
        .collect()
}

fn reason_schema() -> Value {
    json!({
        "type": "string",
        "minLength": 1,
        "maxLength": AGENT_OFFICE_REASON_MAX_CHARS,
        "description": "Required concise, single-line user-facing purpose of this call. It is display and audit metadata only and never grants permission or approval.",
    })
}

fn timeout_schema() -> Value {
    json!({
        "type": "integer",
        "minimum": 1,
        "maximum": MAX_OFFICE_TIMEOUT_MS,
        "description": format!("Execution timeout in milliseconds; defaults to {DEFAULT_OFFICE_TIMEOUT_MS}."),
    })
}

fn positive_integer_schema(description: &str) -> Value {
    json!({ "type": "integer", "minimum": 1, "description": description })
}

fn file_path_schema(document_kind: OfficeDocumentKind) -> Value {
    let extensions = document_kind.accepted_extensions().join(", ");
    json!({
        "type": "string",
        "minLength": 1,
        "description": format!("Primary Office file path (expected extension: {extensions}). Relative paths are workspace-relative; external absolute paths and supported system aliases require the corresponding all-location permission."),
    })
}

fn destination_path_schema(document_kind: OfficeDocumentKind) -> Value {
    let extensions = document_kind.accepted_extensions().join(", ");
    json!({
        "type": "string",
        "minLength": 1,
        "description": format!("Optional save-as target (expected extension: {extensions}). The frozen source remains unchanged and the new target is staged and atomically published. Omit for an in-place edit."),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::office::{
        OfficeEngineAvailability, OfficeEngineCapabilities, OfficeEngineSource, OfficeEngineStatus,
        OfficeOperation, OfficeOperationParameters, OfficePreparedExecution, OFFICECLI_PROVIDER_ID,
        OFFICE_ENGINE_STATUS_SCHEMA_VERSION, OFFICE_PREPARED_EXECUTION_SCHEMA_VERSION,
    };
    use crate::protocol::{
        AgentApprovalStatus, AgentCommandPermission, AgentCommandSafetyPolicy,
        AgentPatchPermission, AgentPermissions, AgentReadPermission, AgentRunContext,
        AgentWorkspaceContext, AgentWritePermission,
    };
    use crate::AgentCancellationToken;
    use std::sync::atomic::AtomicBool;
    use tempfile::tempdir;

    #[derive(Clone)]
    struct PreparingOfficeEngine;

    impl OfficeEngine for PreparingOfficeEngine {
        fn capabilities(&self) -> OfficeEngineCapabilities {
            OfficeEngineCapabilities::office_cli()
        }

        fn status(&self, _cancellation: AgentCancellationToken) -> OfficeEngineStatus {
            OfficeEngineStatus {
                schema_version: OFFICE_ENGINE_STATUS_SCHEMA_VERSION,
                provider_id: OFFICECLI_PROVIDER_ID.to_string(),
                availability: OfficeEngineAvailability::Available,
                source: Some(OfficeEngineSource::Configured),
                version: Some("test".to_string()),
                engine_revision: Some("office-reason-test".to_string()),
                capabilities: self.capabilities(),
                error_code: None,
                message: None,
            }
        }

        fn prepare(
            &self,
            _context: &OfficeExecutionContext,
            request: &OfficeExecutionRequest,
        ) -> Result<OfficePreparedExecution, OfficeEngineError> {
            Ok(OfficePreparedExecution {
                schema_version: OFFICE_PREPARED_EXECUTION_SCHEMA_VERSION,
                provider_id: OFFICECLI_PROVIDER_ID.to_string(),
                engine_revision: "office-reason-test".to_string(),
                workspace_revision: None,
                access: request.access(),
                request: request.clone(),
                argv: vec![request.operation.cli_name().to_string()],
                paths: Vec::new(),
                input_bindings: Vec::new(),
                document_precondition: None,
                output_precondition: None,
                destination_precondition: None,
                resource_preconditions: Vec::new(),
            })
        }

        fn execute_prepared(
            &self,
            _context: &OfficeExecutionContext,
            prepared: &OfficePreparedExecution,
            _cancellation: AgentCancellationToken,
            _action_cancel_flag: Option<Arc<AtomicBool>>,
        ) -> Result<OfficeExecutionResult, OfficeEngineError> {
            Ok(OfficeExecutionResult {
                provider_id: prepared.provider_id.clone(),
                engine_revision: prepared.engine_revision.clone(),
                document_kind: prepared.request.document_kind,
                operation: prepared.request.operation,
                argv: prepared.argv.clone(),
                cwd: ".".to_string(),
                exit_code: Some(0),
                stdout: String::new(),
                stderr: String::new(),
                timed_out: false,
                cancelled: false,
                duration_ms: 0,
                stdout_truncated: false,
                stderr_truncated: false,
                error_code: None,
                error: None,
                outputs: Vec::new(),
            })
        }
    }

    fn office_args(operation: &str, reason: Option<Value>) -> Value {
        let mut args = match operation {
            "status" => json!({ "operation": "status" }),
            "get" | "inspect" => json!({
                "operation": "inspect",
                "filePath": "budget.xlsx",
                "sheetName": "Sheet1",
                "range": "A1"
            }),
            "set" | "writeCell" => json!({
                "operation": "writeCell",
                "filePath": "budget.xlsx",
                "sheetName": "Sheet1",
                "cell": "A1",
                "value": "Budget"
            }),
            other => json!({ "operation": other, "filePath": "budget.xlsx" }),
        };
        if let Some(reason) = reason {
            args["reason"] = reason;
        }
        args
    }

    fn typed_call(flat: Value) -> Value {
        flat
    }

    #[test]
    fn office_reason_is_required_and_bounded_for_read_and_status_calls() {
        for operation in ["status", "get"] {
            for (reason, expected_code) in [
                (None, "office.reason_required"),
                (Some(json!("   ")), "office.reason_required"),
                (
                    Some(json!("界".repeat(AGENT_OFFICE_REASON_MAX_CHARS + 1))),
                    "office.reason_too_long",
                ),
                (Some(json!("Inspect\nthe workbook")), "office.reason_unsafe"),
                (
                    Some(json!("Inspect\u{0007}the workbook")),
                    "office.reason_unsafe",
                ),
                (
                    Some(json!("Inspect\u{202e}the workbook")),
                    "office.reason_unsafe",
                ),
            ] {
                let error = parse_args(office_args(operation, reason), "office_spreadsheet")
                    .expect_err("invalid Office reason must be rejected");
                assert_eq!(error.code(), Some(expected_code));
            }
        }
    }

    #[test]
    fn office_reason_accepts_240_unicode_characters_and_normalizes_whitespace() {
        let bounded = "界".repeat(AGENT_OFFICE_REASON_MAX_CHARS);
        let parsed = parse_args(
            office_args("status", Some(json!(format!("  {bounded}  ")))),
            "office_spreadsheet",
        )
        .expect("a normalized 240-character reason is valid");

        assert_eq!(parsed.reason, bounded);
    }

    #[test]
    fn unsafe_office_reason_returns_stable_structured_diagnostics() {
        let error = parse_args(
            office_args("get", Some(json!("Inspect\u{2067}the workbook"))),
            "office_spreadsheet",
        )
        .expect_err("a Unicode bidi isolate must be rejected");

        assert_eq!(error.code(), Some("office.reason_unsafe"));
        let details = error.details().expect("structured reason diagnostics");
        assert_eq!(details["type"], "office_operation");
        assert_eq!(details["code"], "reasonUnsafe");
        assert_eq!(details["recovery"], "changeRequest");
    }

    #[test]
    fn retired_raw_help_is_rejected_with_semantic_recovery() {
        let error = parse_args(
            json!({
            "operation": "help",
            "verb": "create",
            "element": "document",
            "reason": "了解创建文档和添加元素的操作参数"
            }),
            "office_document",
        )
        .expect_err("raw provider help is not part of the semantic model surface");
        assert_eq!(error.code(), Some("office.semantic_invalid_request"));
        assert_eq!(
            error.details().expect("structured semantic error")["recovery"],
            "changeRequest"
        );
    }

    #[test]
    fn common_semantic_reads_do_not_require_approval() {
        let tool = OfficeTool::new(
            OfficeDocumentKind::Document,
            Arc::new(PreparingOfficeEngine),
        );
        for call in [
            json!({
                "operation": "status",
                "reason": "检查文档引擎状态"
            }),
            json!({
                "operation": "inspect",
                "filePath": "document.docx",
                "reason": "查看文档结构"
            }),
            json!({
                "operation": "validate",
                "filePath": "document.docx",
                "reason": "校验文档结构"
            }),
        ] {
            assert!(
                !tool.requires_approval_for_call(&call),
                "{call} must remain read-only"
            );
        }
    }

    #[test]
    fn office_call_projections_normalize_only_safe_reasons_without_mutating_execution_input() {
        let tool = OfficeTool::new(
            OfficeDocumentKind::Spreadsheet,
            Arc::new(PreparingOfficeEngine),
        );
        let legal = AgentToolCall {
            id: "office-observable-valid".to_string(),
            tool: "office_spreadsheet".to_string(),
            args: office_args("get", Some(json!("  Inspect the workbook  "))),
            approval_status: AgentApprovalStatus::NotRequired,
            reason: Some("Inspect the workbook".to_string()),
        };

        let trace_projection = tool.trace_call_projection(&legal);
        let event_projection = tool.event_call_projection(&legal);
        for projection in [&trace_projection, &event_projection] {
            assert_eq!(projection.reason.as_deref(), Some("Inspect the workbook"));
            assert_eq!(projection.args["reason"], "Inspect the workbook");
        }
        assert_eq!(legal.args["reason"], "  Inspect the workbook  ");

        let missing = AgentToolCall {
            id: "office-observable-missing".to_string(),
            tool: "office_spreadsheet".to_string(),
            args: office_args("get", None),
            approval_status: AgentApprovalStatus::NotRequired,
            reason: Some("must not trust the derived call field".to_string()),
        };
        for projection in [
            tool.trace_call_projection(&missing),
            tool.event_call_projection(&missing),
        ] {
            assert!(projection.args.get("reason").is_none());
            assert_eq!(projection.reason, None);
        }
        assert!(parse_args(missing.args, "office_spreadsheet").is_err());

        for (index, raw_reason) in [
            json!("   "),
            json!("x".repeat(AGENT_OFFICE_REASON_MAX_CHARS + 1)),
            json!("Inspect\nthe workbook"),
            json!("Inspect\u{0007}the workbook"),
            json!("Inspect\u{202e}the workbook"),
        ]
        .into_iter()
        .enumerate()
        {
            let call = AgentToolCall {
                id: format!("office-observable-invalid-{index}"),
                tool: "office_spreadsheet".to_string(),
                args: office_args("get", Some(raw_reason.clone())),
                approval_status: AgentApprovalStatus::NotRequired,
                reason: raw_reason.as_str().map(ToString::to_string),
            };
            for projection in [
                tool.trace_call_projection(&call),
                tool.event_call_projection(&call),
            ] {
                assert!(projection.args.get("reason").is_none());
                assert_eq!(projection.reason, None);
            }
            assert_eq!(call.args["reason"], raw_reason);
            assert!(parse_args(call.args, "office_spreadsheet").is_err());
        }
    }

    #[test]
    fn office_reason_content_does_not_change_read_write_approval_classification() {
        let tool = OfficeTool::new(
            OfficeDocumentKind::Spreadsheet,
            Arc::new(PreparingOfficeEngine),
        );

        for operation in ["status", "get"] {
            let first = office_args(operation, Some(json!("Inspect the workbook")));
            let second = office_args(operation, Some(json!("检查工作簿")));
            assert_eq!(
                tool.requires_approval_for_call(&first),
                tool.requires_approval_for_call(&second)
            );
            assert!(!tool.requires_approval_for_call(&first));
        }

        let first = office_args("set", Some(json!("Update the title")));
        let second = office_args("set", Some(json!("更新标题")));
        assert_eq!(
            tool.requires_approval_for_call(&first),
            tool.requires_approval_for_call(&second)
        );
        assert!(tool.requires_approval_for_call(&first));
    }

    #[test]
    fn write_proposal_binds_the_trimmed_reason_to_the_frozen_action() {
        let fixture = tempdir().unwrap();
        let context = ToolExecutionContext::from_run_context(Some(&AgentRunContext {
            conversation_id: None,
            project_id: None,
            workspace: Some(AgentWorkspaceContext {
                project_id: None,
                display_name: Some("workspace".to_string()),
                root_path: Some(fixture.path().to_string_lossy().to_string()),
            }),
            attachment_library: None,
            permissions: AgentPermissions {
                read: AgentReadPermission::WorkspaceOnly,
                write: AgentWritePermission::WorkspaceOnly,
                command: AgentCommandPermission::RequireApproval,
                command_safety: AgentCommandSafetyPolicy::Guarded,
                patch: AgentPatchPermission::RequireApproval,
            },
        }));
        let tool = OfficeTool::new(
            OfficeDocumentKind::Spreadsheet,
            Arc::new(PreparingOfficeEngine),
        );
        let call = AgentToolCall {
            id: "office-reason".to_string(),
            tool: "office_spreadsheet".to_string(),
            args: office_args("set", Some(json!("  Update the workbook title  "))),
            approval_status: AgentApprovalStatus::Required,
            reason: None,
        };

        let action = tool
            .proposed_action(&context, &call)
            .expect("valid write call should freeze an Office action");
        let AgentProposedAction::OfficeOperation { office_operation } = action else {
            panic!("Office write must produce a typed Office action");
        };
        assert_eq!(office_operation.reason, "Update the workbook title");
        assert_eq!(
            office_operation.schema_version,
            AGENT_OFFICE_OPERATION_SCHEMA_VERSION
        );
        assert!(crate::is_valid_agent_office_reason(
            &office_operation.reason
        ));
    }

    #[test]
    fn model_schema_is_portable_and_exposes_typed_fields_without_raw_arguments() {
        for kind in [
            OfficeDocumentKind::Document,
            OfficeDocumentKind::Spreadsheet,
            OfficeDocumentKind::Presentation,
        ] {
            let schema = office_input_schema(kind);
            crate::tools::schema::validate_portable_tool_input_schema("office", &schema)
                .expect("Office input schema must be portable across model providers");
            assert!(schema.get("oneOf").is_none());
            assert!(!contains_object_key(&schema, "arguments"));
            assert!(!contains_object_key(&schema, "request"));
            assert!(!contains_object_key(&schema, "target"));
            assert!(!contains_object_key(&schema, "parent"));
            assert_eq!(schema["type"], "object");
            assert_eq!(schema["additionalProperties"], false);
            assert_eq!(schema["required"], json!(["operation", "reason"]));
            assert!(schema["properties"]["operation"]["enum"]
                .as_array()
                .is_some_and(|operations| operations.len() >= 14));
            for field in ["operation", "reason", "filePath", "outputPath", "timeoutMs"] {
                assert!(schema["properties"].get(field).is_some(), "missing {field}");
            }
        }
    }

    #[test]
    fn model_schema_exposes_document_kind_specific_fields_only_where_supported() {
        let document = office_input_schema(OfficeDocumentKind::Document);
        let spreadsheet = office_input_schema(OfficeDocumentKind::Spreadsheet);
        let presentation = office_input_schema(OfficeDocumentKind::Presentation);

        assert!(document["properties"].get("blockKind").is_some());
        assert!(document["properties"].get("sheetName").is_none());
        assert!(document["properties"].get("slideNumber").is_none());

        assert!(spreadsheet["properties"].get("sheetName").is_some());
        assert!(spreadsheet["properties"].get("formula").is_some());
        assert!(spreadsheet["properties"].get("blockKind").is_none());
        assert!(spreadsheet["properties"].get("slideNumber").is_none());

        assert!(presentation["properties"].get("slideNumber").is_some());
        assert!(presentation["properties"].get("shapeType").is_some());
        assert!(presentation["properties"].get("sheetName").is_none());
        assert!(presentation["properties"].get("blockKind").is_none());
    }

    #[test]
    fn every_model_operation_parses_into_the_matching_typed_request() {
        let calls = [
            json!({
                "operation": "create",
                "filePath": "budget.xlsx",
                "overwriteExisting": true,
                "reason": "Create the workbook"
            }),
            json!({
                "operation": "render",
                "filePath": "budget.xlsx",
                "sheetName": "Sheet1",
                "range": "A1:E8",
                "viewport": { "width": 1600, "height": 1200 },
                "outputPath": "preview.png",
                "reason": "Render the workbook"
            }),
            json!({
                "operation": "inspect",
                "filePath": "budget.xlsx",
                "sheetName": "Sheet1",
                "range": "A1:E8",
                "depth": 2,
                "reason": "Read the workbook range"
            }),
            json!({
                "operation": "validate",
                "filePath": "budget.xlsx",
                "reason": "Validate the workbook"
            }),
            json!({
                "operation": "addSheet",
                "filePath": "budget.xlsx",
                "sheetName": "预算",
                "reason": "Add the budget sheet"
            }),
            json!({
                "operation": "writeCell",
                "filePath": "budget.xlsx",
                "sheetName": "预算",
                "cell": "A1",
                "value": "Budget",
                "style": { "bold": true },
                "reason": "Update the workbook title"
            }),
            json!({
                "operation": "setFormula",
                "filePath": "budget.xlsx",
                "sheetName": "预算",
                "cell": "E2",
                "formula": "=SUM(B2:D2)",
                "reason": "Set the quarterly formula"
            }),
            json!({
                "operation": "formatRange",
                "filePath": "budget.xlsx",
                "sheetName": "预算",
                "range": "A1:E1",
                "style": { "bold": true, "fillColor": "1F4E79" },
                "reason": "Format the header"
            }),
            json!({
                "operation": "freezePanes",
                "filePath": "budget.xlsx",
                "sheetName": "预算",
                "freezeAt": "A2",
                "reason": "Freeze the header row"
            }),
            json!({
                "operation": "addChart",
                "filePath": "budget.xlsx",
                "sheetName": "预算",
                "chartType": "column",
                "title": "Quarterly budget",
                "dataRange": "E2:E5",
                "categoryRange": "A2:A5",
                "anchor": "G2:N18",
                "reason": "Add the budget chart"
            }),
        ];
        let expected = [
            OfficeOperation::Create,
            OfficeOperation::View,
            OfficeOperation::Get,
            OfficeOperation::Validate,
            OfficeOperation::Add,
            OfficeOperation::Set,
            OfficeOperation::Set,
            OfficeOperation::Set,
            OfficeOperation::Set,
            OfficeOperation::Add,
        ];

        for (call, expected) in calls.into_iter().zip(expected) {
            let request = parse_args(call, "office_spreadsheet")
                .expect("typed model call should parse")
                .into_request()
                .expect("typed model call should validate");
            assert_eq!(request.operation, expected);
            assert_eq!(request.typed_parameters().unwrap().operation(), expected);
        }
    }

    #[test]
    fn model_input_rejects_legacy_argv_and_ambiguous_file_path_names() {
        for unexpected in [
            json!({
                "operation": "get",
                "filePath": "budget.xlsx",
                "arguments": ["/Sheet1", "--json"],
                "reason": "Inspect the workbook"
            }),
            json!({
                "operation": "get",
                "path": "budget.xlsx",
                "target": "/Sheet1",
                "reason": "Inspect the workbook"
            }),
            json!({
                "operation": "status",
                "timeoutMs": 1000,
                "reason": "Check workbook tools"
            }),
        ]
        .map(typed_call)
        {
            parse_args(unexpected, "office_spreadsheet")
                .expect_err("legacy, ambiguous, and status-only extra fields must fail closed");
        }

        parse_args(
            json!({
                "operation": "get",
                "filePath": "budget.xlsx",
                "reason": "Inspect the workbook"
            }),
            "office_spreadsheet",
        )
        .expect_err("the retired flat model envelope must fail closed");
    }

    #[test]
    fn semantic_image_input_compiles_without_model_visible_provider_tokens() {
        let request = parse_args(
            json!({
                "operation": "insertImage",
                "filePath": "deck.pptx",
                "slideNumber": 1,
                "source": { "type": "workspace", "path": "assets/hero.png" },
                "x": "1in",
                "y": "1in",
                "width": "12cm",
                "height": "7cm",
                "altText": "Hero illustration",
                "reason": "Add the local hero image"
            }),
            "office_presentation",
        )
        .unwrap()
        .into_request()
        .unwrap();

        let OfficeOperationParameters::Add { properties, .. } = request.typed_parameters().unwrap()
        else {
            panic!("add must remain a typed add request");
        };
        assert_eq!(
            properties["src"]["resourcePath"],
            "__mycopilot_agent_input__/office/image-input-1.png"
        );
        assert_eq!(properties["alt"], "Hero illustration");
        assert_eq!(properties["width"], "12cm");
        assert_eq!(request.inputs.len(), 1);
        assert_eq!(request.inputs[0].mount_path, "office/image-input-1.png");
        assert_eq!(
            request.inputs[0].source,
            AgentFileInputRef::Workspace {
                path: "assets/hero.png".to_string()
            }
        );
    }

    #[test]
    fn semantic_image_inputs_preserve_all_unified_source_variants() {
        let sources = [
            json!({ "type": "attachment", "readPath": "@attachments/a/hero.png" }),
            json!({ "type": "workspace", "path": "assets/hero.png" }),
            json!({ "type": "external", "path": "/tmp/hero.png" }),
            json!({
                "type": "generated_artifact",
                "uri": format!("image-artifact://sha256/{}", "a".repeat(64)),
                "path": "/app-data/image-generation-artifacts/objects/hero.png"
            }),
            json!({
                "type": "skill_resource",
                "uri": "skill://bundled:documents/references/hero.png?revision=abc"
            }),
        ];
        for source in sources {
            let request = parse_args(
                json!({
                    "operation": "insertImage",
                    "filePath": "document.docx",
                    "source": source,
                    "reason": "Insert the authorized image"
                }),
                "office_document",
            )
            .unwrap()
            .into_request()
            .unwrap();
            assert_eq!(request.inputs.len(), 1);
            let OfficeOperationParameters::Add { properties, .. } =
                request.typed_parameters().unwrap()
            else {
                panic!("insertImage must compile to add");
            };
            assert_eq!(
                properties["src"]["resourcePath"],
                format!("__mycopilot_agent_input__/{}", request.inputs[0].mount_path)
            );
        }
    }

    fn contains_object_key(value: &Value, key: &str) -> bool {
        match value {
            Value::Object(object) => {
                object.contains_key(key)
                    || object.values().any(|value| contains_object_key(value, key))
            }
            Value::Array(values) => values.iter().any(|value| contains_object_key(value, key)),
            _ => false,
        }
    }
}
