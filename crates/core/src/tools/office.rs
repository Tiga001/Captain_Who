use super::{AgentTool, AgentToolPermissionPolicy, FileWriteToolAccess, ToolExecutionContext};
use crate::office::{
    validate_office_request, OfficeDocumentKind, OfficeEngine, OfficeEngineError,
    OfficeExecutionContext, OfficeExecutionRequest, OfficeExecutionResult, OfficeOperation,
    OfficeOperationAccess, DEFAULT_OFFICE_TIMEOUT_MS, MAX_OFFICE_ARGUMENTS, MAX_OFFICE_TIMEOUT_MS,
};
use crate::protocol::{
    AgentError, AgentOfficeOperationRequest, AgentProposedAction, AgentResult, AgentToolCall,
    AgentToolDefinition, AgentToolSafety, AgentWritePermission,
    AGENT_OFFICE_OPERATION_SCHEMA_VERSION,
};
use serde::Deserialize;
use serde_json::{json, Value};
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
        let reason = non_empty(args.reason.as_deref())
            .map(truncate_reason)
            .or_else(|| call.reason.as_deref().map(truncate_reason));
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

    fn tool_name(&self) -> &'static str {
        match self.document_kind {
            OfficeDocumentKind::Document => "office_document",
            OfficeDocumentKind::Spreadsheet => "office_spreadsheet",
            OfficeDocumentKind::Presentation => "office_presentation",
        }
    }

    fn description(&self) -> &'static str {
        match self.document_kind {
            OfficeDocumentKind::Document => "Inspect, create, edit, render, and validate Word-compatible .docx documents through the managed Office engine. Call operation=status before first use. Read-only operations run immediately; file changes run as frozen host actions under the same approval policy as other file-write tools. Pass arguments as literal argv tokens after the operation and document path; no shell is used.",
            OfficeDocumentKind::Spreadsheet => "Inspect, create, edit, render, and validate Excel-compatible .xlsx/.xlsm/.csv files through the managed Office engine. Call operation=status before first use. Read-only operations run immediately; file changes run as frozen host actions under the same approval policy as other file-write tools. Pass arguments as literal argv tokens after the operation and workbook path; no shell is used.",
            OfficeDocumentKind::Presentation => "Inspect, create, edit, render, and validate PowerPoint-compatible .pptx presentations through the managed Office engine. Call operation=status before first use. Read-only operations run immediately; file changes run as frozen host actions under the same approval policy as other file-write tools. Pass arguments as literal argv tokens after the operation and presentation path; no shell is used.",
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

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct OfficeToolArgs {
    operation: String,
    path: Option<String>,
    #[serde(default)]
    arguments: Vec<String>,
    output_path: Option<String>,
    destination_path: Option<String>,
    timeout_ms: Option<u64>,
    reason: Option<String>,
}

impl OfficeToolArgs {
    fn is_status(&self) -> bool {
        self.operation.trim().eq_ignore_ascii_case("status")
    }

    fn validate_status_call(&self, tool_name: &str) -> AgentResult<()> {
        if self.path.is_some()
            || !self.arguments.is_empty()
            || self.output_path.is_some()
            || self.destination_path.is_some()
            || self.timeout_ms.is_some()
        {
            return Err(AgentError::new(format!(
                "{tool_name}.status cannot receive file paths, provider arguments, or a timeout."
            )));
        }
        Ok(())
    }

    fn into_request(
        self,
        document_kind: OfficeDocumentKind,
    ) -> AgentResult<OfficeExecutionRequest> {
        let operation =
            OfficeOperation::parse_supported(&self.operation).map_err(map_engine_error)?;
        let request = OfficeExecutionRequest {
            document_kind,
            operation,
            document_path: non_empty_owned(self.path),
            arguments: self.arguments,
            output_path: non_empty_owned(self.output_path),
            destination_path: non_empty_owned(self.destination_path),
            timeout_ms: self.timeout_ms,
        };
        validate_office_request(&request).map_err(map_engine_error)?;
        Ok(request)
    }
}

fn parse_args(value: Value, tool_name: &str) -> AgentResult<OfficeToolArgs> {
    let args: OfficeToolArgs = serde_json::from_value(value)
        .map_err(|error| AgentError::new(format!("{tool_name} parameters are invalid: {error}")))?;
    if args.operation.trim().is_empty() {
        return Err(AgentError::new(format!(
            "{tool_name}.operation cannot be empty."
        )));
    }
    if args.arguments.len() > MAX_OFFICE_ARGUMENTS {
        return Err(AgentError::structured(
            "office.argv_too_large",
            "Office arguments exceed the bounded argument-count limit.",
            json!({
                "type": "office_operation",
                "code": "argvTooLarge",
                "maxArguments": MAX_OFFICE_ARGUMENTS,
            }),
        ));
    }
    Ok(args)
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
    let reason_was_present = args.reason.is_some();
    let reason = non_empty(args.reason.as_deref()).map(truncate_reason);
    let request = args
        .into_request(document_kind)
        .map_err(|_| format!("{tool_name} frozen ToolCall request is invalid"))?;
    if request != frozen.prepared.request || (reason_was_present && reason != frozen.reason) {
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
    let extensions = document_kind.accepted_extensions().join(", ");
    json!({
        "type": "object",
        "properties": {
            "operation": {
                "type": "string",
                "enum": ["status", "help", "create", "view", "get", "query", "validate", "set", "add", "remove", "move", "swap"],
                "description": "Managed Office operation. status probes the engine without starting a file transaction."
            },
            "path": {
                "type": "string",
                "description": format!("Office file path. Relative paths are workspace-relative; absolute paths and @home/@desktop/@documents/@downloads aliases require the corresponding all-location permission. Registered @attachments paths are accepted only for read-source use. Expected extension: {extensions}. Omit only for status/help.")
            },
            "arguments": {
                "type": "array",
                "maxItems": MAX_OFFICE_ARGUMENTS,
                "items": { "type": "string" },
                "description": "Literal OfficeCLI argv tokens placed after the operation and primary document path. Shell syntax is never interpreted; output flags and unsafe administrative/network operations are rejected."
            },
            "outputPath": {
                "type": "string",
                "description": "Managed output path for view html/screenshot/svg rendering. Relative paths are workspace-relative; external absolute paths or system aliases require write=all. The host owns construction of the output flag."
            },
            "destinationPath": {
                "type": "string",
                "description": "Optional managed save-as target for set/add/remove/move/swap. Relative paths are workspace-relative; external absolute paths or system aliases require write=all. The source remains unchanged; omit for an in-place mutation."
            },
            "timeoutMs": {
                "type": "integer",
                "minimum": 1,
                "maximum": MAX_OFFICE_TIMEOUT_MS,
                "description": format!("Execution timeout in milliseconds; defaults to {DEFAULT_OFFICE_TIMEOUT_MS}.")
            },
            "reason": {
                "type": "string",
                "maxLength": 2000,
                "description": "Short user-facing reason for a file-changing approval request."
            }
        },
        "required": ["operation"],
        "additionalProperties": false
    })
}

fn non_empty(value: Option<&str>) -> Option<&str> {
    value.map(str::trim).filter(|value| !value.is_empty())
}

fn non_empty_owned(value: Option<String>) -> Option<String> {
    value.and_then(|value| {
        let trimmed = value.trim();
        (!trimmed.is_empty()).then(|| trimmed.to_string())
    })
}

fn truncate_reason(value: &str) -> String {
    value.trim().chars().take(2_000).collect()
}
