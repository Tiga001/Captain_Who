use super::{clean_relative_path, AgentTool, ToolExecutionContext};
use crate::command::{
    classify_command_risk, infer_managed_artifact_builder_command,
    infer_managed_artifact_command_kind, infer_managed_pdf_command_kind,
    infer_managed_pdf_workspace_inputs, is_presentation_editor_direct_command,
    normalize_command_text, validate_command_runtime_binding,
    validate_managed_artifact_builder_output_scope, MANAGED_OFFICE_SCRIPT_RESERVED_MOUNT_PREFIX,
    MAX_ADDITIONAL_ROOTS, MAX_COMMAND_CHARS, MAX_EXPECTED_OUTPUTS, MAX_OBSERVATION_PATH_CHARS,
    PRESENTATION_EDITOR_RESERVED_MOUNT_PREFIX, PRESENTATION_EDITOR_SCRIPT_MOUNT_PATH,
};
use crate::file_input::{
    agent_file_input_ref_from_model_path, agent_file_input_ref_matches_model_path,
    default_agent_file_input_mount_path, normalize_agent_file_input_specs,
    prepare_agent_file_input_bindings, resolve_verified_agent_file_input_path,
    AgentFileInputExecutionContext, MAX_AGENT_FILE_INPUTS, MAX_AGENT_FILE_INPUT_MOUNT_PATH_CHARS,
};
use crate::protocol::{
    AgentApprovalStatus, AgentCommandArtifactObservationKind,
    AgentCommandArtifactObservationRequest, AgentCommandRequest, AgentCommandRuntimeProfile,
    AgentError, AgentFileInputRef, AgentFileInputSpec, AgentProposedAction, AgentResult,
    AgentSkillMaterializationResult, AgentSkillMaterializationResultStatus, AgentToolCall,
    AgentToolDefinition, AgentToolResult, AgentToolSafety,
};
use crate::skills::{
    SkillResourceUri, APPLICATION_BUNDLED_SKILL_SOURCE_ID, DOCUMENTS_LOCAL_ID, PDF_LOCAL_ID,
    PRESENTATIONS_LOCAL_ID, SPREADSHEETS_LOCAL_ID,
};
use crate::system_paths::expand_system_path;
use serde::Deserialize;
use serde_json::{json, Map, Value};
use std::collections::BTreeSet;
use std::path::{Component, Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum TrustedManagedBuilderPurpose {
    Create,
    EditDocument,
    EditSpreadsheet,
    EditPresentation,
}

impl TrustedManagedBuilderPurpose {
    fn is_editor(self) -> bool {
        !matches!(self, Self::Create)
    }

    fn office_purpose(self) -> crate::office::OfficeManagedScriptPurpose {
        match self {
            Self::Create => crate::office::OfficeManagedScriptPurpose::Create,
            Self::EditPresentation => {
                crate::office::OfficeManagedScriptPurpose::EditPresentationPlan
            }
            Self::EditDocument | Self::EditSpreadsheet => {
                crate::office::OfficeManagedScriptPurpose::Edit
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct TrustedManagedBuilder {
    profile: AgentCommandRuntimeProfile,
    purpose: TrustedManagedBuilderPurpose,
    workspace_script_path: String,
}

fn managed_office_script_mount(builder: &TrustedManagedBuilder) -> String {
    if builder.purpose == TrustedManagedBuilderPurpose::EditPresentation {
        return PRESENTATION_EDITOR_SCRIPT_MOUNT_PATH.to_string();
    }
    let profile = match builder.profile {
        AgentCommandRuntimeProfile::Documents => "documents",
        AgentCommandRuntimeProfile::Spreadsheets => "spreadsheets",
        AgentCommandRuntimeProfile::Presentations => "presentations",
        AgentCommandRuntimeProfile::Pdf => "pdf",
    };
    let name = Path::new(&builder.workspace_script_path)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("script");
    format!("{MANAGED_OFFICE_SCRIPT_RESERVED_MOUNT_PREFIX}/{profile}/{name}")
}

pub(super) struct RunCommandTool;

impl AgentTool for RunCommandTool {
    fn exposure(&self) -> super::AgentToolExposure {
        super::AgentToolExposure::Stable
    }

    fn permission_policy(&self) -> super::AgentToolPermissionPolicy {
        super::AgentToolPermissionPolicy::Default
    }

    fn definition(&self) -> AgentToolDefinition {
        AgentToolDefinition {
            name: "run_command".to_string(),
            description: "Run one bounded non-interactive shell command through the host for builds, tests, queries, dependency management, or program execution. Before the first ordinary call, inspect World State workspace.binding and follow the cwd field contract; without a workspace, the first call must include cwd. Never use run_command, shell redirection, a heredoc, or an inline script as an alternate writer for ordinary text/code/config files or to bypass file-write approval; use apply_patch action=apply for short Direct changes and apply_patch Staged actions for long or staged content. Explicit managed Skill/Builder workflows retain their narrower Host-owned contracts. Host policy may execute, request approval, or deny catastrophic/unsupported operations; it is not an OS sandbox. The Host owns process lifetime and its short initial yield: do not add a deadline merely to bound waiting or confirm startup. status=running returns a sessionId and is not final success. For a required build/test/serial result, use command_session action=wait without repeated polling or waiting narration; a GUI app or long-lived server normally only needs startup confirmation. Background output and exit update Host state but never start a model turn. A backend-verified Office Skill Builder uses one direct Python/Node command with --output <file.docx|file.xlsx|file.pptx>; Host binds the runtime and observes output, runtimeProfile must be omitted and observe is optional. Other trusted activated Skills may also bind a managed runtime and publication contract: follow their instructions, never guess Host paths or runtimeProfile. inputs binds authorized files read-only under $MYCOPILOT_INPUT_ROOT/<mountPath>; copy input paths from tool results or the user, never private Host storage paths.".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "command": { "type": "string", "maxLength": MAX_COMMAND_CHARS, "description": "Non-interactive shell command. Newlines are allowed; CRLF/CR normalize to LF. Host checks every newline, pipeline, &&, || and ; segment. Use a quoted heredoc delimiter such as <<'PY' for bounded non-writing input; unquoted or shell-interpreter heredocs, here-strings, background execution and NUL are denied." },
                    "cwd": { "type": "string", "description": "Working directory, based on World State workspace.binding before the first ordinary call. With a workspace, omit for its root or use a workspace-relative directory. When no workspace is selected, cwd is mandatory even when command/executable/argument paths are absolute: use an existing absolute directory or @home, @desktop, @documents, @downloads with an optional safe child path (e.g. @desktop/project-dir), normally the target file's parent. It cannot be relative or `.` without a workspace. Absolute directories and aliases require the write scope to allow all locations (write=all). Only a backend-recognized managed PDF command whose activated PDF Skill supplies a Host-owned private working directory may omit cwd without a workspace." },
                    "reason": { "type": "string", "description": "Why this command is needed and what result is expected." },
                    "observe": {
                        "type": "object",
                        "description": "Optional best-effort Office artifact observation; verified Builders are observed automatically. Paths resolve relative to cwd. Grants no command/read/write permission.",
                        "properties": {
                            "kinds": {
                                "type": "array",
                                "items": { "type": "string", "enum": ["office"] },
                                "minItems": 1,
                                "maxItems": 1
                            },
                            "expectedOutputs": {
                                "type": "array",
                                "items": { "type": "string", "minLength": 1, "maxLength": MAX_OBSERVATION_PATH_CHARS },
                                "maxItems": MAX_EXPECTED_OUTPUTS,
                                "description": "Exact Office output files to observe; sibling files are not enumerated. Observation does not change command success."
                            },
                            "additionalRoots": {
                                "type": "array",
                                "items": { "type": "string", "minLength": 1, "maxLength": MAX_OBSERVATION_PATH_CHARS },
                                "maxItems": MAX_ADDITIONAL_ROOTS,
                                "description": "Additional Office files/directories to observe beyond the workspace. Recursive external directory scans require read=all."
                            }
                        },
                        "required": ["kinds"],
                        "additionalProperties": false
                    },
                    "runtimeProfile": {
                        "type": "string",
                        "enum": ["documents", "spreadsheets", "presentations"],
                        "description": "Optional managed runtime for a custom saved .mjs/.py Office script. Verified Skill Builders must omit: Host verifies the run-scoped materialization receipt, derives Node/Python and profile, and freezes packages, version and integrity identity. Never supply package versions. No additional command/file permission or fallback to PATH."
                    },
                    "inputs": {
                        "type": "array",
                        "maxItems": MAX_AGENT_FILE_INPUTS,
                        "description": "Optional authorized read-only inputs for any command. Host freezes hash/size before approval, revalidates before execution, and exposes $MYCOPILOT_INPUT_ROOT/<mountPath>. Use inputs to copy browser-download references into the workspace without revealing private storage paths.",
                        "items": {
                            "type": "object",
                            "properties": {
                                "path": {
                                    "type": "string",
                                    "minLength": 1,
                                    "description": "A workspace-relative path, absolute path, system alias, @attachments/... path, browser-download:... reference, image-artifact://... or artifact://... URI, or revision-bound skill://... URI."
                                },
                                "mountPath": {
                                    "type": "string",
                                    "minLength": 1,
                                    "maxLength": MAX_AGENT_FILE_INPUT_MOUNT_PATH_CHARS,
                                    "description": "Optional stable safe relative input path. If omitted, the host uses the source filename."
                                }
                            },
                            "required": ["path"],
                            "additionalProperties": false
                        }
                    }
                },
                "required": ["command"],
                "additionalProperties": false
            }),
            safety: AgentToolSafety::RequiresApproval,
            requires_workspace: false,
            requires_approval: true,
            approval_mode: crate::protocol::AgentToolApprovalMode::Always,
        }
    }

    fn execute(&self, _context: &ToolExecutionContext, _args: Value) -> AgentResult<Value> {
        Err(AgentError::new(
            "run_command 需要用户审批，不能由 agent runtime 自动执行。",
        ))
    }

    fn proposed_action(
        &self,
        context: &ToolExecutionContext,
        call: &AgentToolCall,
    ) -> AgentResult<AgentProposedAction> {
        Ok(AgentProposedAction::Command {
            command: command_request_from_call(context, call)?,
        })
    }

    fn model_projection(&self, result: &AgentToolResult) -> AgentToolResult {
        run_command_model_projection(result)
    }
}

pub(super) fn run_command_model_projection(result: &AgentToolResult) -> AgentToolResult {
    let projected = result
        .result
        .as_ref()
        .and_then(project_command_result_value);
    super::model_projection::compact_model_result(result, projected)
}

fn project_command_result_value(value: &Value) -> Option<Value> {
    if value.get("status").and_then(Value::as_str) == Some("rejected") {
        // Approval rejection is a successfully settled user decision, not a command result.
        // Keep its authority and feedback visible when projecting both live and replayed facts.
        let feedback = value
            .get("userFeedback")
            .or_else(|| {
                // Older Host receipts stored feedback as message. New projected messages are
                // system guidance and must never become invented user feedback on re-projection.
                (value.get("code").is_none() && value.get("decisionBy").is_none())
                    .then(|| value.get("message"))
                    .flatten()
            })
            .and_then(Value::as_str)
            .filter(|message| !message.trim().is_empty());
        let mut output = json!({
            "status": "rejected",
            "code": "command.approval_rejected",
            "decisionBy": "user",
            "executionAttempted": false,
            "retryable": false,
            "retryPolicy": if feedback.is_some() {
                "follow_user_feedback_without_repeating_same_call"
            } else {
                "new_explicit_user_instruction_required"
            },
            "message": if feedback.is_some() {
                "The user rejected this command approval. The command was not executed. Follow the user's feedback, but do not repeat the same or an equivalent command without a new explicit user instruction. This does not mean the command tool is unavailable."
            } else {
                "The user rejected this command approval. The command was not executed. Do not retry the same or an equivalent command without a new explicit user instruction. This does not mean the command tool is unavailable."
            },
        });
        if let Some(feedback) = feedback {
            output["userFeedback"] = json!(feedback);
        }
        return Some(output);
    }
    if let Some(execution) = value.get("execution") {
        let mut output = Map::new();
        for field in [
            "type",
            "code",
            "recovery",
            "phase",
            "executionAttempted",
            "effectsMayHaveOccurred",
            "commitMayHaveSucceeded",
            "auditError",
        ] {
            super::model_projection::insert_field(&mut output, value, field);
        }
        if let Some(execution) = project_command_execution(execution) {
            output.insert("execution".to_string(), execution);
        }
        return (!output.is_empty()).then_some(Value::Object(output));
    }
    project_command_execution(value)
}

fn project_command_execution(value: &Value) -> Option<Value> {
    let mut output = Map::new();
    for field in [
        "status",
        "sessionId",
        "startedAt",
        "latestSequence",
        "durationMs",
        "output",
        "outputTruncated",
        "exitCode",
        "stdout",
        "stderr",
        "error",
        // A Host-owned command Session may already have committed the full stdout/stderr body
        // before the ordinary ToolResult is assembled. Preserve its opaque recovery route so the
        // central 10K gate reuses that authoritative archive instead of inventing a second,
        // bounded-preview archive.
        "historyOpen",
        "continueWith",
    ] {
        super::model_projection::insert_field(&mut output, value, field);
    }
    for field in [
        "timedOut",
        "cancelled",
        "stdoutTruncated",
        "stderrTruncated",
        "stdoutPreviewTruncated",
        "stderrPreviewTruncated",
    ] {
        if value.get(field).and_then(Value::as_bool) == Some(true) {
            output.insert(field.to_string(), Value::Bool(true));
        }
    }
    for field in [
        "originalBytes",
        "capturedBytes",
        "omittedBytes",
        "truncatedAtSource",
        "stopReason",
    ] {
        super::model_projection::insert_field(&mut output, value, field);
    }
    for field in ["stdoutOmittedBytes", "stderrOmittedBytes"] {
        if value.get(field).and_then(Value::as_u64).unwrap_or(0) > 0 {
            super::model_projection::insert_field(&mut output, value, field);
        }
    }
    if let Some(policy) = value.get("policyEvaluation") {
        if let Some(policy) =
            super::model_projection::retain_object_fields(policy, &["decision", "code", "reason"])
        {
            output.insert("policy".to_string(), policy);
        }
    }
    if let Some(observation) = value
        .get("artifactObservation")
        .and_then(project_artifact_observation)
    {
        output.insert("artifacts".to_string(), observation);
    }
    if let Some(outputs) = value.get("outputs").and_then(Value::as_array) {
        let outputs = outputs
            .iter()
            .filter_map(|item| {
                super::model_projection::retain_object_fields(item, &["name", "kind", "readPath"])
            })
            .collect::<Vec<_>>();
        if !outputs.is_empty() {
            output.insert("outputs".to_string(), Value::Array(outputs));
        }
    }
    (!output.is_empty()).then_some(Value::Object(output))
}

pub(super) fn project_artifact_observation(value: &Value) -> Option<Value> {
    let mut output = Map::new();
    for field in [
        "status",
        "partial",
        "stopReasons",
        "scanned",
        "returned",
        "omitted",
        "changesTruncated",
        "changesOmitted",
    ] {
        super::model_projection::insert_field(&mut output, value, field);
    }
    if let Some(changes) = value.get("changes").and_then(Value::as_array) {
        let changes = changes
            .iter()
            .filter_map(|change| {
                let mut item = Map::new();
                for field in ["kind", "artifactKind", "path", "scope", "previousPath"] {
                    super::model_projection::insert_field(&mut item, change, field);
                }
                if let Some(validation) = change
                    .get("after")
                    .and_then(|after| after.get("validation"))
                    .and_then(|validation| {
                        super::model_projection::retain_object_fields(
                            validation,
                            &["status", "code", "message"],
                        )
                    })
                {
                    item.insert("validation".to_string(), validation);
                }
                (!item.is_empty()).then_some(Value::Object(item))
            })
            .collect::<Vec<_>>();
        if !changes.is_empty() {
            output.insert("changes".to_string(), Value::Array(changes));
        }
    }
    if let Some(expected) = value.get("expectedOutputs").and_then(Value::as_array) {
        let expected = expected
            .iter()
            .filter_map(|item| {
                super::model_projection::retain_object_fields(
                    item,
                    &["requestedPath", "outcome", "path", "scope", "artifactKind"],
                )
            })
            .collect::<Vec<_>>();
        if !expected.is_empty() {
            output.insert("expectedOutputs".to_string(), Value::Array(expected));
        }
    }
    if let Some(warnings) = value.get("warnings").and_then(Value::as_array) {
        let warnings = warnings
            .iter()
            .filter_map(|item| {
                super::model_projection::retain_object_fields(item, &["code", "path", "message"])
            })
            .collect::<Vec<_>>();
        if !warnings.is_empty() {
            output.insert("warnings".to_string(), Value::Array(warnings));
        }
    }
    (!output.is_empty()).then_some(Value::Object(output))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RunCommandArgs {
    command: String,
    cwd: Option<String>,
    reason: Option<String>,
    observe: Option<AgentCommandArtifactObservationRequest>,
    runtime_profile: Option<AgentCommandRuntimeProfile>,
    #[serde(default)]
    inputs: Vec<RunCommandModelPathInput>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RunCommandModelPathInput {
    path: String,
    mount_path: Option<String>,
}

fn command_request_from_call(
    context: &ToolExecutionContext,
    call: &AgentToolCall,
) -> AgentResult<AgentCommandRequest> {
    let args: RunCommandArgs = serde_json::from_value(call.args.clone())
        .map_err(|error| AgentError::new(format!("run_command 参数无效：{error}")))?;
    let command = sanitize_command(&args.command)?;
    if args.runtime_profile == Some(AgentCommandRuntimeProfile::Pdf) {
        return Err(AgentError::structured(
            "artifactRuntime.invalidRequest",
            "PDF Runtime 由已激活的可信内置 PDF Skill 自动绑定，不能通过 runtimeProfile 请求。",
            json!({
                "type": "commandRuntimeProfile",
                "code": "artifactRuntime.invalidRequest",
                "recovery": "removeRuntimeProfile"
            }),
        ));
    }
    let managed_pdf_profile = trusted_managed_pdf_profile(context, &command, args.runtime_profile)?;
    let cwd = if managed_pdf_profile.is_some() {
        sanitize_managed_pdf_cwd(args.cwd)?
    } else {
        sanitize_cwd(context, args.cwd)?
    };
    let reason = args
        .reason
        .or_else(|| call.reason.clone())
        .map(|reason| reason.trim().to_string())
        .filter(|reason| !reason.is_empty());
    let trusted_builder = trusted_materialized_builder_profile(context, &command, cwd.as_deref())?;
    let trusted_builder_command = trusted_builder
        .as_ref()
        .map(|_| infer_managed_artifact_builder_command(&command).map_err(builder_contract_error))
        .transpose()?
        .flatten();
    if let Some((builder, parsed)) = trusted_builder
        .as_ref()
        .zip(trusted_builder_command.as_ref())
    {
        validate_trusted_office_source_contract(builder, parsed)?;
    }
    let _trusted_editor_syntax_check = if trusted_builder
        .as_ref()
        .is_some_and(|builder| builder.purpose == TrustedManagedBuilderPurpose::EditPresentation)
    {
        let parsed = trusted_builder_command.as_ref().ok_or_else(|| {
            builder_contract_error("后端验证的 Presentation Editor 命令无法解析。".to_string())
        })?;
        if parsed.node_syntax_check {
            true
        } else if is_presentation_editor_direct_command(&command) {
            false
        } else {
            return Err(builder_contract_error(
                "Presentation Editor 只允许 `node --check <editor.mjs>` 语法校验，或标准的 `node <editor.mjs> --source <input> --output <save-as.pptx>` 执行形状。"
                    .to_string(),
            ));
        }
    } else {
        false
    };
    let host_builder_profile = trusted_builder.as_ref().map(|builder| builder.profile);
    let builder_config = derive_managed_builder_config(
        &command,
        args.runtime_profile,
        args.observe,
        host_builder_profile,
    )?;
    validate_managed_builder_output_scope(
        context,
        cwd.as_deref(),
        &builder_config.inferred_outputs,
    )?;
    let runtime_profile = builder_config.runtime_profile.or(managed_pdf_profile);
    let runtime_binding = runtime_profile
        .map(|profile| prepare_runtime_binding(context, &command, profile))
        .transpose()?;
    let input_context = AgentFileInputExecutionContext::new(
        context.attachment_library().cloned(),
        context.skill_resources_optional(),
    )
    .with_storage(context.storage_optional())
    .with_conversation_id(context.conversation_id_optional())
    .with_permissions(context.permissions());
    let workspace_root = context.workspace_root_optional()?;
    let mut input_specs = resolve_run_command_inputs(&input_context, args.inputs)?;
    if input_specs
        .iter()
        .any(|input| is_managed_office_script_reserved_mount(&input.mount_path))
    {
        return Err(AgentError::structured(
            "agent.fileInput.invalidRequest",
            "Managed Office Script 的 Host 保留挂载命名空间不能由模型声明。",
            json!({
                "type": "agentFileInput",
                "code": "agent.fileInput.invalidRequest",
                "recovery": "changeRequest"
            }),
        ));
    }
    let trusted_execution = trusted_builder
        .as_ref()
        .zip(trusted_builder_command.as_ref())
        .filter(|(_, command)| !command.node_syntax_check && !command.output_paths.is_empty());
    if let Some((builder, _)) = trusted_execution {
        input_specs.push(AgentFileInputSpec {
            mount_path: managed_office_script_mount(builder),
            source: AgentFileInputRef::Workspace {
                path: builder.workspace_script_path.clone(),
            },
        });
    }
    if managed_pdf_profile.is_some() {
        for relative in infer_managed_pdf_workspace_inputs(&command).map_err(|message| {
            AgentError::structured(
                "agent.fileInput.invalidRequest",
                message,
                json!({
                    "type": "agentFileInput",
                    "code": "agent.fileInput.invalidRequest",
                    "recovery": "changeRequest"
                }),
            )
        })? {
            // `outputs/` belongs to the private Run workspace and may have been produced by an
            // earlier managed PDF command. It is deliberately not interpreted as a workspace
            // input even when the selected workspace happens to contain a path with that name.
            if !relative.starts_with("outputs/") {
                let workspace_root = workspace_root.as_deref().ok_or_else(|| {
                    AgentError::structured(
                        "agent.fileInput.notFound",
                        format!(
                            "相对 PDF `{relative}` 没有可用的 workspace。附件、Artifact 或外部 PDF 必须通过 run_command.inputs 绑定，并在命令中使用 `$MYCOPILOT_INPUT_ROOT/<mountPath>`。"
                        ),
                        json!({
                            "type": "agentFileInput",
                            "code": "agent.fileInput.notFound",
                            "recovery": "bindInput"
                        }),
                    )
                })?;
                if !workspace_root.join(&relative).is_file() {
                    return Err(AgentError::structured(
                        "agent.fileInput.notFound",
                        format!(
                            "当前 workspace 中不存在 PDF `{relative}`。请核对准确的工作区相对路径；如果它来自附件、Artifact 或外部位置，请通过 run_command.inputs 绑定，并在命令中使用 `$MYCOPILOT_INPUT_ROOT/<mountPath>`。"
                        ),
                        json!({
                            "type": "agentFileInput",
                            "code": "agent.fileInput.notFound",
                            "recovery": "bindInput"
                        }),
                    ));
                }
                let implicit = AgentFileInputSpec {
                    mount_path: relative.clone(),
                    source: AgentFileInputRef::Workspace { path: relative },
                };
                if let Some(existing) = input_specs
                    .iter()
                    .find(|input| input.mount_path == implicit.mount_path)
                {
                    if existing != &implicit {
                        return Err(AgentError::structured(
                            "agent.fileInput.invalidRequest",
                            format!(
                                "run_command.inputs 的 mountPath `{}` 与隐式 workspace PDF 输入冲突。",
                                implicit.mount_path
                            ),
                            json!({
                                "type": "agentFileInput",
                                "code": "agent.fileInput.invalidRequest",
                                "recovery": "changeRequest"
                            }),
                        ));
                    }
                } else {
                    input_specs.push(implicit);
                }
            }
        }
    }
    let inputs = prepare_agent_file_input_bindings(
        workspace_root.as_deref(),
        context.permissions(),
        &input_context,
        &input_specs,
        Some(&context.cancellation_token()),
    )
    .map_err(|error| {
        AgentError::structured(
            error.code(),
            error.message(),
            json!({
                "type": "agentFileInput",
                "code": error.code(),
                "recovery": error.recovery()
            }),
        )
    })?;
    let managed_office_script = trusted_execution
        .map(|(builder, parsed)| {
            prepare_managed_office_script_binding(
                context,
                workspace_root.clone(),
                cwd.as_deref(),
                builder,
                parsed,
                &input_specs,
                &input_context,
            )
        })
        .transpose()?;

    Ok(AgentCommandRequest {
        id: call.id.clone(),
        command: command.clone(),
        cwd,
        timeout_ms: None,
        approval_status: AgentApprovalStatus::Required,
        risk_level: Some(classify_command_risk(&command)),
        reason,
        observe: builder_config.observe,
        inputs,
        runtime_binding: runtime_binding.map(Box::new),
        managed_office_script: managed_office_script.map(Box::new),
    })
}

fn trusted_managed_pdf_profile(
    context: &ToolExecutionContext,
    command: &str,
    explicit_profile: Option<AgentCommandRuntimeProfile>,
) -> AgentResult<Option<AgentCommandRuntimeProfile>> {
    if explicit_profile.is_some() {
        return Ok(None);
    }
    // Office Managed Builders have an existing, provenance-bound runtime contract. Activating
    // the PDF Skill in the same Run must not cause its deliberately narrower shell parser to
    // reinterpret or reject those commands before the Office binding is considered.
    if infer_managed_artifact_builder_command(command)
        .map_err(builder_contract_error)?
        .is_some_and(|builder| {
            builder.node_syntax_check
                || builder
                    .output_paths
                    .iter()
                    .any(|output| office_profile_for_output(output).is_some())
        })
    {
        return Ok(None);
    }
    let activated = context.skill_resources_optional().is_some_and(|resources| {
        resources.package_uris().iter().any(|package| {
            package.skill_id().source_id().as_str() == APPLICATION_BUNDLED_SKILL_SOURCE_ID
                && package.skill_id().local_id() == PDF_LOCAL_ID
        })
    });
    if !activated {
        return Ok(None);
    }
    let Some(kind) = infer_managed_pdf_command_kind(command).map_err(|message| {
        AgentError::structured(
            "artifactRuntime.invalidCommandShape",
            message,
            json!({
                "type": "managedPdfRuntime",
                "code": "artifactRuntime.invalidCommandShape",
                "recovery": "changeRequest"
            }),
        )
    })?
    else {
        return Ok(None);
    };
    debug_assert_eq!(kind, crate::AgentCommandRuntimeKind::Python);
    Ok(Some(AgentCommandRuntimeProfile::Pdf))
}

fn sanitize_managed_pdf_cwd(cwd: Option<String>) -> AgentResult<Option<String>> {
    if cwd
        .as_deref()
        .is_none_or(|cwd| cwd.trim().is_empty() || cwd.trim() == ".")
    {
        return Ok(None);
    }
    Err(AgentError::structured(
        "artifactRuntime.invalidRequest",
        "受管 PDF 命令由后端提供私有工作目录；请省略 run_command.cwd。",
        json!({
            "type": "managedPdfRuntime",
            "code": "artifactRuntime.invalidRequest",
            "recovery": "removeCwd"
        }),
    ))
}

fn resolve_run_command_inputs(
    context: &AgentFileInputExecutionContext,
    inputs: Vec<RunCommandModelPathInput>,
) -> AgentResult<Vec<AgentFileInputSpec>> {
    inputs
        .into_iter()
        .enumerate()
        .map(|(index, input)| {
            let source = agent_file_input_ref_from_model_path(context, &input.path)
                .map_err(AgentError::from)?;
            Ok(AgentFileInputSpec {
                mount_path: input
                    .mount_path
                    .unwrap_or_else(|| default_agent_file_input_mount_path(&source, index)),
                source,
            })
        })
        .collect()
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ManagedBuilderConfig {
    runtime_profile: Option<AgentCommandRuntimeProfile>,
    observe: Option<AgentCommandArtifactObservationRequest>,
    inferred_outputs: Vec<String>,
}

fn trusted_materialized_builder_profile(
    context: &ToolExecutionContext,
    command: &str,
    cwd: Option<&str>,
) -> AgentResult<Option<TrustedManagedBuilder>> {
    let builder =
        infer_managed_artifact_builder_command(command).map_err(builder_contract_error)?;
    let Some(builder) = builder else {
        return Ok(None);
    };
    let (Some(storage), Ok(run_id), Some(workspace_root)) = (
        context.storage_optional(),
        context.run_id(),
        context.workspace_root_optional()?,
    ) else {
        return Ok(None);
    };
    let command_cwd = match cwd {
        None => workspace_root.clone(),
        Some(cwd) if Path::new(cwd).is_absolute() => PathBuf::from(cwd),
        Some(cwd) => workspace_root.join(cwd),
    };
    let script = Path::new(&builder.script);
    let script_path = normalize_builder_script_path(if script.is_absolute() {
        script.to_path_buf()
    } else {
        command_cwd.join(script)
    })?;
    let Ok(relative_script) = script_path.strip_prefix(&workspace_root) else {
        return Ok(None);
    };
    let relative_script = relative_script
        .components()
        .filter_map(|component| match component {
            Component::Normal(part) => Some(part.to_string_lossy().into_owned()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("/");

    let results = storage
        .list_agent_tool_results_for_run(run_id, "skills_materialize_resource")
        .map_err(|message| {
            AgentError::structured(
                "managedBuilder.provenanceUnavailable",
                format!("无法读取 Managed Builder 的后端物化记录：{message}"),
                json!({
                    "type": "managedBuilder",
                    "code": "managedBuilder.provenanceUnavailable",
                    "recovery": "retry"
                }),
            )
        })?;
    let mut profiles = BTreeSet::new();
    for result in results {
        if !result.ok {
            continue;
        }
        let value = result.result.ok_or_else(|| {
            AgentError::structured(
                "managedBuilder.provenanceInvalid",
                "成功的 Skill 物化审计缺少结果，不能作为 Managed Builder 依据。",
                json!({
                    "type": "managedBuilder",
                    "code": "managedBuilder.provenanceInvalid",
                    "recovery": "restartRun"
                }),
            )
        })?;
        let receipt: AgentSkillMaterializationResult =
            serde_json::from_value(value).map_err(|error| {
                AgentError::structured(
                    "managedBuilder.provenanceInvalid",
                    format!("Skill 物化审计无法解析：{error}"),
                    json!({
                        "type": "managedBuilder",
                        "code": "managedBuilder.provenanceInvalid",
                        "recovery": "restartRun"
                    }),
                )
            })?;
        if receipt.destination != relative_script {
            continue;
        }
        if receipt.source_prefix.is_some()
            || receipt.file_count != 1
            || receipt.plan_digest.is_none()
            || !matches!(
                receipt.status,
                AgentSkillMaterializationResultStatus::Applied
                    | AgentSkillMaterializationResultStatus::AlreadyApplied
            )
        {
            return Err(AgentError::structured(
                "managedBuilder.provenanceInvalid",
                "匹配 Builder 路径的 Skill 物化审计不是成功的单文件内容寻址记录。",
                json!({
                    "type": "managedBuilder",
                    "code": "managedBuilder.provenanceInvalid",
                    "recovery": "rematerializeBuilder"
                }),
            ));
        }
        let Some((profile, purpose)) = profile_for_bundled_builder_receipt(&receipt, builder.kind)?
        else {
            continue;
        };
        profiles.insert((profile, purpose));
    }
    if profiles.len() > 1 {
        return Err(AgentError::structured(
            "managedBuilder.provenanceInvalid",
            "同一个 Builder 路径存在相互冲突的后端 Skill 物化记录。",
            json!({
                "type": "managedBuilder",
                "code": "managedBuilder.provenanceInvalid",
                "recovery": "restartRun"
            }),
        ));
    }
    let Some((profile, purpose)) = profiles.into_iter().next() else {
        return Ok(None);
    };

    let metadata = std::fs::symlink_metadata(&script_path).map_err(|error| {
        AgentError::structured(
            "managedBuilder.provenanceInvalid",
            format!("后端物化的 Managed Builder 不再可访问：{error}"),
            json!({
                "type": "managedBuilder",
                "code": "managedBuilder.provenanceInvalid",
                "recovery": "rematerializeBuilder"
            }),
        )
    })?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(AgentError::structured(
            "managedBuilder.provenanceInvalid",
            "后端物化的 Managed Builder 必须仍是非符号链接普通文件。",
            json!({
                "type": "managedBuilder",
                "code": "managedBuilder.provenanceInvalid",
                "recovery": "rematerializeBuilder"
            }),
        ));
    }
    let canonical_script = script_path
        .canonicalize()
        .map_err(|error| AgentError::new(format!("Managed Builder 路径无法规范化：{error}")))?;
    if !canonical_script.starts_with(&workspace_root) {
        return Err(AgentError::structured(
            "managedBuilder.provenanceInvalid",
            "后端物化的 Managed Builder 路径已经通过符号链接离开 workspace。",
            json!({
                "type": "managedBuilder",
                "code": "managedBuilder.provenanceInvalid",
                "recovery": "rematerializeBuilder"
            }),
        ));
    }
    Ok(Some(TrustedManagedBuilder {
        profile,
        purpose,
        workspace_script_path: relative_script,
    }))
}

fn profile_for_bundled_builder_receipt(
    receipt: &AgentSkillMaterializationResult,
    command_kind: crate::AgentCommandRuntimeKind,
) -> AgentResult<Option<(AgentCommandRuntimeProfile, TrustedManagedBuilderPurpose)>> {
    let source = SkillResourceUri::parse(&receipt.source_uri)
        .map_err(|error| AgentError::new(format!("Skill 物化审计包含无效 sourceUri：{error}")))?;
    if source.package().skill_id().source_id().as_str() != APPLICATION_BUNDLED_SKILL_SOURCE_ID {
        return Ok(None);
    }
    if receipt.source_revision != source.package().revision().as_str() {
        return Err(AgentError::structured(
            "managedBuilder.provenanceInvalid",
            "内置 Builder 物化审计的 source revision 不一致。",
            json!({
                "type": "managedBuilder",
                "code": "managedBuilder.provenanceInvalid",
                "recovery": "restartRun"
            }),
        ));
    }
    let local_id = source.package().skill_id().local_id();
    let path = source.path().as_str();
    let (profile, purpose) = match (local_id, path, command_kind) {
        (DOCUMENTS_LOCAL_ID, "templates/builder.py", crate::AgentCommandRuntimeKind::Python) => (
            AgentCommandRuntimeProfile::Documents,
            TrustedManagedBuilderPurpose::Create,
        ),
        (SPREADSHEETS_LOCAL_ID, "templates/builder.py", crate::AgentCommandRuntimeKind::Python) => {
            (
                AgentCommandRuntimeProfile::Spreadsheets,
                TrustedManagedBuilderPurpose::Create,
            )
        }
        (DOCUMENTS_LOCAL_ID, "templates/editor.py", crate::AgentCommandRuntimeKind::Python) => (
            AgentCommandRuntimeProfile::Documents,
            TrustedManagedBuilderPurpose::EditDocument,
        ),
        (SPREADSHEETS_LOCAL_ID, "templates/editor.py", crate::AgentCommandRuntimeKind::Python) => (
            AgentCommandRuntimeProfile::Spreadsheets,
            TrustedManagedBuilderPurpose::EditSpreadsheet,
        ),
        (PRESENTATIONS_LOCAL_ID, "templates/builder.mjs", crate::AgentCommandRuntimeKind::Node) => {
            (
                AgentCommandRuntimeProfile::Presentations,
                TrustedManagedBuilderPurpose::Create,
            )
        }
        (PRESENTATIONS_LOCAL_ID, "templates/editor.mjs", crate::AgentCommandRuntimeKind::Node) => (
            AgentCommandRuntimeProfile::Presentations,
            TrustedManagedBuilderPurpose::EditPresentation,
        ),
        _ => return Ok(None),
    };
    Ok(Some((profile, purpose)))
}

fn normalize_builder_script_path(path: PathBuf) -> AgentResult<PathBuf> {
    if !path.is_absolute() {
        return Err(AgentError::new(
            "Managed Builder 脚本路径规范化前必须是绝对路径。",
        ));
    }
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Prefix(prefix) => normalized.push(prefix.as_os_str()),
            Component::RootDir => normalized.push(component.as_os_str()),
            Component::CurDir => {}
            Component::Normal(part) => normalized.push(part),
            Component::ParentDir => {
                if !normalized.pop() {
                    return Err(AgentError::new(
                        "Managed Builder 脚本路径不能越过文件系统根目录。",
                    ));
                }
            }
        }
    }
    Ok(normalized)
}

fn builder_contract_error(message: String) -> AgentError {
    AgentError::structured(
        "managedBuilder.invalidOutputContract",
        message,
        json!({
            "type": "managedBuilder",
            "code": "managedBuilder.invalidOutputContract",
            "recovery": "changeRequest"
        }),
    )
}

fn derive_managed_builder_config(
    command: &str,
    explicit_profile: Option<AgentCommandRuntimeProfile>,
    explicit_observe: Option<AgentCommandArtifactObservationRequest>,
    host_builder_profile: Option<AgentCommandRuntimeProfile>,
) -> AgentResult<ManagedBuilderConfig> {
    // The inferred profile selects only a host-owned dependency family; it is not command or
    // filesystem authority. Normal command policy, write-scope checks, approval, frozen runtime
    // identity, and post-approval revalidation remain mandatory.
    let explicit_observe = explicit_observe.map(sanitize_observe).transpose()?;
    let builder =
        infer_managed_artifact_builder_command(command).map_err(builder_contract_error)?;

    let mut inferred_profiles = BTreeSet::new();
    let mut office_outputs = Vec::new();
    let mut node_syntax_check = false;
    if let Some(builder) = builder {
        node_syntax_check = builder.node_syntax_check;
        for output in builder.output_paths {
            if let Some(profile) = office_profile_for_output(&output) {
                inferred_profiles.insert(profile);
                office_outputs.push(output);
            }
        }
    }
    if inferred_profiles.len() > 1 {
        if explicit_profile.is_none() && host_builder_profile.is_none() {
            return Ok(ManagedBuilderConfig {
                runtime_profile: None,
                observe: explicit_observe,
                inferred_outputs: Vec::new(),
            });
        }
        return Err(AgentError::structured(
            "managedBuilder.ambiguousProfile",
            "同一个 Managed Builder 命令声明了不同 Office 类型的输出，后端无法绑定唯一运行配置。",
            json!({
                "type": "managedBuilder",
                "code": "managedBuilder.ambiguousProfile",
                "recovery": "splitCommand"
            }),
        ));
    }
    let inferred_profile = inferred_profiles.into_iter().next();
    if explicit_profile.is_none() && host_builder_profile.is_none() {
        return Ok(ManagedBuilderConfig {
            runtime_profile: None,
            observe: explicit_observe,
            inferred_outputs: Vec::new(),
        });
    }
    if host_builder_profile.is_some() && inferred_profile.is_none() && !node_syntax_check {
        return Err(AgentError::structured(
            "managedBuilder.invalidOutputContract",
            "后端验证的 Managed Builder 必须声明一个静态 .docx、.xlsx 或 .pptx `--output`。",
            json!({
                "type": "managedBuilder",
                "code": "managedBuilder.invalidOutputContract",
                "recovery": "changeRequest"
            }),
        ));
    }
    if explicit_profile.is_some()
        && inferred_profile.is_some()
        && explicit_profile != inferred_profile
    {
        return Err(AgentError::structured(
            "managedBuilder.profileMismatch",
            "run_command.runtimeProfile 与 Managed Builder 的 Office 输出类型不一致。",
            json!({
                "type": "managedBuilder",
                "code": "managedBuilder.profileMismatch",
                "recovery": "removeRuntimeProfile",
                "explicitProfile": explicit_profile,
                "inferredProfile": inferred_profile
            }),
        ));
    }
    if host_builder_profile.is_some()
        && inferred_profile.is_some()
        && host_builder_profile != inferred_profile
    {
        return Err(AgentError::structured(
            "managedBuilder.profileMismatch",
            "Managed Builder 的来源 Skill 与 `--output` Office 类型不一致。",
            json!({
                "type": "managedBuilder",
                "code": "managedBuilder.profileMismatch",
                "recovery": "changeOutput",
                "hostProfile": host_builder_profile,
                "inferredProfile": inferred_profile
            }),
        ));
    }
    if host_builder_profile.is_some()
        && explicit_profile.is_some()
        && host_builder_profile != explicit_profile
    {
        return Err(AgentError::structured(
            "managedBuilder.profileMismatch",
            "run_command.runtimeProfile 与后端验证的 Managed Builder 来源不一致。",
            json!({
                "type": "managedBuilder",
                "code": "managedBuilder.profileMismatch",
                "recovery": "removeRuntimeProfile",
                "hostProfile": host_builder_profile,
                "explicitProfile": explicit_profile
            }),
        ));
    }
    let runtime_profile = host_builder_profile.or(explicit_profile);
    let observe = if runtime_profile.is_some() && !node_syntax_check {
        let mut observe =
            explicit_observe.unwrap_or_else(default_office_artifact_observation_request);
        for output in &office_outputs {
            if !observe
                .expected_outputs
                .iter()
                .any(|existing| existing == output)
            {
                observe.expected_outputs.push(output.clone());
            }
        }
        Some(sanitize_observe(observe)?)
    } else {
        explicit_observe
    };

    Ok(ManagedBuilderConfig {
        runtime_profile,
        observe,
        inferred_outputs: office_outputs,
    })
}

fn default_office_artifact_observation_request() -> AgentCommandArtifactObservationRequest {
    AgentCommandArtifactObservationRequest {
        kinds: vec![AgentCommandArtifactObservationKind::Office],
        expected_outputs: Vec::new(),
        additional_roots: Vec::new(),
    }
}

fn office_profile_for_output(path: &str) -> Option<AgentCommandRuntimeProfile> {
    match Path::new(path)
        .extension()
        .and_then(|extension| extension.to_str())
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("docx") => Some(AgentCommandRuntimeProfile::Documents),
        Some("xlsx") => Some(AgentCommandRuntimeProfile::Spreadsheets),
        Some("pptx") => Some(AgentCommandRuntimeProfile::Presentations),
        _ => None,
    }
}

fn prepare_managed_office_script_binding(
    context: &ToolExecutionContext,
    workspace_root: Option<PathBuf>,
    cwd: Option<&str>,
    builder: &TrustedManagedBuilder,
    command: &crate::command::ManagedArtifactBuilderCommand,
    input_specs: &[AgentFileInputSpec],
    input_context: &AgentFileInputExecutionContext,
) -> AgentResult<crate::office::OfficeManagedScriptBinding> {
    if command.output_paths.len() != 1 {
        return Err(builder_contract_error(
            "后端验证的 Managed Office Script 必须声明且只声明一个静态 --output。".to_string(),
        ));
    }
    validate_trusted_office_source_contract(builder, command)?;
    let source_mount_path = match builder.purpose {
        TrustedManagedBuilderPurpose::Create => None,
        TrustedManagedBuilderPurpose::EditDocument
        | TrustedManagedBuilderPurpose::EditSpreadsheet
        | TrustedManagedBuilderPurpose::EditPresentation => command.source_paths.first().cloned(),
    };
    if let Some(source) = source_mount_path.as_deref() {
        let source_count = input_specs
            .iter()
            .filter(|input| input.mount_path == source)
            .count();
        if source_count != 1 {
            return Err(builder_contract_error(
                "Managed Office Script 的 --source 必须精确匹配一个冻结的 run_command.inputs mountPath。"
                    .to_string(),
            ));
        }
    }
    let document_kind = match builder.profile {
        AgentCommandRuntimeProfile::Documents => crate::office::OfficeDocumentKind::Document,
        AgentCommandRuntimeProfile::Spreadsheets => crate::office::OfficeDocumentKind::Spreadsheet,
        AgentCommandRuntimeProfile::Presentations => {
            crate::office::OfficeDocumentKind::Presentation
        }
        AgentCommandRuntimeProfile::Pdf => {
            return Err(builder_contract_error(
                "PDF Runtime 不能创建 Managed Office Script 事务。".to_string(),
            ))
        }
    };
    if builder.purpose.is_editor() {
        let source = source_mount_path
            .as_deref()
            .expect("editor source checked above");
        if !document_kind.accepts_path(Path::new(source)) {
            return Err(builder_contract_error(
                "Managed Office Editor 的 --source 类型必须与来源 Skill 一致。".to_string(),
            ));
        }
    }
    let requested_output = &command.output_paths[0];
    if source_mount_path.as_deref() == Some(requested_output.as_str()) {
        return Err(builder_contract_error(
            "Managed Office Editor 只允许另存为，--output 必须不同于 --source。".to_string(),
        ));
    }
    let destination = managed_office_destination_logical_path(cwd, requested_output)
        .map_err(builder_contract_error)?;
    let office_context = crate::office::OfficeExecutionContext::new(
        workspace_root.clone(),
        context.permissions(),
        context.attachment_library().cloned(),
    );
    let binding = crate::office::prepare_managed_script_binding(
        &office_context,
        document_kind,
        builder.purpose.office_purpose(),
        managed_office_script_mount(builder),
        source_mount_path.clone(),
        &destination,
    )
    .map_err(|error| {
        AgentError::structured(
            error.code().stable_name(),
            error.message(),
            json!({
                "type": "managedOfficeScript",
                "code": error.code().stable_name(),
                "recovery": error.recovery().stable_name()
            }),
        )
    })?;
    if let Some(source_mount) = source_mount_path.as_deref() {
        let source = input_specs
            .iter()
            .find(|input| input.mount_path == source_mount)
            .expect("the exact Editor source input was counted above");
        let physical_source = resolve_verified_agent_file_input_path(
            workspace_root.as_deref(),
            context.permissions(),
            input_context,
            &source.source,
        )
        .map_err(|error| {
            builder_contract_error(format!(
                "Managed Office Editor 无法验证 --source 的真实文件身份：{}",
                error.message()
            ))
        })?;
        if physical_source.as_deref() == Some(Path::new(&binding.destination.normalized_path)) {
            return Err(builder_contract_error(
                "Managed Office Editor 只允许另存为；--output 不能通过不同拼写指向真实 --source 文件。"
                    .to_string(),
            ));
        }
    }
    Ok(binding)
}

fn managed_office_destination_logical_path(
    cwd: Option<&str>,
    requested_output: &str,
) -> Result<String, String> {
    let expanded = expand_system_path(requested_output)?;
    let output = expanded.unwrap_or_else(|| PathBuf::from(requested_output));
    let logical = if output.is_absolute() {
        output
    } else {
        match cwd {
            Some(cwd) => Path::new(cwd).join(output),
            None => output,
        }
    };
    Ok(logical.to_string_lossy().into_owned())
}

fn validate_trusted_office_source_contract(
    builder: &TrustedManagedBuilder,
    command: &crate::command::ManagedArtifactBuilderCommand,
) -> AgentResult<()> {
    if command.node_syntax_check {
        return Ok(());
    }
    match builder.purpose {
        TrustedManagedBuilderPurpose::Create if !command.source_paths.is_empty() => {
            Err(builder_contract_error(
                "Managed Office Builder 不允许声明 --source；编辑现有文件必须使用对应的固定 Editor。"
                    .to_string(),
            ))
        }
        TrustedManagedBuilderPurpose::EditDocument
        | TrustedManagedBuilderPurpose::EditSpreadsheet
        | TrustedManagedBuilderPurpose::EditPresentation
            if command.source_paths.len() != 1 =>
        {
            Err(builder_contract_error(
                "Managed Office Editor 必须声明且只声明一个静态 --source。".to_string(),
            ))
        }
        _ => Ok(()),
    }
}

fn validate_managed_builder_output_scope(
    context: &ToolExecutionContext,
    cwd: Option<&str>,
    outputs: &[String],
) -> AgentResult<()> {
    let workspace_root = context.workspace_root_optional()?;
    let command_cwd = match (cwd, workspace_root.as_ref()) {
        (None, Some(root)) => root.clone(),
        (None, None) => PathBuf::from("/"),
        (Some(cwd), _) if Path::new(cwd).is_absolute() => PathBuf::from(cwd),
        (Some(cwd), Some(root)) => root.join(cwd),
        (Some(_), None) => {
            return Err(AgentError::structured(
                "managedBuilder.outputOutsideWriteScope",
                "没有 workspace 时，Managed Builder 相对 cwd 无法解析。",
                json!({
                    "type": "managedBuilder",
                    "code": "managedBuilder.outputOutsideWriteScope",
                    "recovery": "changePermissionsOrOutput"
                }),
            ))
        }
    };
    validate_managed_artifact_builder_output_scope(
        workspace_root.as_deref(),
        &command_cwd,
        outputs,
        context.permissions().write,
    )
    .map_err(|message| {
        AgentError::structured(
            "managedBuilder.outputOutsideWriteScope",
            message,
            json!({
                "type": "managedBuilder",
                "code": "managedBuilder.outputOutsideWriteScope",
                "recovery": "changePermissionsOrOutput"
            }),
        )
    })
}

fn prepare_runtime_binding(
    context: &ToolExecutionContext,
    command: &str,
    profile: AgentCommandRuntimeProfile,
) -> AgentResult<crate::AgentCommandRuntimeBinding> {
    let kind = if profile == AgentCommandRuntimeProfile::Pdf {
        infer_managed_pdf_command_kind(command).and_then(|kind| {
            kind.ok_or_else(|| "命令不属于受管 PDF Runtime 支持的调用。".to_string())
        })
    } else {
        infer_managed_artifact_command_kind(command)
    }
    .map_err(|message| {
        AgentError::structured(
            "artifactRuntime.invalidCommandShape",
            message,
            json!({
                "type": "commandRuntimeProfile",
                "code": "artifactRuntime.invalidCommandShape",
                "recovery": "changeRequest",
                "profile": profile
            }),
        )
    })?;
    let resolver = context.command_runtime_profile_resolver()?;
    let binding = resolver.resolve_profile(profile, kind).map_err(|error| {
        AgentError::structured(
            error.code(),
            error.message(),
            json!({
                "type": "commandRuntimeProfile",
                "code": error.code(),
                "recovery": error.recovery(),
                "profile": profile,
                "kind": kind
            }),
        )
    })?;
    validate_command_runtime_binding(&binding).map_err(|error| {
        AgentError::structured(
            error.code(),
            error.message(),
            json!({
                "type": "commandRuntimeProfile",
                "code": error.code(),
                "recovery": error.recovery(),
                "profile": profile,
                "kind": kind
            }),
        )
    })?;
    Ok(binding)
}

/// Validates the original model-visible ToolCall arguments against the trusted frozen command.
///
/// Startup receipt reconciliation cannot call `command_request_from_call` because the original
/// workspace/permission context is intentionally not reconstructed. This comparison therefore
/// applies only the context-free normalization performed by that function. A missing `reason` is
/// accepted because it may have come from `AgentToolCall.reason`; every argument actually present
/// in the operation is strictly parsed, normalized, and bound to the frozen request.
pub(crate) fn validate_frozen_command_trace_args(
    frozen: &AgentCommandRequest,
    operation: &Value,
) -> Result<(), String> {
    let args: FrozenRunCommandArgs = serde_json::from_value(operation.clone())
        .map_err(|error| format!("run_command frozen ToolCall arguments are invalid: {error}"))?;
    let command = sanitize_command(&args.command)
        .map_err(|_| "run_command frozen ToolCall command is invalid".to_string())?;
    let cwd = normalize_trace_cwd(args.cwd)
        .map_err(|_| "run_command frozen ToolCall cwd is invalid".to_string())?;
    let reason_was_present = args.reason.is_some();
    let reason = args
        .reason
        .map(|reason| reason.trim().to_string())
        .filter(|reason| !reason.is_empty());
    let explicit_observe = args
        .observe
        .clone()
        .map(sanitize_observe)
        .transpose()
        .map_err(|_| "run_command frozen ToolCall observe hint is invalid".to_string())?;
    let mut frozen_inputs = Vec::with_capacity(frozen.inputs.len());
    let mut managed_script = None;
    let managed_script_mount = frozen
        .managed_office_script
        .as_deref()
        .map(|binding| binding.script_mount_path.as_str());
    for binding in &frozen.inputs {
        if managed_script_mount == Some(binding.mount_path.as_str()) {
            if managed_script.replace(binding).is_some() {
                return Err(
                    "run_command frozen command contains multiple Host-reserved Managed Office scripts"
                        .to_string(),
                );
            }
            continue;
        }
        if is_managed_office_script_reserved_mount(&binding.mount_path) {
            return Err(
                "run_command frozen command contains an invalid Host-reserved Managed Office input"
                    .to_string(),
            );
        }
        frozen_inputs.push(AgentFileInputSpec {
            mount_path: binding.mount_path.clone(),
            source: binding.source.clone(),
        });
    }
    if let Some(transaction) = frozen.managed_office_script.as_deref() {
        let Some(script) = managed_script else {
            return Err(
                "run_command frozen Managed Office transaction has no frozen script".to_string(),
            );
        };
        let Some(runtime) = frozen.runtime_binding.as_deref() else {
            return Err(
                "run_command frozen Managed Office script has no managed runtime identity"
                    .to_string(),
            );
        };
        let expected_profile = match transaction.document_kind {
            crate::office::OfficeDocumentKind::Document => AgentCommandRuntimeProfile::Documents,
            crate::office::OfficeDocumentKind::Spreadsheet => {
                AgentCommandRuntimeProfile::Spreadsheets
            }
            crate::office::OfficeDocumentKind::Presentation => {
                AgentCommandRuntimeProfile::Presentations
            }
        };
        let parsed = infer_managed_artifact_builder_command(&command)
            .map_err(|_| "run_command frozen Managed Office command is invalid".to_string())?
            .ok_or_else(|| "run_command frozen Managed Office command is missing".to_string())?;
        let AgentFileInputRef::Workspace {
            path: script_workspace_path,
        } = &script.source
        else {
            return Err(
                "run_command frozen Managed Office script is not workspace-owned".to_string(),
            );
        };
        let (expected_purpose, expected_script_mount, expected_source) =
            frozen_managed_office_contract(runtime.profile, &parsed, script_workspace_path)
                .ok_or_else(|| {
                "run_command frozen Managed Office command has an invalid purpose or source contract"
                    .to_string()
            })?;
        let expected_destination = managed_office_destination_logical_path(
            cwd.as_deref(),
            parsed.output_paths.first().ok_or_else(|| {
                "run_command frozen Managed Office command has no output".to_string()
            })?,
        )
        .map_err(|_| "run_command frozen Managed Office destination is invalid".to_string())?;
        if transaction.schema_version != crate::office::OFFICE_MANAGED_SCRIPT_BINDING_SCHEMA_VERSION
            || runtime.profile != expected_profile
            || parsed.kind != runtime.kind
            || parsed.node_syntax_check
            || parsed.output_paths.len() != 1
            || transaction.purpose != expected_purpose
            || transaction.script_mount_path != expected_script_mount
            || transaction.source_mount_path.as_deref() != expected_source
            || transaction.destination.logical_path != expected_destination
            || (transaction.purpose
                == crate::office::OfficeManagedScriptPurpose::EditPresentationPlan
                && !is_presentation_editor_direct_command(&command))
        {
            return Err(
                "run_command frozen Managed Office script has an invalid Host identity".to_string(),
            );
        }
    } else if managed_script.is_some() {
        return Err(
            "run_command frozen command contains an unbound Host-reserved Office script"
                .to_string(),
        );
    }
    let frozen_pdf = frozen
        .runtime_binding
        .as_deref()
        .is_some_and(|binding| binding.profile == AgentCommandRuntimeProfile::Pdf);
    let inputs =
        normalize_frozen_run_command_inputs(&command, args.inputs, &frozen_inputs, frozen_pdf)?;
    let mut expected_observe = explicit_observe.clone();
    let runtime_matches = match frozen.runtime_binding.as_deref() {
        Some(binding) if binding.profile == AgentCommandRuntimeProfile::Pdf => {
            expected_observe = explicit_observe.clone();
            validate_command_runtime_binding(binding).is_ok()
                && args.runtime_profile.is_none()
                && frozen.observe == explicit_observe
                && infer_managed_pdf_command_kind(&command).ok().flatten() == Some(binding.kind)
        }
        Some(binding) => {
            let derived = derive_managed_builder_config(
                &command,
                args.runtime_profile,
                args.observe.clone(),
                args.runtime_profile.is_none().then_some(binding.profile),
            )
            .map_err(|_| {
                "run_command frozen ToolCall Managed Builder config is invalid".to_string()
            })?;
            expected_observe = derived.observe;
            validate_command_runtime_binding(binding).is_ok()
                && derived.runtime_profile == Some(binding.profile)
                && infer_managed_artifact_command_kind(&command).ok() == Some(binding.kind)
        }
        None => args.runtime_profile.is_none(),
    };

    if command != frozen.command
        || cwd != frozen.cwd
        || expected_observe != frozen.observe
        || inputs != frozen_inputs
        || !runtime_matches
        || (reason_was_present && reason != frozen.reason)
    {
        return Err(
            "run_command frozen ToolCall arguments differ from the prepared command".to_string(),
        );
    }
    Ok(())
}

fn is_presentation_editor_reserved_mount(mount_path: &str) -> bool {
    mount_path
        .strip_prefix(PRESENTATION_EDITOR_RESERVED_MOUNT_PREFIX)
        .is_some_and(|suffix| suffix.is_empty() || suffix.starts_with('/'))
}

fn frozen_managed_office_contract<'a>(
    profile: AgentCommandRuntimeProfile,
    parsed: &'a crate::command::ManagedArtifactBuilderCommand,
    script_workspace_path: &str,
) -> Option<(
    crate::office::OfficeManagedScriptPurpose,
    String,
    Option<&'a str>,
)> {
    use crate::office::OfficeManagedScriptPurpose;

    let source = match parsed.source_paths.as_slice() {
        [] => None,
        [source] => Some(source.as_str()),
        _ => return None,
    };
    let (purpose, profile_name) = match (profile, source) {
        (AgentCommandRuntimeProfile::Documents, None) => {
            (OfficeManagedScriptPurpose::Create, "documents")
        }
        (AgentCommandRuntimeProfile::Documents, Some(_)) => {
            (OfficeManagedScriptPurpose::Edit, "documents")
        }
        (AgentCommandRuntimeProfile::Spreadsheets, None) => {
            (OfficeManagedScriptPurpose::Create, "spreadsheets")
        }
        (AgentCommandRuntimeProfile::Spreadsheets, Some(_)) => {
            (OfficeManagedScriptPurpose::Edit, "spreadsheets")
        }
        (AgentCommandRuntimeProfile::Presentations, None) => {
            (OfficeManagedScriptPurpose::Create, "presentations")
        }
        (AgentCommandRuntimeProfile::Presentations, Some(_)) => {
            return Some((
                OfficeManagedScriptPurpose::EditPresentationPlan,
                PRESENTATION_EDITOR_SCRIPT_MOUNT_PATH.to_string(),
                source,
            ));
        }
        (AgentCommandRuntimeProfile::Pdf, _) => return None,
    };
    let script_name = Path::new(script_workspace_path)
        .file_name()
        .and_then(|name| name.to_str())?;
    Some((
        purpose,
        format!("{MANAGED_OFFICE_SCRIPT_RESERVED_MOUNT_PREFIX}/{profile_name}/{script_name}"),
        source,
    ))
}

fn is_managed_office_script_reserved_mount(mount_path: &str) -> bool {
    is_presentation_editor_reserved_mount(mount_path)
        || mount_path
            .strip_prefix(MANAGED_OFFICE_SCRIPT_RESERVED_MOUNT_PREFIX)
            .is_some_and(|suffix| suffix.is_empty() || suffix.starts_with('/'))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct FrozenRunCommandArgs {
    command: String,
    cwd: Option<String>,
    reason: Option<String>,
    observe: Option<AgentCommandArtifactObservationRequest>,
    runtime_profile: Option<AgentCommandRuntimeProfile>,
    #[serde(default)]
    inputs: Vec<RunCommandModelPathInput>,
}

fn normalize_frozen_run_command_inputs(
    command: &str,
    inputs: Vec<RunCommandModelPathInput>,
    frozen_inputs: &[AgentFileInputSpec],
    frozen_pdf: bool,
) -> Result<Vec<AgentFileInputSpec>, String> {
    let mut specs = inputs
        .into_iter()
        .enumerate()
        .map(|(index, input)| {
            let frozen = frozen_inputs.get(index).ok_or_else(|| {
                "run_command frozen ToolCall inputs exceed prepared inputs".to_string()
            })?;
            if !agent_file_input_ref_matches_model_path(&frozen.source, &input.path)
                .map_err(|_| "run_command frozen ToolCall input path is invalid".to_string())?
            {
                return Err(
                    "run_command frozen ToolCall input path differs from prepared input"
                        .to_string(),
                );
            }
            Ok(AgentFileInputSpec {
                mount_path: input
                    .mount_path
                    .unwrap_or_else(|| default_agent_file_input_mount_path(&frozen.source, index)),
                source: frozen.source.clone(),
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    if frozen_pdf {
        for relative in infer_managed_pdf_workspace_inputs(command)? {
            let implicit = AgentFileInputSpec {
                mount_path: relative.clone(),
                source: AgentFileInputRef::Workspace { path: relative },
            };
            if let Some(existing) = specs
                .iter()
                .find(|input| input.mount_path == implicit.mount_path)
            {
                if existing != &implicit {
                    return Err(
                        "run_command frozen ToolCall input collides with implicit PDF input"
                            .to_string(),
                    );
                }
            } else {
                specs.push(implicit);
            }
        }
    }
    if specs.len() != frozen_inputs.len() {
        return Err("run_command frozen ToolCall inputs differ from prepared inputs".to_string());
    }
    normalize_agent_file_input_specs(&specs)
        .map_err(|_| "run_command frozen ToolCall inputs are invalid".to_string())
}

fn normalize_trace_cwd(cwd: Option<String>) -> AgentResult<Option<String>> {
    let Some(cwd) = cwd else {
        return Ok(None);
    };
    let cwd = cwd.trim();
    if cwd.is_empty() || cwd == "." {
        return Ok(None);
    }
    if let Some(expanded) = expand_system_path(cwd).map_err(AgentError::new)? {
        return Ok(Some(expanded.to_string_lossy().to_string()));
    }
    if std::path::Path::new(cwd).is_absolute() {
        return Ok(Some(cwd.to_string()));
    }
    Ok(Some(
        clean_relative_path(cwd)?
            .components()
            .filter_map(|component| match component {
                std::path::Component::Normal(part) => Some(part.to_string_lossy().to_string()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("/"),
    ))
}

fn sanitize_observe(
    mut observe: AgentCommandArtifactObservationRequest,
) -> AgentResult<AgentCommandArtifactObservationRequest> {
    observe.kinds.sort_by_key(|kind| match kind {
        AgentCommandArtifactObservationKind::Office => 0,
    });
    observe.kinds.dedup();
    if observe.kinds != [AgentCommandArtifactObservationKind::Office] {
        return Err(AgentError::new(
            "run_command.observe.kinds 当前必须且只能包含 `office`。",
        ));
    }
    sanitize_observation_paths(
        &mut observe.expected_outputs,
        MAX_EXPECTED_OUTPUTS,
        "expectedOutputs",
    )?;
    sanitize_observation_paths(
        &mut observe.additional_roots,
        MAX_ADDITIONAL_ROOTS,
        "additionalRoots",
    )?;
    Ok(observe)
}

fn sanitize_observation_paths(
    paths: &mut Vec<String>,
    max_items: usize,
    field: &str,
) -> AgentResult<()> {
    if paths.len() > max_items {
        return Err(AgentError::new(format!(
            "run_command.observe.{field} 最多允许 {max_items} 项。"
        )));
    }
    let mut normalized = Vec::with_capacity(paths.len());
    for path in std::mem::take(paths) {
        let path = path.trim();
        if path.is_empty() || path.contains('\0') || path.contains('\n') || path.contains('\r') {
            return Err(AgentError::new(format!(
                "run_command.observe.{field} 包含空路径或不支持的控制字符。"
            )));
        }
        if path.chars().count() > MAX_OBSERVATION_PATH_CHARS {
            return Err(AgentError::new(format!(
                "run_command.observe.{field} 中的路径最多允许 {MAX_OBSERVATION_PATH_CHARS} 个字符。"
            )));
        }
        if !normalized.iter().any(|existing| existing == path) {
            normalized.push(path.to_string());
        }
    }
    *paths = normalized;
    Ok(())
}

fn sanitize_command(command: &str) -> AgentResult<String> {
    normalize_command_text(command).map_err(|error| {
        AgentError::new(if error.code == "command.malformed.too_long" {
            format!("run_command.command 过长，最多允许 {MAX_COMMAND_CHARS} 个字符。")
        } else {
            format!("run_command.command 无效：{}", error.reason)
        })
    })
}

fn sanitize_cwd(
    context: &ToolExecutionContext,
    cwd: Option<String>,
) -> AgentResult<Option<String>> {
    let workspace_exists = context.workspace_root_optional()?.is_some();
    let Some(cwd) = cwd else {
        return if workspace_exists {
            Ok(None)
        } else {
            Err(AgentError::new(
                "当前没有 workspace；run_command.cwd 必须指定绝对目录或系统路径别名。",
            ))
        };
    };
    let cwd = cwd.trim();
    if cwd.is_empty() || cwd == "." {
        return if workspace_exists {
            Ok(None)
        } else {
            Err(AgentError::new(
                "当前没有 workspace；run_command.cwd 不能省略或使用 `.`。",
            ))
        };
    }

    let write_permission = context.permissions().write;
    if let Some(expanded) = expand_system_path(cwd).map_err(AgentError::new)? {
        if write_permission != crate::protocol::AgentWritePermission::All {
            return Err(AgentError::new(
                "命令使用系统路径别名需要将写入范围设为“所有位置”。",
            ));
        }
        return Ok(Some(expanded.to_string_lossy().to_string()));
    }
    if std::path::Path::new(cwd).is_absolute() {
        if write_permission != crate::protocol::AgentWritePermission::All {
            return Err(AgentError::new(
                "命令在 workspace 外运行需要 write=all 权限。",
            ));
        }
        return Ok(Some(cwd.to_string()));
    }

    if !workspace_exists {
        return Err(AgentError::new(
            "没有 workspace 时，run_command.cwd 必须使用绝对目录或系统路径别名。",
        ));
    }

    Ok(Some(
        clean_relative_path(cwd)?
            .components()
            .filter_map(|component| match component {
                std::path::Component::Normal(part) => Some(part.to_string_lossy().to_string()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("/"),
    ))
}

#[cfg(test)]
mod tests;
