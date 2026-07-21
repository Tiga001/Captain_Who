use super::{AgentTool, AgentToolPermissionPolicy, FileWriteToolAccess, ToolExecutionContext};
use crate::office::{
    validate_office_request, OfficeCellShift, OfficeDocumentKind, OfficeElementPosition,
    OfficeEngine, OfficeEngineError, OfficeExecutionContext, OfficeExecutionRequest,
    OfficeExecutionResult, OfficeGridLayout, OfficeHelpVerb, OfficeOperation,
    OfficeOperationAccess, OfficeOperationParameters, OfficePageRange, OfficePropertyMap,
    OfficeRequestParameters, OfficeTextReplacement, OfficeViewMode, OfficeViewRenderMode,
    OfficeViewport, DEFAULT_OFFICE_TIMEOUT_MS, MAX_OFFICE_TIMEOUT_MS,
};
use crate::protocol::{
    has_unsafe_agent_office_reason_character, normalize_agent_office_reason, AgentError,
    AgentOfficeOperationRequest, AgentProposedAction, AgentResult, AgentToolCall,
    AgentToolDefinition, AgentToolSafety, AgentWritePermission,
    AGENT_OFFICE_OPERATION_SCHEMA_VERSION, AGENT_OFFICE_REASON_MAX_CHARS,
};
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

        let request = args.into_request(self.document_kind)?;
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
        let request = args.into_request(self.document_kind)?;
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
        let Ok(request) = args.into_request(self.document_kind) else {
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
            OfficeDocumentKind::Document => "Inspect, create, edit, render, and validate Word-compatible .docx documents through typed managed operations. Pass `{ request: { operation: ... }, reason: ... }` and call request.operation=status before first use. Put filePath and operation-specific fields such as target, parent, properties, pages, and outputPath inside request; provider flags are generated by the host. Read-only operations run immediately, while file changes run as frozen host actions under the normal file-write approval policy.",
            OfficeDocumentKind::Spreadsheet => "Inspect, create, edit, render, and validate Excel-compatible .xlsx/.xlsm/.csv files through typed managed operations. Pass `{ request: { operation: ... }, reason: ... }` and call request.operation=status before first use. Put filePath and operation-specific fields such as target, parent, properties, range, and outputPath inside request; provider flags are generated by the host. Read-only operations run immediately, while file changes run as frozen host actions under the normal file-write approval policy.",
            OfficeDocumentKind::Presentation => "Inspect, create, edit, render, and validate PowerPoint-compatible .pptx presentations through typed managed operations. Pass `{ request: { operation: ... }, reason: ... }` and call request.operation=status before first use. Put filePath and operation-specific fields such as target, parent, properties, pages, and outputPath inside request; provider flags are generated by the host. Read-only operations run immediately, while file changes run as frozen host actions under the normal file-write approval policy.",
        }
    }
}

fn office_execution_context(context: &ToolExecutionContext) -> AgentResult<OfficeExecutionContext> {
    Ok(OfficeExecutionContext::new(
        context.workspace_root_optional()?,
        context.permissions(),
        context.attachment_library().cloned(),
    ))
}

#[derive(Debug)]
struct OfficeToolArgs {
    operation: OfficeToolOperationWire,
    reason: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct OfficeToolCallWire {
    request: OfficeToolOperationWire,
    reason: String,
}

#[derive(Debug, Deserialize)]
#[serde(
    tag = "operation",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
enum OfficeToolOperationWire {
    Status {},
    Help {
        verb: Option<OfficeHelpVerb>,
        element: Option<String>,
        timeout_ms: Option<u64>,
    },
    Create {
        file_path: String,
        locale: Option<String>,
        #[serde(default)]
        minimal: bool,
        #[serde(default)]
        overwrite_existing: bool,
        timeout_ms: Option<u64>,
    },
    View {
        file_path: String,
        mode: OfficeViewMode,
        start: Option<u32>,
        end: Option<u32>,
        max_lines: Option<u32>,
        issue_type: Option<String>,
        limit: Option<u32>,
        #[serde(default)]
        columns: Vec<String>,
        #[serde(default)]
        pages: Vec<OfficePageRange>,
        range: Option<String>,
        viewport: Option<OfficeViewport>,
        grid: Option<OfficeGridLayout>,
        render_mode: Option<OfficeViewRenderMode>,
        #[serde(default)]
        include_page_count: bool,
        output_path: Option<String>,
        timeout_ms: Option<u64>,
    },
    Get {
        file_path: String,
        target: Option<String>,
        depth: Option<u32>,
        timeout_ms: Option<u64>,
    },
    Query {
        file_path: String,
        selector: String,
        contains_text: Option<String>,
        #[serde(default)]
        compact: bool,
        #[serde(default)]
        fields: Vec<String>,
        timeout_ms: Option<u64>,
    },
    Validate {
        file_path: String,
        timeout_ms: Option<u64>,
    },
    Set {
        file_path: String,
        target: String,
        #[serde(default)]
        properties: OfficePropertyMap,
        text_replacement: Option<OfficeTextReplacement>,
        #[serde(default)]
        override_protection: bool,
        destination_path: Option<String>,
        timeout_ms: Option<u64>,
    },
    Add {
        file_path: String,
        parent: String,
        element: String,
        copy_from: Option<String>,
        placement: Option<OfficeElementPosition>,
        #[serde(default)]
        properties: OfficePropertyMap,
        #[serde(default)]
        override_protection: bool,
        destination_path: Option<String>,
        timeout_ms: Option<u64>,
    },
    Remove {
        file_path: String,
        target: String,
        shift: Option<OfficeCellShift>,
        #[serde(default)]
        properties: OfficePropertyMap,
        destination_path: Option<String>,
        timeout_ms: Option<u64>,
    },
    Move {
        file_path: String,
        target: String,
        to_parent: Option<String>,
        placement: Option<OfficeElementPosition>,
        #[serde(default)]
        properties: OfficePropertyMap,
        destination_path: Option<String>,
        timeout_ms: Option<u64>,
    },
    Swap {
        file_path: String,
        first_target: String,
        second_target: String,
        destination_path: Option<String>,
        timeout_ms: Option<u64>,
    },
}

impl OfficeToolArgs {
    fn is_status(&self) -> bool {
        matches!(self.operation, OfficeToolOperationWire::Status {})
    }

    fn validate_status_call(&self, _tool_name: &str) -> AgentResult<()> {
        // The internally tagged, deny-unknown wire variant has no fields. A
        // status call carrying paths, timeouts, or mutation parameters cannot
        // deserialize and therefore never reaches this point.
        Ok(())
    }

    fn into_request(
        self,
        document_kind: OfficeDocumentKind,
    ) -> AgentResult<OfficeExecutionRequest> {
        let (operation, document_path, parameters, output_path, destination_path, timeout_ms) =
            match self.operation {
                OfficeToolOperationWire::Status {} => {
                    return Err(AgentError::new(
                        "The Office status operation does not create an execution request.",
                    ))
                }
                OfficeToolOperationWire::Help {
                    verb,
                    element,
                    timeout_ms,
                } => (
                    OfficeOperation::Help,
                    None,
                    OfficeOperationParameters::Help { verb, element },
                    None,
                    None,
                    timeout_ms,
                ),
                OfficeToolOperationWire::Create {
                    file_path,
                    locale,
                    minimal,
                    overwrite_existing,
                    timeout_ms,
                } => (
                    OfficeOperation::Create,
                    non_empty_owned(Some(file_path)),
                    OfficeOperationParameters::Create {
                        locale,
                        minimal,
                        overwrite: overwrite_existing,
                    },
                    None,
                    None,
                    timeout_ms,
                ),
                OfficeToolOperationWire::View {
                    file_path,
                    mode,
                    start,
                    end,
                    max_lines,
                    issue_type,
                    limit,
                    columns,
                    pages,
                    range,
                    viewport,
                    grid,
                    render_mode,
                    include_page_count,
                    output_path,
                    timeout_ms,
                } => (
                    OfficeOperation::View,
                    non_empty_owned(Some(file_path)),
                    OfficeOperationParameters::View {
                        mode,
                        start,
                        end,
                        max_lines,
                        issue_type,
                        limit,
                        columns,
                        pages,
                        range,
                        viewport,
                        grid,
                        render_mode,
                        page_count: include_page_count,
                    },
                    non_empty_owned(output_path),
                    None,
                    timeout_ms,
                ),
                OfficeToolOperationWire::Get {
                    file_path,
                    target,
                    depth,
                    timeout_ms,
                } => (
                    OfficeOperation::Get,
                    non_empty_owned(Some(file_path)),
                    OfficeOperationParameters::Get { target, depth },
                    None,
                    None,
                    timeout_ms,
                ),
                OfficeToolOperationWire::Query {
                    file_path,
                    selector,
                    contains_text,
                    compact,
                    fields,
                    timeout_ms,
                } => (
                    OfficeOperation::Query,
                    non_empty_owned(Some(file_path)),
                    OfficeOperationParameters::Query {
                        selector,
                        contains: contains_text,
                        compact,
                        fields,
                    },
                    None,
                    None,
                    timeout_ms,
                ),
                OfficeToolOperationWire::Validate {
                    file_path,
                    timeout_ms,
                } => (
                    OfficeOperation::Validate,
                    non_empty_owned(Some(file_path)),
                    OfficeOperationParameters::Validate,
                    None,
                    None,
                    timeout_ms,
                ),
                OfficeToolOperationWire::Set {
                    file_path,
                    target,
                    properties,
                    text_replacement,
                    override_protection,
                    destination_path,
                    timeout_ms,
                } => (
                    OfficeOperation::Set,
                    non_empty_owned(Some(file_path)),
                    OfficeOperationParameters::Set {
                        target,
                        properties,
                        replacement: text_replacement,
                        force: override_protection,
                    },
                    None,
                    non_empty_owned(destination_path),
                    timeout_ms,
                ),
                OfficeToolOperationWire::Add {
                    file_path,
                    parent,
                    element,
                    copy_from,
                    placement,
                    properties,
                    override_protection,
                    destination_path,
                    timeout_ms,
                } => (
                    OfficeOperation::Add,
                    non_empty_owned(Some(file_path)),
                    OfficeOperationParameters::Add {
                        parent,
                        element_type: element,
                        copy_from,
                        position: placement,
                        properties,
                        force: override_protection,
                    },
                    None,
                    non_empty_owned(destination_path),
                    timeout_ms,
                ),
                OfficeToolOperationWire::Remove {
                    file_path,
                    target,
                    shift,
                    properties,
                    destination_path,
                    timeout_ms,
                } => (
                    OfficeOperation::Remove,
                    non_empty_owned(Some(file_path)),
                    OfficeOperationParameters::Remove {
                        target,
                        shift,
                        properties,
                    },
                    None,
                    non_empty_owned(destination_path),
                    timeout_ms,
                ),
                OfficeToolOperationWire::Move {
                    file_path,
                    target,
                    to_parent,
                    placement,
                    properties,
                    destination_path,
                    timeout_ms,
                } => (
                    OfficeOperation::Move,
                    non_empty_owned(Some(file_path)),
                    OfficeOperationParameters::Move {
                        target,
                        new_parent: to_parent,
                        position: placement,
                        properties,
                    },
                    None,
                    non_empty_owned(destination_path),
                    timeout_ms,
                ),
                OfficeToolOperationWire::Swap {
                    file_path,
                    first_target,
                    second_target,
                    destination_path,
                    timeout_ms,
                } => (
                    OfficeOperation::Swap,
                    non_empty_owned(Some(file_path)),
                    OfficeOperationParameters::Swap {
                        first_target,
                        second_target,
                    },
                    None,
                    non_empty_owned(destination_path),
                    timeout_ms,
                ),
            };
        validate_model_kind_parameters(document_kind, &parameters)?;
        let request = OfficeExecutionRequest {
            document_kind,
            operation,
            document_path,
            parameters: OfficeRequestParameters::Typed(parameters),
            output_path,
            destination_path,
            timeout_ms,
        };
        validate_office_request(&request).map_err(map_engine_error)?;
        Ok(request)
    }
}

fn validate_model_kind_parameters(
    document_kind: OfficeDocumentKind,
    parameters: &OfficeOperationParameters,
) -> AgentResult<()> {
    match parameters {
        OfficeOperationParameters::View {
            grid,
            render_mode,
            ..
        } if document_kind == OfficeDocumentKind::Spreadsheet
            && (grid.is_some() || render_mode.is_some()) =>
        {
            Err(AgentError::new(
                "office_spreadsheet view does not accept grid or renderMode; use range, columns, pages, and viewport as applicable.",
            ))
        }
        OfficeOperationParameters::Query {
            compact, fields, ..
        } if document_kind == OfficeDocumentKind::Spreadsheet
            && (*compact || !fields.is_empty()) =>
        {
            Err(AgentError::new(
                "office_spreadsheet query does not accept compact or fields; use view mode=text with range or columns for compact worksheet inspection.",
            ))
        }
        _ => Ok(()),
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
    let parsed: OfficeToolCallWire = serde_json::from_value(value)
        .map_err(|error| AgentError::new(format!("{tool_name} parameters are invalid: {error}")))?;
    debug_assert_eq!(parsed.reason, raw_reason);
    Ok(OfficeToolArgs {
        operation: parsed.request,
        reason,
    })
}

pub(crate) fn validate_frozen_office_trace_args(
    frozen: &AgentOfficeOperationRequest,
    operation: &Value,
) -> Result<(), String> {
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
        .into_request(document_kind)
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
    // Some model providers reject composed schemas at the tool-input root. Keep the root as one
    // portable object and place the exact discriminated union one level below it. This preserves
    // provider compatibility without lying to the model about operation-specific required fields.
    let request_schemas = [
        status_schema(),
        help_schema(),
        create_schema(document_kind),
        view_schema(document_kind),
        get_schema(document_kind),
        query_schema(document_kind),
        validate_schema(document_kind),
        set_schema(document_kind),
        add_schema(document_kind),
        remove_schema(document_kind),
        move_schema(document_kind),
        swap_schema(document_kind),
    ];
    json!({
        "type": "object",
        "description": "Typed managed Office call. Select one exact request variant and provide a concise user-facing reason. Provider argv, output flags, and document-format tokens are generated and frozen by the host and are never accepted from callers.",
        "properties": {
            "request": {
                "description": "Exact operation-specific request. The selected branch declares every required and allowed field; do not add fields from another operation.",
                "oneOf": request_schemas,
            },
            "reason": reason_schema(),
        },
        "required": ["request", "reason"],
        "additionalProperties": false,
    })
}

fn status_schema() -> Value {
    office_operation_schema("status", Map::new(), &[])
}

fn office_operation_schema(
    operation: &'static str,
    mut properties: Map<String, Value>,
    operation_required: &[&str],
) -> Value {
    properties.insert(
        "operation".to_string(),
        json!({
            "type": "string",
            "enum": [operation],
            "description": format!("Run the typed `{operation}` Office operation."),
        }),
    );
    let required = std::iter::once("operation")
        .chain(operation_required.iter().copied())
        .collect::<Vec<_>>();
    json!({
        "type": "object",
        "properties": properties,
        "required": required,
        "additionalProperties": false,
    })
}

fn help_schema() -> Value {
    office_operation_schema(
        "help",
        schema_properties([
            (
                "verb",
                json!({
                    "type": "string",
                    "enum": ["get", "query", "set", "add", "remove", "move", "swap"],
                    "description": "Optional element-oriented operation whose provider schema should be inspected. The document format is inferred from the selected tool.",
                }),
            ),
            (
                "element",
                json!({
                    "type": "string",
                    "minLength": 1,
                    "description": "help: optional element name that requires verb and narrows the returned schema. add: required type of the element to create, such as paragraph, cell, slide, shape, table, chart, or picture.",
                }),
            ),
            ("timeoutMs", timeout_schema()),
        ]),
        &[],
    )
}

fn create_schema(document_kind: OfficeDocumentKind) -> Value {
    let mut properties = schema_properties([
        ("filePath", file_path_schema(document_kind)),
        (
            "overwriteExisting",
            json!({
                "type": "boolean",
                "description": "Whether this create operation is intended to replace an existing target. This never bypasses file permissions, approval, conflict checks, staging, or atomic publication.",
            }),
        ),
        ("timeoutMs", timeout_schema()),
    ]);
    if document_kind == OfficeDocumentKind::Document {
        properties.insert(
            "locale".to_string(),
            json!({
                "type": "string",
                "minLength": 1,
                "description": "Optional BCP-47 locale such as zh-CN. It selects document defaults and script-aware fonts.",
            }),
        );
        properties.insert(
            "minimal".to_string(),
            json!({
                "type": "boolean",
                "description": "Create a minimal raw OOXML document without the normal Word baseline. Prefer false for ordinary documents.",
            }),
        );
    }
    office_operation_schema("create", properties, &["filePath"])
}

fn view_schema(document_kind: OfficeDocumentKind) -> Value {
    let mut properties = schema_properties([
        ("filePath", file_path_schema(document_kind)),
        (
            "mode",
            json!({
                "type": "string",
                "enum": ["text", "annotated", "outline", "stats", "issues", "html", "svg", "screenshot", "forms"],
                "description": "Typed view mode. html, svg, and screenshot require outputPath and are file-write operations; the other modes are read-only.",
            }),
        ),
        (
            "start",
            positive_integer_schema("First line, row, paragraph, or slide to include."),
        ),
        (
            "end",
            positive_integer_schema("Last line, row, paragraph, or slide to include."),
        ),
        (
            "maxLines",
            positive_integer_schema("Maximum number of lines, rows, or slides returned."),
        ),
        ("issueType", issue_type_schema(document_kind)),
        (
            "limit",
            positive_integer_schema("Maximum number of matching results."),
        ),
        (
            "pages",
            json!({
                "type": "array",
                "maxItems": 64,
                "items": {
                    "type": "object",
                    "properties": {
                        "start": { "type": "integer", "minimum": 1 },
                        "end": { "type": "integer", "minimum": 1 }
                    },
                    "required": ["start"],
                    "additionalProperties": false
                },
                "description": "One-based page or slide ranges. Use start=end or omit end for one page. The host compiles the range expression.",
            }),
        ),
        (
            "range",
            json!({
                "type": "string",
                "minLength": 1,
                "description": "Office-internal region to inspect or render, such as a worksheet range or element data path. This is not a filesystem path.",
            }),
        ),
        (
            "viewport",
            json!({
                "type": "object",
                "properties": {
                    "width": { "type": "integer", "minimum": 1, "maximum": 16384 },
                    "height": { "type": "integer", "minimum": 1, "maximum": 16384 }
                },
                "required": ["width", "height"],
                "additionalProperties": false,
                "description": "Screenshot viewport dimensions. Valid only in screenshot mode.",
            }),
        ),
        ("outputPath", render_output_path_schema()),
        ("timeoutMs", timeout_schema()),
    ]);
    if document_kind == OfficeDocumentKind::Spreadsheet {
        properties.insert(
            "columns".to_string(),
            json!({
                "type": "array",
                "maxItems": 64,
                "items": { "type": "string", "minLength": 1 },
                "description": "Spreadsheet columns to include, for example [\"A\", \"B\", \"C\"]. The host generates the provider column list.",
            }),
        );
    } else {
        properties.insert(
            "grid".to_string(),
            json!({
                "description": "Optional screenshot contact-sheet layout. Use auto for a managed square layout or columns for an explicit column count. Valid only in screenshot mode.",
                "oneOf": [
                    {
                        "type": "object",
                        "properties": {
                            "mode": { "type": "string", "enum": ["auto"] }
                        },
                        "required": ["mode"],
                        "additionalProperties": false
                    },
                    {
                        "type": "object",
                        "properties": {
                            "mode": { "type": "string", "enum": ["columns"] },
                            "columns": { "type": "integer", "minimum": 1, "maximum": 32 }
                        },
                        "required": ["mode", "columns"],
                        "additionalProperties": false
                    }
                ]
            }),
        );
        properties.insert(
            "renderMode".to_string(),
            json!({
                "type": "string",
                "enum": ["auto", "html"],
                "description": "Managed screenshot renderer. Native desktop application launch is intentionally unavailable.",
            }),
        );
    }
    if document_kind == OfficeDocumentKind::Document {
        properties.insert(
            "includePageCount".to_string(),
            json!({
                "type": "boolean",
                "description": "Include the repaginated page count in document stats mode when supported.",
            }),
        );
    }
    office_operation_schema("view", properties, &["filePath", "mode"])
}

fn get_schema(document_kind: OfficeDocumentKind) -> Value {
    office_operation_schema(
        "get",
        schema_properties([
            ("filePath", file_path_schema(document_kind)),
            (
                "target",
                json!({
                    "type": "string",
                    "minLength": 1,
                    "description": "get/set/remove/move: Office DOM target path, range, cell, or element. get defaults to the document root. This is never a filesystem path.",
                }),
            ),
            (
                "depth",
                json!({
                    "type": "integer",
                    "minimum": 0,
                    "description": "Maximum child depth returned for the target node.",
                }),
            ),
            ("timeoutMs", timeout_schema()),
        ]),
        &["filePath"],
    )
}

fn query_schema(document_kind: OfficeDocumentKind) -> Value {
    let mut properties = schema_properties([
        ("filePath", file_path_schema(document_kind)),
        (
            "selector",
            json!({
                "type": "string",
                "minLength": 1,
                "description": "Element name or CSS-like Office selector, for example slide or shape[text=Hello]. Use get, not query, for paths beginning with '/'.",
            }),
        ),
        (
            "containsText",
            json!({
                "type": "string",
                "minLength": 1,
                "description": "Optional case-insensitive text substring used to filter matching elements.",
            }),
        ),
        ("timeoutMs", timeout_schema()),
    ]);
    if document_kind != OfficeDocumentKind::Spreadsheet {
        properties.insert(
            "compact".to_string(),
            json!({
                "type": "boolean",
                "description": "Return the stable compact one-line-per-element representation.",
            }),
        );
        properties.insert(
            "fields".to_string(),
            json!({
                "type": "array",
                "maxItems": 64,
                "items": { "type": "string", "minLength": 1 },
                "description": "Additional provider-reported format keys to append to compact output, such as x, y, or width.",
            }),
        );
    }
    office_operation_schema("query", properties, &["filePath", "selector"])
}

fn validate_schema(document_kind: OfficeDocumentKind) -> Value {
    office_operation_schema(
        "validate",
        schema_properties([
            ("filePath", file_path_schema(document_kind)),
            ("timeoutMs", timeout_schema()),
        ]),
        &["filePath"],
    )
}

fn set_schema(document_kind: OfficeDocumentKind) -> Value {
    office_operation_schema(
        "set",
        schema_properties([
            ("filePath", file_path_schema(document_kind)),
            (
                "target",
                mutation_target_schema("Office DOM element, range, or cell to update."),
            ),
            ("properties", property_map_schema()),
            (
                "textReplacement",
                json!({
                    "type": "object",
                    "properties": {
                        "find": { "type": "string", "minLength": 1 },
                        "replace": { "type": "string" }
                    },
                    "required": ["find", "replace"],
                    "additionalProperties": false,
                    "description": "Optional literal text replacement. At least one property or textReplacement is required.",
                }),
            ),
            ("overrideProtection", override_protection_schema()),
            ("destinationPath", destination_path_schema(document_kind)),
            ("timeoutMs", timeout_schema()),
        ]),
        &["filePath", "target"],
    )
}

fn add_schema(document_kind: OfficeDocumentKind) -> Value {
    office_operation_schema(
        "add",
        schema_properties([
            ("filePath", file_path_schema(document_kind)),
            (
                "parent",
                mutation_target_schema("Office DOM parent that receives the new element."),
            ),
            (
                "element",
                json!({
                    "type": "string",
                    "minLength": 1,
                    "description": "Element type reported by typed help, such as paragraph, cell, slide, shape, table, chart, or picture.",
                }),
            ),
            (
                "copyFrom",
                mutation_target_schema(
                    "Optional existing Office DOM element to copy. This is not a filesystem path.",
                ),
            ),
            ("placement", placement_schema()),
            ("properties", property_map_schema()),
            ("overrideProtection", override_protection_schema()),
            ("destinationPath", destination_path_schema(document_kind)),
            ("timeoutMs", timeout_schema()),
        ]),
        &["filePath", "parent", "element"],
    )
}

fn remove_schema(document_kind: OfficeDocumentKind) -> Value {
    let mut properties = schema_properties([
        ("filePath", file_path_schema(document_kind)),
        (
            "target",
            mutation_target_schema("Office DOM element, cell, row, or range to remove."),
        ),
        ("properties", property_map_schema()),
        ("destinationPath", destination_path_schema(document_kind)),
        ("timeoutMs", timeout_schema()),
    ]);
    if document_kind == OfficeDocumentKind::Spreadsheet {
        properties.insert(
            "shift".to_string(),
            json!({
                "type": "string",
                "enum": ["left", "up"],
                "description": "For a spreadsheet cell removal, shift surrounding cells left or up to fill the gap.",
            }),
        );
    }
    office_operation_schema("remove", properties, &["filePath", "target"])
}

fn move_schema(document_kind: OfficeDocumentKind) -> Value {
    office_operation_schema(
        "move",
        schema_properties([
            ("filePath", file_path_schema(document_kind)),
            ("target", mutation_target_schema("Office DOM element to move.")),
            (
                "toParent",
                mutation_target_schema("Optional Office DOM destination parent. Omit to reorder within the current parent."),
            ),
            ("placement", placement_schema()),
            ("properties", property_map_schema()),
            ("destinationPath", destination_path_schema(document_kind)),
            ("timeoutMs", timeout_schema()),
        ]),
        &["filePath", "target"],
    )
}

fn swap_schema(document_kind: OfficeDocumentKind) -> Value {
    office_operation_schema(
        "swap",
        schema_properties([
            ("filePath", file_path_schema(document_kind)),
            (
                "firstTarget",
                mutation_target_schema("First Office DOM element to swap."),
            ),
            (
                "secondTarget",
                mutation_target_schema("Second Office DOM element to swap."),
            ),
            ("destinationPath", destination_path_schema(document_kind)),
            ("timeoutMs", timeout_schema()),
        ]),
        &["filePath", "firstTarget", "secondTarget"],
    )
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

fn render_output_path_schema() -> Value {
    json!({
        "type": "string",
        "minLength": 1,
        "description": "Managed render output path. Required for html, svg, and screenshot modes; its extension must match the mode. The host constructs the provider output option.",
    })
}

fn mutation_target_schema(description: &str) -> Value {
    json!({
        "type": "string",
        "minLength": 1,
        "description": description,
    })
}

fn override_protection_schema() -> Value {
    json!({
        "type": "boolean",
        "description": "Allow the provider to edit a protected Office document. This does not bypass MyCopilot file permissions, approval, frozen preconditions, staging, or atomic publication.",
    })
}

fn placement_schema() -> Value {
    json!({
        "description": "Optional mutually exclusive insertion anchor. Omit to append.",
        "oneOf": [
            {
                "type": "object",
                "properties": {
                    "type": { "type": "string", "enum": ["index"] },
                    "index": { "type": "integer", "minimum": 0 }
                },
                "required": ["type", "index"],
                "additionalProperties": false
            },
            {
                "type": "object",
                "properties": {
                    "type": { "type": "string", "enum": ["after"] },
                    "target": { "type": "string", "minLength": 1 }
                },
                "required": ["type", "target"],
                "additionalProperties": false
            },
            {
                "type": "object",
                "properties": {
                    "type": { "type": "string", "enum": ["before"] },
                    "target": { "type": "string", "minLength": 1 }
                },
                "required": ["type", "target"],
                "additionalProperties": false
            }
        ]
    })
}

fn property_map_schema() -> Value {
    json!({
        "type": "object",
        "maxProperties": 96,
        "additionalProperties": {
            "oneOf": [
                { "type": "string" },
                { "type": "number" },
                { "type": "boolean" },
                {
                    "type": "object",
                    "properties": {
                        "resourcePath": {
                            "type": "string",
                            "minLength": 1,
                            "description": "Local input resource path to authorize, freeze, snapshot, and pass to the provider."
                        }
                    },
                    "required": ["resourcePath"],
                    "additionalProperties": false
                }
            ]
        },
        "description": "Typed provider-reported Office properties. Use scalar JSON values instead of key=value strings. Use {resourcePath: ...} only with documented file-bearing properties: background, csv, fallback, file, image, imagefill, imagepath, path, poster, preview, src, or template. Other property names accept scalar values only.",
    })
}

fn issue_type_schema(document_kind: OfficeDocumentKind) -> Value {
    let values: &[&str] = match document_kind {
        OfficeDocumentKind::Document => &[
            "format",
            "content",
            "structure",
            "field_not_evaluated",
            "field_cache_stale",
        ],
        OfficeDocumentKind::Spreadsheet => &[
            "format",
            "content",
            "structure",
            "formula_not_evaluated",
            "formula_cache_stale",
            "formula_ref_missing_sheet",
            "formula_eval_error",
            "chart_series_ref_missing_sheet",
            "chart_cache_stale",
            "definedname_broken",
            "definedname_target_missing",
        ],
        OfficeDocumentKind::Presentation => &[
            "format",
            "content",
            "structure",
            "slide_field_not_evaluated",
            "notes_unresolved_rid",
            "broken_part_ref",
            "low_contrast",
        ],
    };
    json!({
        "type": "string",
        "enum": values,
        "description": "Optional issue bucket or format-specific subtype. Valid only in issues mode.",
    })
}

fn non_empty_owned(value: Option<String>) -> Option<String> {
    value.and_then(|value| {
        let trimmed = value.trim();
        (!trimmed.is_empty()).then(|| trimmed.to_string())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::office::{
        OfficeEngineAvailability, OfficeEngineCapabilities, OfficeEngineSource, OfficeEngineStatus,
        OfficePreparedExecution, OFFICECLI_PROVIDER_ID, OFFICE_ENGINE_STATUS_SCHEMA_VERSION,
        OFFICE_PREPARED_EXECUTION_SCHEMA_VERSION,
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
            })
        }
    }

    fn office_args(operation: &str, reason: Option<Value>) -> Value {
        let mut request = json!({ "operation": operation });
        if operation != "status" {
            request["filePath"] = json!("budget.xlsx");
        }
        if operation == "set" {
            request["target"] = json!("/Sheet1/A1");
            request["properties"] = json!({ "value": "Budget" });
        }
        let mut args = json!({ "request": request });
        if let Some(reason) = reason {
            args["reason"] = reason;
        }
        args
    }

    fn typed_call(mut flat: Value) -> Value {
        let object = flat
            .as_object_mut()
            .expect("typed Office test call must be an object");
        let reason = object
            .remove("reason")
            .expect("typed Office test call must include reason");
        json!({ "request": flat, "reason": reason })
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
            assert!(!contains_object_key(&schema, "path"));
            assert_eq!(schema["type"], "object");
            assert_eq!(schema["additionalProperties"], false);
            assert_eq!(schema["required"], json!(["request", "reason"]));
            assert_eq!(
                schema["properties"]
                    .as_object()
                    .expect("root properties")
                    .keys()
                    .cloned()
                    .collect::<std::collections::BTreeSet<_>>(),
                ["reason".to_string(), "request".to_string()]
                    .into_iter()
                    .collect()
            );
            assert_eq!(
                schema["properties"]["request"]["oneOf"]
                    .as_array()
                    .expect("typed operation union")
                    .len(),
                12
            );
            for operation in [
                "status", "help", "create", "view", "get", "query", "validate", "set", "add",
                "remove", "move", "swap",
            ] {
                request_schema_for(&schema, operation);
            }
            for field in [
                "filePath",
                "verb",
                "element",
                "overwriteExisting",
                "mode",
                "outputPath",
                "target",
                "selector",
                "containsText",
                "properties",
                "textReplacement",
                "parent",
                "copyFrom",
                "placement",
                "overrideProtection",
                "destinationPath",
                "toParent",
                "firstTarget",
                "secondTarget",
                "timeoutMs",
            ] {
                assert!(
                    contains_object_key(&schema["properties"]["request"], field),
                    "missing typed field {field} for {kind:?}"
                );
            }
        }
    }

    #[test]
    fn model_schema_exposes_document_kind_specific_fields_only_where_supported() {
        let document = office_input_schema(OfficeDocumentKind::Document);
        let spreadsheet = office_input_schema(OfficeDocumentKind::Spreadsheet);
        let presentation = office_input_schema(OfficeDocumentKind::Presentation);

        let document_create = request_schema_for(&document, "create");
        let spreadsheet_create = request_schema_for(&spreadsheet, "create");
        assert!(document_create["properties"].get("locale").is_some());
        assert!(document_create["properties"].get("minimal").is_some());
        assert!(spreadsheet_create["properties"].get("locale").is_none());
        assert!(spreadsheet_create["properties"].get("minimal").is_none());

        let document_view = request_schema_for(&document, "view");
        let spreadsheet_view = request_schema_for(&spreadsheet, "view");
        let presentation_view = request_schema_for(&presentation, "view");
        assert!(document_view["properties"]
            .get("includePageCount")
            .is_some());
        assert!(document_view["properties"].get("renderMode").is_some());
        assert!(presentation_view["properties"].get("renderMode").is_some());
        assert!(presentation_view["properties"]
            .get("includePageCount")
            .is_none());
        assert!(spreadsheet_view["properties"].get("columns").is_some());
        assert!(spreadsheet_view["properties"].get("grid").is_none());
        assert!(spreadsheet_view["properties"].get("renderMode").is_none());
        assert!(document_view["properties"].get("grid").is_some());

        assert!(request_schema_for(&spreadsheet, "remove")["properties"]
            .get("shift")
            .is_some());
        assert!(request_schema_for(&document, "remove")["properties"]
            .get("shift")
            .is_none());
        assert!(request_schema_for(&document, "query")["properties"]
            .get("compact")
            .is_some());
        assert!(request_schema_for(&spreadsheet, "query")["properties"]
            .get("compact")
            .is_none());
    }

    #[test]
    fn every_model_operation_parses_into_the_matching_typed_request() {
        let calls = [
            json!({
                "operation": "help",
                "verb": "add",
                "element": "cell",
                "reason": "Inspect the cell schema"
            }),
            json!({
                "operation": "create",
                "filePath": "budget.xlsx",
                "overwriteExisting": true,
                "reason": "Create the workbook"
            }),
            json!({
                "operation": "view",
                "filePath": "budget.xlsx",
                "mode": "screenshot",
                "range": "Sheet1!A1:E8",
                "viewport": { "width": 1600, "height": 1200 },
                "outputPath": "preview.png",
                "reason": "Render the workbook"
            }),
            json!({
                "operation": "get",
                "filePath": "budget.xlsx",
                "target": "/Sheet1/A1:E8",
                "depth": 2,
                "reason": "Read the workbook range"
            }),
            json!({
                "operation": "query",
                "filePath": "budget.xlsx",
                "selector": "cell",
                "containsText": "Budget",
                "reason": "Find matching workbook cells"
            }),
            json!({
                "operation": "validate",
                "filePath": "budget.xlsx",
                "reason": "Validate the workbook"
            }),
            json!({
                "operation": "set",
                "filePath": "budget.xlsx",
                "target": "/Sheet1/A1",
                "properties": { "value": "Budget", "bold": true },
                "reason": "Update the workbook title"
            }),
            json!({
                "operation": "add",
                "filePath": "budget.xlsx",
                "parent": "/Sheet1",
                "element": "chart",
                "placement": { "type": "index", "index": 0 },
                "properties": { "title": "Quarterly budget" },
                "reason": "Add the budget chart"
            }),
            json!({
                "operation": "remove",
                "filePath": "budget.xlsx",
                "target": "/Sheet1/A2",
                "shift": "up",
                "reason": "Remove the obsolete value"
            }),
            json!({
                "operation": "move",
                "filePath": "budget.xlsx",
                "target": "/Sheet1/chart[1]",
                "toParent": "/Sheet1",
                "placement": { "type": "after", "target": "/Sheet1/chart[2]" },
                "reason": "Reorder the workbook charts"
            }),
            json!({
                "operation": "swap",
                "filePath": "budget.xlsx",
                "firstTarget": "/Sheet1/chart[1]",
                "secondTarget": "/Sheet1/chart[2]",
                "reason": "Swap the workbook charts"
            }),
        ]
        .map(typed_call);
        let expected = [
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
        ];

        for (call, expected) in calls.into_iter().zip(expected) {
            let request = parse_args(call, "office_spreadsheet")
                .expect("typed model call should parse")
                .into_request(OfficeDocumentKind::Spreadsheet)
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
    fn typed_properties_preserve_scalar_and_resource_values_without_provider_tokens() {
        let request = parse_args(
            typed_call(json!({
                "operation": "add",
                "filePath": "deck.pptx",
                "parent": "/slide[1]",
                "element": "picture",
                "properties": {
                    "src": { "resourcePath": "assets/hero.png" },
                    "width": "12cm",
                    "decorative": false,
                    "opacity": 0.8
                },
                "reason": "Add the local hero image"
            })),
            "office_presentation",
        )
        .unwrap()
        .into_request(OfficeDocumentKind::Presentation)
        .unwrap();

        let OfficeOperationParameters::Add { properties, .. } = request.typed_parameters().unwrap()
        else {
            panic!("add must remain a typed add request");
        };
        assert_eq!(properties["src"]["resourcePath"], "assets/hero.png");
        assert_eq!(properties["decorative"], false);
        assert_eq!(properties["opacity"], 0.8);
    }

    fn request_schema_for<'a>(schema: &'a Value, operation: &str) -> &'a Value {
        schema["properties"]["request"]["oneOf"]
            .as_array()
            .expect("Office request must be a typed union")
            .iter()
            .find(|branch| {
                branch["properties"]["operation"]["enum"]
                    .as_array()
                    .is_some_and(|values| values.len() == 1 && values[0] == operation)
            })
            .unwrap_or_else(|| panic!("missing typed schema for {operation}"))
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
