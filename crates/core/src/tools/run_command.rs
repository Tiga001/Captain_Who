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
            description: "Run one bounded non-interactive shell command through the host for builds, tests, queries, dependency management, or program execution. The command string may contain multiple lines; the Host normalizes line endings and evaluates every newline, pipeline, `&&`, `||`, and `;` segment before execution. A safely quoted heredoc delimiter such as `<<'PY'` is supported for ordinary commands; unquoted delimiters, here-strings, background execution, and shell-interpreter heredocs are denied. Command policy may execute the command automatically, request explicit user approval, or deny catastrophic/unsupported operations. This policy is not an OS sandbox. Prefer apply_patch for reviewable source edits. The Host owns process lifetime and its short initial yield; do not add a deadline merely to bound tool waiting or confirm startup. A command that outlives that initial yield returns status=running with a sessionId; running is not final success. For a GUI app or long-lived server, normally finish the turn after confirming startup instead of waiting for natural exit. For a build, test, or other serial command whose final result is required, call command_session once with action=wait and let the Host wait quietly; do not repeatedly poll or narrate waiting. Background output and exit update Host state but never start a new model turn. For a backend-verified Office Skill Builder materialized in this run, use one direct Python/Node command with `--output <file.docx|file.xlsx|file.pptx>`; the host binds the matching managed runtime and observes the output, so runtimeProfile and observe are not required. Trusted activated built-in Skills may also bind a managed runtime, private working directory, and output publication contract; follow the activated Skill instructions rather than guessing Host paths or runtimeProfile. inputs may bind authorized files read-only for a Host-bound managed workflow. Give each input only the path returned by another tool or supplied by the user; the host recognizes workspace, absolute/system, attachment, artifact, and revision-bound Skill paths automatically.".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "command": { "type": "string", "maxLength": MAX_COMMAND_CHARS, "description": "One bounded, non-interactive shell command. Literal newlines are allowed; CRLF/CR are normalized to LF and every newline, pipeline, `&&`, `||`, and `;` segment is evaluated by Host policy. A quoted heredoc delimiter such as `<<'PY'` is supported for ordinary commands; unquoted heredocs, here-strings, background execution, and NUL are rejected." },
                    "cwd": { "type": "string", "description": "Working directory. May be workspace-relative, absolute, or @home/@desktop/@documents/@downloads when permissions allow. Required when no workspace exists unless a trusted activated built-in Skill explicitly supplies a private working directory." },
                    "reason": { "type": "string", "description": "Why this command is needed and what result is expected." },
                    "observe": {
                        "type": "object",
                        "description": "Optional extra best-effort artifact observation hint. Managed Office Builders are observed automatically. This field does not grant command, read, or write permission. Expected outputs and additional roots are resolved relative to cwd.",
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
                                "description": "Expected Office output files observed directly. Sibling files are never enumerated through this hint; outcomes are reported without changing command success."
                            },
                            "additionalRoots": {
                                "type": "array",
                                "items": { "type": "string", "minLength": 1, "maxLength": MAX_OBSERVATION_PATH_CHARS },
                                "maxItems": MAX_ADDITIONAL_ROOTS,
                                "description": "Additional files or directories to scan for Office changes beyond the workspace. Recursively observing an external directory requires read=all."
                            }
                        },
                        "required": ["kinds"],
                        "additionalProperties": false
                    },
                    "runtimeProfile": {
                        "type": "string",
                        "enum": ["documents", "spreadsheets", "presentations"],
                        "description": "Optional compatibility selector for a custom saved .mjs or .py Office script that is not a backend-verified Skill Builder. Materialized Office Skill Builders must omit it: the host verifies their run-scoped materialization receipt, derives the profile, infers Node/Python, and freezes exact packages, runtime version, and integrity identity. Never provide package versions. This does not grant command/file permission and never falls back to PATH."
                    },
                    "inputs": {
                        "type": "array",
                        "maxItems": MAX_AGENT_FILE_INPUTS,
                        "description": "Optional read-only inputs for a Host-bound managed workflow. Pass one authorized path per file; the host freezes hash/size and revalidates it after approval. This field is unavailable for ordinary shell commands; follow the activated Skill for its private input convention.",
                        "items": {
                            "type": "object",
                            "properties": {
                                "path": {
                                    "type": "string",
                                    "minLength": 1,
                                    "description": "A workspace-relative path, absolute path, system alias, @attachments/... path, image-artifact://... or artifact://... URI, or revision-bound skill://... URI."
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
    if !args.inputs.is_empty() && runtime_binding.is_none() {
        return Err(AgentError::structured(
            "agent.fileInput.invalidRequest",
            "run_command.inputs 只适用于后端已绑定的受管 Office/PDF 命令。",
            json!({
                "type": "agentFileInput",
                "code": "agent.fileInput.invalidRequest",
                "recovery": "changeRequest"
            }),
        ));
    }
    let input_context = AgentFileInputExecutionContext::new(
        context.attachment_library().cloned(),
        context.skill_resources_optional(),
    )
    .with_storage(context.storage_optional())
    .with_conversation_id(context.conversation_id_optional());
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
mod tests {
    use super::*;
    use crate::protocol::{
        AgentApprovalStatus, AgentAttachmentLibraryContext, AgentAttachmentReference,
        AgentCommandRiskLevel, AgentCommandRuntimeBinding, AgentCommandRuntimeKind,
        AgentCommandRuntimeResolvedPackage, AgentInputAttachmentKind, AgentPermissions,
        AgentReadPermission, AgentRunContext, AgentSkillMaterializationResult,
        AgentSkillMaterializationResultStatus, AgentToolCall, AgentToolResult,
        AgentWorkspaceContext, AgentWritePermission, AGENT_COMMAND_RUNTIME_BINDING_SCHEMA_VERSION,
    };
    use crate::skills::{
        memory_resource_session_for_test, SkillId, SkillPackageUri, SkillResourceKind,
        SkillResourcePath, SkillRevision, SkillSourceId, APPLICATION_BUNDLED_SKILL_SOURCE_ID,
    };
    use crate::storage::models::{
        AgentActionAuditRecord, ChatConversationRecord, ChatMessageRecord,
    };
    use crate::storage::service::{ManagedArtifactAuthority, StorageService};
    use serde_json::json;
    use sha2::{Digest, Sha256};
    use std::sync::Arc;

    #[derive(Clone)]
    struct FakeProfileResolver {
        binding: AgentCommandRuntimeBinding,
    }

    impl crate::command::CommandRuntimeProfileResolver for FakeProfileResolver {
        fn resolve_profile(
            &self,
            profile: AgentCommandRuntimeProfile,
            kind: AgentCommandRuntimeKind,
        ) -> Result<AgentCommandRuntimeBinding, crate::command::CommandRuntimeProfileError>
        {
            assert_eq!(profile, self.binding.profile);
            assert_eq!(kind, self.binding.kind);
            Ok(self.binding.clone())
        }
    }

    fn test_binding(
        profile: AgentCommandRuntimeProfile,
        kind: AgentCommandRuntimeKind,
        packages: &[(&str, &str)],
    ) -> AgentCommandRuntimeBinding {
        let resolved_packages = packages
            .iter()
            .map(|(name, version)| AgentCommandRuntimeResolvedPackage {
                name: (*name).to_string(),
                version: (*version).to_string(),
            })
            .collect::<Vec<_>>();
        AgentCommandRuntimeBinding {
            schema_version: AGENT_COMMAND_RUNTIME_BINDING_SCHEMA_VERSION,
            profile,
            profile_revision: crate::command::runtime_profile_revision(
                profile,
                kind,
                &resolved_packages,
            ),
            provider_id: crate::artifact_runtime::ARTIFACT_RUNTIME_PROVIDER_ID.to_string(),
            bundle_version: crate::artifact_runtime::ARTIFACT_RUNTIME_BUNDLE_VERSION.to_string(),
            bundle_revision: "artifact-runtime-bundle-sha256-v1:test".to_string(),
            kind,
            runtime_version: "22.23.1".to_string(),
            runtime_fingerprint: "artifact-runtime-sha256-v1:test".to_string(),
            resolved_packages,
        }
    }

    fn with_profile_resolver(
        context: ToolExecutionContext,
        binding: AgentCommandRuntimeBinding,
    ) -> ToolExecutionContext {
        context
            .with_command_runtime_profile_resolver(Some(Arc::new(FakeProfileResolver { binding })))
    }

    fn record_materialized_builder(
        storage: &StorageService,
        run_id: &str,
        profile: AgentCommandRuntimeProfile,
        destination: &str,
    ) {
        let (local_id, template_path) = match profile {
            AgentCommandRuntimeProfile::Documents => (DOCUMENTS_LOCAL_ID, "templates/builder.py"),
            AgentCommandRuntimeProfile::Spreadsheets => {
                (SPREADSHEETS_LOCAL_ID, "templates/builder.py")
            }
            AgentCommandRuntimeProfile::Presentations => {
                (PRESENTATIONS_LOCAL_ID, "templates/builder.mjs")
            }
            AgentCommandRuntimeProfile::Pdf => {
                panic!("PDF does not use the Office Builder materialization helper")
            }
        };
        let revision = SkillRevision::parse(format!("revision-{local_id}")).unwrap();
        let source = SkillPackageUri::new(
            SkillId::parse(format!("{APPLICATION_BUNDLED_SKILL_SOURCE_ID}:{local_id}")).unwrap(),
            revision.clone(),
        )
        .resource(SkillResourcePath::parse(template_path.to_string()).unwrap());
        let result = AgentSkillMaterializationResult {
            status: AgentSkillMaterializationResultStatus::Applied,
            source_uri: source.to_string(),
            source_prefix: None,
            destination: destination.to_string(),
            source_revision: revision.to_string(),
            file_count: 1,
            byte_count: 100,
            plan_digest: Some("skill-materialization-sha256-v1:test".to_string()),
            error: None,
            message: Some("created".to_string()),
        };
        let tool_result = AgentToolResult {
            exact_archive_file: None,
            call_id: format!("materialize-{local_id}"),
            tool: "skills_materialize_resource".to_string(),
            ok: true,
            result: Some(serde_json::to_value(result).unwrap()),
            error: None,
        };
        storage
            .upsert_agent_action_audit(AgentActionAuditRecord {
                action_id: format!("materialize-{local_id}"),
                run_id: run_id.to_string(),
                conversation_id: None,
                assistant_message_id: None,
                action_type: "skill_materialization".to_string(),
                tool_name: "skills_materialize_resource".to_string(),
                decision: Some("approved".to_string()),
                status: "completed".to_string(),
                action_json: "{}".to_string(),
                patch_result_json: None,
                command_result_json: None,
                tool_result_json: Some(serde_json::to_string(&tool_result).unwrap()),
                error: None,
                created_at: 1,
                decided_at: Some(2),
                completed_at: Some(3),
                effective_permissions_json: None,
                path_scope: None,
                command_cwd_scope: None,
                blocked_reason: None,
                decision_source: Some("manual".to_string()),
            })
            .unwrap();
    }

    fn record_materialized_presentation_editor(
        storage: &StorageService,
        run_id: &str,
        source_id: &str,
        destination: &str,
    ) {
        let revision = SkillRevision::parse("revision-presentations-editor").unwrap();
        let source = SkillPackageUri::new(
            SkillId::parse(format!("{source_id}:{PRESENTATIONS_LOCAL_ID}")).unwrap(),
            revision.clone(),
        )
        .resource(SkillResourcePath::parse("templates/editor.mjs".to_string()).unwrap());
        let result = AgentSkillMaterializationResult {
            status: AgentSkillMaterializationResultStatus::Applied,
            source_uri: source.to_string(),
            source_prefix: None,
            destination: destination.to_string(),
            source_revision: revision.to_string(),
            file_count: 1,
            byte_count: 100,
            plan_digest: Some("skill-materialization-sha256-v1:editor-test".to_string()),
            error: None,
            message: Some("created".to_string()),
        };
        let tool_result = AgentToolResult {
            exact_archive_file: None,
            call_id: format!("materialize-editor-{source_id}"),
            tool: "skills_materialize_resource".to_string(),
            ok: true,
            result: Some(serde_json::to_value(result).unwrap()),
            error: None,
        };
        storage
            .upsert_agent_action_audit(AgentActionAuditRecord {
                action_id: format!("materialize-editor-{source_id}"),
                run_id: run_id.to_string(),
                conversation_id: None,
                assistant_message_id: None,
                action_type: "skill_materialization".to_string(),
                tool_name: "skills_materialize_resource".to_string(),
                decision: Some("approved".to_string()),
                status: "completed".to_string(),
                action_json: "{}".to_string(),
                patch_result_json: None,
                command_result_json: None,
                tool_result_json: Some(serde_json::to_string(&tool_result).unwrap()),
                error: None,
                created_at: 1,
                decided_at: Some(2),
                completed_at: Some(3),
                effective_permissions_json: None,
                path_scope: None,
                command_cwd_scope: None,
                blocked_reason: None,
                decision_source: Some("manual".to_string()),
            })
            .unwrap();
    }

    fn record_materialized_python_editor(
        storage: &StorageService,
        run_id: &str,
        profile: AgentCommandRuntimeProfile,
        destination: &str,
    ) {
        let (local_id, template_path) = match profile {
            AgentCommandRuntimeProfile::Documents => (DOCUMENTS_LOCAL_ID, "templates/editor.py"),
            AgentCommandRuntimeProfile::Spreadsheets => {
                (SPREADSHEETS_LOCAL_ID, "templates/editor.py")
            }
            _ => panic!("only Word and Excel use the Python Editor receipt helper"),
        };
        let revision = SkillRevision::parse(format!("revision-{local_id}-editor")).unwrap();
        let source = SkillPackageUri::new(
            SkillId::parse(format!("{APPLICATION_BUNDLED_SKILL_SOURCE_ID}:{local_id}")).unwrap(),
            revision.clone(),
        )
        .resource(SkillResourcePath::parse(template_path.to_string()).unwrap());
        let result = AgentSkillMaterializationResult {
            status: AgentSkillMaterializationResultStatus::Applied,
            source_uri: source.to_string(),
            source_prefix: None,
            destination: destination.to_string(),
            source_revision: revision.to_string(),
            file_count: 1,
            byte_count: 100,
            plan_digest: Some("skill-materialization-sha256-v1:python-editor-test".to_string()),
            error: None,
            message: Some("created".to_string()),
        };
        let tool_result = AgentToolResult {
            exact_archive_file: None,
            call_id: format!("materialize-{local_id}-editor"),
            tool: "skills_materialize_resource".to_string(),
            ok: true,
            result: Some(serde_json::to_value(result).unwrap()),
            error: None,
        };
        storage
            .upsert_agent_action_audit(AgentActionAuditRecord {
                action_id: format!("materialize-{local_id}-editor"),
                run_id: run_id.to_string(),
                conversation_id: None,
                assistant_message_id: None,
                action_type: "skill_materialization".to_string(),
                tool_name: "skills_materialize_resource".to_string(),
                decision: Some("approved".to_string()),
                status: "completed".to_string(),
                action_json: "{}".to_string(),
                patch_result_json: None,
                command_result_json: None,
                tool_result_json: Some(serde_json::to_string(&tool_result).unwrap()),
                error: None,
                created_at: 1,
                decided_at: Some(2),
                completed_at: Some(3),
                effective_permissions_json: None,
                path_scope: None,
                command_cwd_scope: None,
                blocked_reason: None,
                decision_source: Some("manual".to_string()),
            })
            .unwrap();
    }

    #[test]
    fn model_schema_exposes_profiles_without_timeout_or_runtime_authority() {
        let definition = RunCommandTool.definition();
        let properties = definition.input_schema["properties"].as_object().unwrap();
        assert!(properties.contains_key("runtimeProfile"));
        assert!(!properties.contains_key("timeoutMs"));
        assert!(!properties.contains_key("runtime"));
        assert_eq!(properties["command"]["maxLength"], json!(MAX_COMMAND_CHARS));
        let input_item = &properties["inputs"]["items"];
        assert_eq!(
            properties["runtimeProfile"]["enum"],
            json!(["documents", "spreadsheets", "presentations"])
        );
        assert_eq!(input_item["required"], json!(["path"]));
        assert!(input_item["properties"]["path"].is_object());
        let input_path_description = input_item["properties"]["path"]["description"]
            .as_str()
            .unwrap();
        assert!(input_path_description.contains("image-artifact://"));
        assert!(input_path_description.contains("artifact://"));
        assert!(input_item["properties"]["mountPath"].is_object());
        assert!(input_item["properties"].get("source").is_none());
        let serialized = serde_json::to_string(&definition.input_schema).unwrap();
        for forbidden in [
            "requiredPackages",
            "managedArtifact",
            "pptxgenjs",
            "4.0.1",
            "3.12.0",
            "MYCOPILOT_INPUT_ROOT",
            "built-in PDF",
        ] {
            assert!(
                !serialized.contains(forbidden),
                "model schema leaked backend runtime authority: {forbidden}"
            );
        }
    }

    fn pdf_skill_session(source_id: &str) -> Arc<crate::skills::SkillResourceSession> {
        let skill_id = SkillId::parse(format!("{source_id}:{PDF_LOCAL_ID}")).unwrap();
        Arc::new(
            memory_resource_session_for_test(
                skill_id,
                SkillRevision::parse("pdf-test-revision").unwrap(),
                SkillSourceId::parse(source_id).unwrap(),
                vec![(
                    "references/reading.md".to_string(),
                    SkillResourceKind::Reference,
                    b"test".to_vec(),
                )],
            )
            .unwrap(),
        )
    }

    fn pdf_and_documents_skill_session() -> Arc<crate::skills::SkillResourceSession> {
        let session = pdf_skill_session(APPLICATION_BUNDLED_SKILL_SOURCE_ID);
        let documents = memory_resource_session_for_test(
            SkillId::parse(format!(
                "{APPLICATION_BUNDLED_SKILL_SOURCE_ID}:{DOCUMENTS_LOCAL_ID}"
            ))
            .unwrap(),
            SkillRevision::parse("documents-test-revision").unwrap(),
            SkillSourceId::parse(APPLICATION_BUNDLED_SKILL_SOURCE_ID).unwrap(),
            vec![(
                "templates/builder.py".to_string(),
                SkillResourceKind::Other,
                b"test".to_vec(),
            )],
        )
        .unwrap();
        session.extend_from(&documents).unwrap();
        session
    }

    #[test]
    fn only_exact_bundled_pdf_skill_binds_the_hidden_pdf_runtime() {
        let command = "python -c 'from pypdf import PdfWriter; print(PdfWriter)'";
        let binding = test_binding(
            AgentCommandRuntimeProfile::Pdf,
            AgentCommandRuntimeKind::Python,
            &[
                ("pdfplumber", "0.11.9"),
                ("pypdf", "6.15.0"),
                ("pypdfium2", "5.12.1"),
                ("reportlab", "4.4.9"),
            ],
        );
        let trusted = with_profile_resolver(
            ToolExecutionContext::from_run_context(Some(&AgentRunContext {
                collaboration_identity: None,
                conversation_id: Some("conversation-pdf".to_string()),
                project_id: None,
                workspace: None,
                attachment_library: None,
                permissions: AgentPermissions::default(),
            }))
            .with_skill_resources(Some(pdf_skill_session(APPLICATION_BUNDLED_SKILL_SOURCE_ID))),
            binding,
        );
        let call = AgentToolCall {
            id: "pdf-command".to_string(),
            tool: "run_command".to_string(),
            args: json!({"command": command, "reason": "Create a PDF"}),
            approval_status: AgentApprovalStatus::Required,
            reason: None,
        };
        let request = command_request_from_call(&trusted, &call).unwrap();
        assert!(request.cwd.is_none());
        assert_eq!(
            request
                .runtime_binding
                .as_deref()
                .map(|binding| binding.profile),
            Some(AgentCommandRuntimeProfile::Pdf)
        );

        let untrusted = ToolExecutionContext::from_run_context(Some(&AgentRunContext {
            collaboration_identity: None,
            conversation_id: Some("conversation-pdf".to_string()),
            project_id: None,
            workspace: None,
            attachment_library: None,
            permissions: AgentPermissions::default(),
        }))
        .with_skill_resources(Some(pdf_skill_session("bundled:test")));
        assert_eq!(
            trusted_managed_pdf_profile(&untrusted, command, None).unwrap(),
            None
        );
    }

    #[test]
    fn bundled_pdf_freezes_a_discovered_workspace_path_without_model_inputs() {
        let workspace = tempfile::tempdir().unwrap();
        let filename = "AspenPolymer-Unit Operations and Reaction Models.pdf";
        let bytes = b"%PDF-1.4\nworkspace fixture\n";
        std::fs::write(workspace.path().join(filename), bytes).unwrap();
        let binding = test_binding(
            AgentCommandRuntimeProfile::Pdf,
            AgentCommandRuntimeKind::Python,
            &[
                ("pdfplumber", "0.11.9"),
                ("pypdf", "6.15.0"),
                ("pypdfium2", "5.12.1"),
                ("reportlab", "4.4.9"),
            ],
        );
        let context = with_profile_resolver(
            ToolExecutionContext::from_run_context(Some(&AgentRunContext {
                collaboration_identity: None,
                conversation_id: Some("conversation-aspen".to_string()),
                project_id: None,
                workspace: Some(AgentWorkspaceContext {
                    project_id: None,
                    display_name: Some("Aspen manual".to_string()),
                    root_path: Some(workspace.path().to_string_lossy().into_owned()),
                }),
                attachment_library: None,
                permissions: AgentPermissions {
                    read: AgentReadPermission::WorkspaceOnly,
                    write: AgentWritePermission::WorkspaceOnly,
                    ..AgentPermissions::default()
                },
            }))
            .with_skill_resources(Some(pdf_skill_session(APPLICATION_BUNDLED_SKILL_SOURCE_ID))),
            binding,
        );
        let call = AgentToolCall {
            id: "pdf-aspen-info".to_string(),
            tool: "run_command".to_string(),
            args: json!({
                "command": format!("pdfinfo \"{filename}\""),
                "reason": "Inspect the Aspen manual"
            }),
            approval_status: AgentApprovalStatus::Required,
            reason: None,
        };

        let request = command_request_from_call(&context, &call).unwrap();
        assert!(request.cwd.is_none());
        assert_eq!(request.command, format!("pdfinfo \"{filename}\""));
        assert_eq!(request.inputs.len(), 1);
        assert_eq!(request.inputs[0].mount_path, filename);
        assert_eq!(
            request.inputs[0].source,
            AgentFileInputRef::Workspace {
                path: filename.to_string()
            }
        );
        assert_eq!(request.inputs[0].size_bytes, bytes.len() as u64);
        assert_eq!(
            request.inputs[0].sha256,
            format!("{:x}", Sha256::digest(bytes))
        );
        validate_frozen_command_trace_args(&request, &call.args).unwrap();
        assert!(!serde_json::to_string(&request)
            .unwrap()
            .contains(workspace.path().to_string_lossy().as_ref()));
    }

    #[test]
    fn bundled_pdf_merges_explicit_attachment_and_workspace_inputs_deterministically() {
        let workspace = tempfile::tempdir().unwrap();
        let workspace_name = "工作区 手册.pdf";
        let workspace_bytes = b"%PDF-1.4\nworkspace fixture\n";
        std::fs::write(workspace.path().join(workspace_name), workspace_bytes).unwrap();

        let library = tempfile::tempdir().unwrap();
        let library_root = library.path().canonicalize().unwrap();
        std::fs::create_dir(library_root.join("objects")).unwrap();
        let attachment_bytes = b"%PDF-1.4\nattachment fixture\n";
        std::fs::write(library_root.join("objects/attached.pdf"), attachment_bytes).unwrap();
        let attachment_read_path = "@attachments/attachment-pdf/attached.pdf";

        let context = with_profile_resolver(
            ToolExecutionContext::from_run_context(Some(&AgentRunContext {
                collaboration_identity: None,
                conversation_id: Some("conversation-mixed-pdf-inputs".to_string()),
                project_id: None,
                workspace: Some(AgentWorkspaceContext {
                    project_id: None,
                    display_name: Some("mixed PDF inputs".to_string()),
                    root_path: Some(workspace.path().to_string_lossy().into_owned()),
                }),
                attachment_library: Some(AgentAttachmentLibraryContext {
                    root_path: Some(library_root.to_string_lossy().into_owned()),
                    conversation_id: Some("conversation-mixed-pdf-inputs".to_string()),
                    project_id: None,
                    conversation_attachments: vec![AgentAttachmentReference {
                        id: "attachment-pdf".to_string(),
                        conversation_id: "conversation-mixed-pdf-inputs".to_string(),
                        message_id: "message-mixed-pdf-inputs".to_string(),
                        project_id: None,
                        kind: AgentInputAttachmentKind::File,
                        name: "attached.pdf".to_string(),
                        mime_type: Some("application/pdf".to_string()),
                        size_bytes: attachment_bytes.len() as u64,
                        read_path: attachment_read_path.to_string(),
                        storage_rel_path: "objects/attached.pdf".to_string(),
                        created_at: 1,
                    }],
                    project_attachments: Vec::new(),
                }),
                permissions: AgentPermissions {
                    read: AgentReadPermission::WorkspaceOnly,
                    write: AgentWritePermission::WorkspaceOnly,
                    ..AgentPermissions::default()
                },
            }))
            .with_skill_resources(Some(pdf_skill_session(APPLICATION_BUNDLED_SKILL_SOURCE_ID))),
            test_binding(
                AgentCommandRuntimeProfile::Pdf,
                AgentCommandRuntimeKind::Python,
                &[
                    ("pdfplumber", "0.11.9"),
                    ("pypdf", "6.15.0"),
                    ("pypdfium2", "5.12.1"),
                    ("reportlab", "4.4.9"),
                ],
            ),
        );
        let call = AgentToolCall {
            id: "pdf-mixed-inputs".to_string(),
            tool: "run_command".to_string(),
            args: json!({
                "command": format!(
                    "pdfinfo \"{workspace_name}\"; pdfinfo \"$MYCOPILOT_INPUT_ROOT/attached.pdf\""
                ),
                "inputs": [
                    {"path": attachment_read_path, "mountPath": "attached.pdf"}
                ],
                "reason": "Compare the workspace and attached PDFs"
            }),
            approval_status: AgentApprovalStatus::Required,
            reason: None,
        };

        let request = command_request_from_call(&context, &call).unwrap();
        assert_eq!(request.inputs.len(), 2);
        assert_eq!(request.inputs[0].mount_path, "attached.pdf");
        assert_eq!(request.inputs[1].mount_path, workspace_name);
        assert_eq!(
            request.inputs[1].source,
            AgentFileInputRef::Workspace {
                path: workspace_name.to_string()
            }
        );
        assert_eq!(
            request.inputs[0].sha256,
            format!("{:x}", Sha256::digest(attachment_bytes))
        );
        assert_eq!(
            request.inputs[1].sha256,
            format!("{:x}", Sha256::digest(workspace_bytes))
        );
        validate_frozen_command_trace_args(&request, &call.args).unwrap();
        let serialized = serde_json::to_string(&request).unwrap();
        assert!(!serialized.contains(workspace.path().to_string_lossy().as_ref()));
        assert!(!serialized.contains(library_root.to_string_lossy().as_ref()));
    }

    #[test]
    fn bundled_pdf_missing_workspace_path_returns_an_inputs_recovery() {
        let workspace = tempfile::tempdir().unwrap();
        let context = with_profile_resolver(
            ToolExecutionContext::from_run_context(Some(&AgentRunContext {
                collaboration_identity: None,
                conversation_id: Some("conversation-missing-pdf".to_string()),
                project_id: None,
                workspace: Some(AgentWorkspaceContext {
                    project_id: None,
                    display_name: Some("missing PDF".to_string()),
                    root_path: Some(workspace.path().to_string_lossy().into_owned()),
                }),
                attachment_library: None,
                permissions: AgentPermissions {
                    read: AgentReadPermission::WorkspaceOnly,
                    write: AgentWritePermission::WorkspaceOnly,
                    ..AgentPermissions::default()
                },
            }))
            .with_skill_resources(Some(pdf_skill_session(APPLICATION_BUNDLED_SKILL_SOURCE_ID))),
            test_binding(
                AgentCommandRuntimeProfile::Pdf,
                AgentCommandRuntimeKind::Python,
                &[
                    ("pdfplumber", "0.11.9"),
                    ("pypdf", "6.15.0"),
                    ("pypdfium2", "5.12.1"),
                    ("reportlab", "4.4.9"),
                ],
            ),
        );
        let call = AgentToolCall {
            id: "pdf-missing-info".to_string(),
            tool: "run_command".to_string(),
            args: json!({"command": "pdfinfo \"not-here.pdf\""}),
            approval_status: AgentApprovalStatus::Required,
            reason: None,
        };

        let error = command_request_from_call(&context, &call).unwrap_err();
        assert_eq!(error.code(), Some("agent.fileInput.notFound"));
        assert!(error.to_string().contains("run_command.inputs"));
        assert!(error.to_string().contains("$MYCOPILOT_INPUT_ROOT"));
    }

    #[test]
    fn bundled_pdf_outputs_path_never_binds_a_same_named_workspace_file() {
        let workspace = tempfile::tempdir().unwrap();
        std::fs::create_dir(workspace.path().join("outputs")).unwrap();
        std::fs::write(
            workspace.path().join("outputs/draft.pdf"),
            b"workspace decoy",
        )
        .unwrap();
        let context = with_profile_resolver(
            ToolExecutionContext::from_run_context(Some(&AgentRunContext {
                collaboration_identity: None,
                conversation_id: Some("conversation-private-pdf-output".to_string()),
                project_id: None,
                workspace: Some(AgentWorkspaceContext {
                    project_id: None,
                    display_name: Some("PDF output isolation".to_string()),
                    root_path: Some(workspace.path().to_string_lossy().into_owned()),
                }),
                attachment_library: None,
                permissions: AgentPermissions {
                    read: AgentReadPermission::WorkspaceOnly,
                    write: AgentWritePermission::WorkspaceOnly,
                    ..AgentPermissions::default()
                },
            }))
            .with_skill_resources(Some(pdf_skill_session(APPLICATION_BUNDLED_SKILL_SOURCE_ID))),
            test_binding(
                AgentCommandRuntimeProfile::Pdf,
                AgentCommandRuntimeKind::Python,
                &[
                    ("pdfplumber", "0.11.9"),
                    ("pypdf", "6.15.0"),
                    ("pypdfium2", "5.12.1"),
                    ("reportlab", "4.4.9"),
                ],
            ),
        );
        let call = AgentToolCall {
            id: "pdf-private-output-info".to_string(),
            tool: "run_command".to_string(),
            args: json!({"command": "pdfinfo outputs/draft.pdf"}),
            approval_status: AgentApprovalStatus::Required,
            reason: None,
        };

        let request = command_request_from_call(&context, &call).unwrap();
        assert!(request.inputs.is_empty());
        assert_eq!(request.command, "pdfinfo outputs/draft.pdf");
        validate_frozen_command_trace_args(&request, &call.args).unwrap();
    }

    #[test]
    fn bundled_pdf_explicit_external_input_requires_and_accepts_full_read() {
        let fixture = tempfile::tempdir().unwrap();
        let workspace = fixture.path().join("workspace");
        std::fs::create_dir(&workspace).unwrap();
        let external = fixture.path().join("external manual.pdf");
        let bytes = b"%PDF-1.4\nexternal fixture\n";
        std::fs::write(&external, bytes).unwrap();
        // `tempfile` lives below `/var` on macOS, whose public spelling is a symlink to
        // `/private/var`. External input authority deliberately rejects symlink traversal, so
        // exercise the same canonical absolute path a real file picker/Host resolver provides.
        let external = external.canonicalize().unwrap();
        let binding = test_binding(
            AgentCommandRuntimeProfile::Pdf,
            AgentCommandRuntimeKind::Python,
            &[
                ("pdfplumber", "0.11.9"),
                ("pypdf", "6.15.0"),
                ("pypdfium2", "5.12.1"),
                ("reportlab", "4.4.9"),
            ],
        );
        let make_context = |read| {
            with_profile_resolver(
                ToolExecutionContext::from_run_context(Some(&AgentRunContext {
                    collaboration_identity: None,
                    conversation_id: Some("conversation-external-pdf".to_string()),
                    project_id: None,
                    workspace: Some(AgentWorkspaceContext {
                        project_id: None,
                        display_name: Some("external PDF".to_string()),
                        root_path: Some(workspace.to_string_lossy().into_owned()),
                    }),
                    attachment_library: None,
                    permissions: AgentPermissions {
                        read,
                        write: AgentWritePermission::WorkspaceOnly,
                        ..AgentPermissions::default()
                    },
                }))
                .with_skill_resources(Some(pdf_skill_session(APPLICATION_BUNDLED_SKILL_SOURCE_ID))),
                binding.clone(),
            )
        };
        let call = AgentToolCall {
            id: "pdf-external-info".to_string(),
            tool: "run_command".to_string(),
            args: json!({
                "command": "pdfinfo \"$MYCOPILOT_INPUT_ROOT/external.pdf\"",
                "inputs": [{
                    "path": external.to_string_lossy(),
                    "mountPath": "external.pdf"
                }]
            }),
            approval_status: AgentApprovalStatus::Required,
            reason: None,
        };

        let denied =
            command_request_from_call(&make_context(AgentReadPermission::WorkspaceOnly), &call)
                .unwrap_err();
        assert_eq!(denied.code(), Some("agent.fileInput.authorizationDenied"));

        let request =
            command_request_from_call(&make_context(AgentReadPermission::All), &call).unwrap();
        assert_eq!(request.inputs.len(), 1);
        assert_eq!(request.inputs[0].mount_path, "external.pdf");
        assert_eq!(
            request.inputs[0].source,
            AgentFileInputRef::External {
                path: external.to_string_lossy().into_owned()
            }
        );
        assert_eq!(request.inputs[0].size_bytes, bytes.len() as u64);
        assert_eq!(
            request.inputs[0].sha256,
            format!("{:x}", Sha256::digest(bytes))
        );
        validate_frozen_command_trace_args(&request, &call.args).unwrap();
    }

    #[test]
    fn bundled_pdf_freezes_a_conversation_artifact_without_a_workspace() {
        let fixture = tempfile::tempdir().unwrap();
        let root = fixture.path().canonicalize().unwrap();
        let storage = Arc::new(StorageService::open(&root.join("storage.sqlite")).unwrap());
        let conversation_id = "conversation-pdf-artifact";
        storage
            .save_conversation(ChatConversationRecord {
                id: conversation_id.to_string(),
                project_id: None,
                model_id: Some("test-model".to_string()),
                title: "PDF Artifact input".to_string(),
                messages: vec![ChatMessageRecord {
                    id: "assistant-pdf-artifact".to_string(),
                    role: "assistant".to_string(),
                    content: String::new(),
                    created_at: 1,
                    status: Some("pending".to_string()),
                    attachments: Vec::new(),
                    agent_run_json: None,
                    ui_state_json: None,
                }],
                created_at: 1,
                updated_at: 1,
                pinned_at: None,
                archived_at: None,
                unread_at: None,
            })
            .unwrap();
        let source = root.join("generated-manual.pdf");
        let bytes = b"%PDF-1.7\n1 0 obj\n<<>>\nendobj\n%%EOF\n";
        std::fs::write(&source, bytes).unwrap();
        let published = storage
            .publish_managed_artifact_file(
                &source,
                ManagedArtifactAuthority {
                    conversation_id,
                    run_id: "run-pdf-artifact-source",
                    call_id: "call-pdf-artifact-source",
                },
            )
            .unwrap();
        let read_path = published.read_path();
        assert!(read_path.starts_with("artifact://sha256/"));

        let context = with_profile_resolver(
            ToolExecutionContext::from_run_context(Some(&AgentRunContext {
                collaboration_identity: None,
                conversation_id: Some(conversation_id.to_string()),
                project_id: None,
                workspace: None,
                attachment_library: None,
                permissions: AgentPermissions::default(),
            }))
            .with_skill_resources(Some(pdf_skill_session(APPLICATION_BUNDLED_SKILL_SOURCE_ID))),
            test_binding(
                AgentCommandRuntimeProfile::Pdf,
                AgentCommandRuntimeKind::Python,
                &[
                    ("pdfplumber", "0.11.9"),
                    ("pypdf", "6.15.0"),
                    ("pypdfium2", "5.12.1"),
                    ("reportlab", "4.4.9"),
                ],
            ),
        )
        .with_runtime_services("run-pdf-artifact".to_string(), Some(storage));
        let call = AgentToolCall {
            id: "pdf-artifact-info".to_string(),
            tool: "run_command".to_string(),
            args: json!({
                "command": "pdfinfo \"$MYCOPILOT_INPUT_ROOT/manual.pdf\"",
                "inputs": [{
                    "path": read_path,
                    "mountPath": "manual.pdf"
                }],
                "reason": "Inspect a PDF produced earlier in this conversation"
            }),
            approval_status: AgentApprovalStatus::Required,
            reason: None,
        };

        let request = command_request_from_call(&context, &call).unwrap();
        assert!(request.cwd.is_none());
        assert_eq!(request.inputs.len(), 1);
        assert_eq!(request.inputs[0].mount_path, "manual.pdf");
        assert_eq!(request.inputs[0].size_bytes, bytes.len() as u64);
        assert_eq!(request.inputs[0].sha256, published.sha256);
        assert!(matches!(
            &request.inputs[0].source,
            AgentFileInputRef::GeneratedArtifact { uri, .. } if uri == &published.read_path()
        ));
        validate_frozen_command_trace_args(&request, &call.args).unwrap();
    }

    #[test]
    fn model_path_input_derives_a_private_mount_name_without_source_routing() {
        let resolved = resolve_run_command_inputs(
            &AgentFileInputExecutionContext::default(),
            vec![RunCommandModelPathInput {
                path: "assets/hero.png".to_string(),
                mount_path: None,
            }],
        )
        .unwrap();

        assert_eq!(
            resolved,
            vec![AgentFileInputSpec {
                mount_path: "hero.png".to_string(),
                source: crate::protocol::AgentFileInputRef::Workspace {
                    path: "assets/hero.png".to_string(),
                },
            }]
        );
    }

    #[test]
    fn builds_command_request_for_approval() {
        let call = AgentToolCall {
            id: "tool-1".to_string(),
            tool: "run_command".to_string(),
            args: json!({
                "command": " cargo test ",
                "cwd": "agent/rust",
                "reason": "verify tests"
            }),
            approval_status: AgentApprovalStatus::Required,
            reason: None,
        };
        let context = ToolExecutionContext::from_run_context(Some(&AgentRunContext {
            collaboration_identity: None,
            conversation_id: None,
            project_id: None,
            workspace: Some(AgentWorkspaceContext {
                project_id: None,
                display_name: Some("temp".to_string()),
                root_path: Some(std::env::temp_dir().to_string_lossy().to_string()),
            }),
            attachment_library: None,
            permissions: AgentPermissions {
                write: AgentWritePermission::WorkspaceOnly,
                ..Default::default()
            },
        }));
        let request = command_request_from_call(&context, &call).unwrap();

        assert_eq!(request.id, "tool-1");
        assert_eq!(request.command, " cargo test ");
        assert_eq!(request.cwd.as_deref(), Some("agent/rust"));
        assert_eq!(request.timeout_ms, None);
        assert_eq!(request.approval_status, AgentApprovalStatus::Required);
        assert_eq!(
            request.risk_level,
            Some(AgentCommandRiskLevel::WritesWorkspace)
        );
        assert_eq!(request.reason.as_deref(), Some("verify tests"));
        assert!(request.observe.is_none());
        assert!(request.runtime_binding.is_none());
    }

    #[test]
    fn omitted_timeout_means_no_hard_process_deadline() {
        let call = AgentToolCall {
            id: "tool-without-timeout".to_string(),
            tool: "run_command".to_string(),
            args: json!({ "command": "pwd" }),
            approval_status: AgentApprovalStatus::Required,
            reason: None,
        };
        let context = ToolExecutionContext::from_run_context(Some(&AgentRunContext {
            collaboration_identity: None,
            conversation_id: None,
            project_id: None,
            workspace: Some(AgentWorkspaceContext {
                project_id: None,
                display_name: Some("temp".to_string()),
                root_path: Some(std::env::temp_dir().to_string_lossy().to_string()),
            }),
            attachment_library: None,
            permissions: AgentPermissions::default(),
        }));

        let request = command_request_from_call(&context, &call).unwrap();

        assert_eq!(request.timeout_ms, None);
        validate_frozen_command_trace_args(&request, &call.args).unwrap();
    }

    #[test]
    fn live_model_arguments_reject_removed_timeout_field() {
        let call = AgentToolCall {
            id: "tool-with-live-timeout".to_string(),
            tool: "run_command".to_string(),
            args: json!({
                "command": "pwd",
                "timeoutMs": 15_000
            }),
            approval_status: AgentApprovalStatus::Required,
            reason: None,
        };
        let context = ToolExecutionContext::from_run_context(Some(&AgentRunContext {
            collaboration_identity: None,
            conversation_id: None,
            project_id: None,
            workspace: Some(AgentWorkspaceContext {
                project_id: None,
                display_name: Some("temp".to_string()),
                root_path: Some(std::env::temp_dir().to_string_lossy().to_string()),
            }),
            attachment_library: None,
            permissions: AgentPermissions::default(),
        }));

        let error = command_request_from_call(&context, &call).unwrap_err();

        assert!(error.to_string().contains("unknown field `timeoutMs`"));
    }

    #[test]
    fn resolves_and_freezes_a_model_friendly_runtime_profile() {
        let call = AgentToolCall {
            id: "tool-runtime".to_string(),
            tool: "run_command".to_string(),
            args: json!({
                "command": "node scripts/build.mjs --output outputs/report.xlsx",
                "runtimeProfile": "spreadsheets"
            }),
            approval_status: AgentApprovalStatus::Required,
            reason: None,
        };
        let context = with_profile_resolver(
            ToolExecutionContext::from_run_context(Some(&AgentRunContext {
                collaboration_identity: None,
                conversation_id: None,
                project_id: None,
                workspace: Some(AgentWorkspaceContext {
                    project_id: None,
                    display_name: Some("temp".to_string()),
                    root_path: Some(std::env::temp_dir().to_string_lossy().to_string()),
                }),
                attachment_library: None,
                permissions: AgentPermissions {
                    write: AgentWritePermission::WorkspaceOnly,
                    ..Default::default()
                },
            })),
            test_binding(
                AgentCommandRuntimeProfile::Spreadsheets,
                AgentCommandRuntimeKind::Node,
                &[("exceljs", "4.4.0")],
            ),
        );

        let request = command_request_from_call(&context, &call).unwrap();
        let frozen = serde_json::to_value(&request).unwrap();
        assert!(frozen.get("runtime").is_none());
        assert_eq!(frozen["runtimeBinding"]["profile"], "spreadsheets");
        assert_eq!(
            frozen["runtimeBinding"]["providerId"],
            crate::artifact_runtime::ARTIFACT_RUNTIME_PROVIDER_ID
        );
        assert_eq!(frozen["runtimeBinding"]["kind"], "node");
        assert_eq!(
            frozen["runtimeBinding"]["resolvedPackages"][0]["version"],
            "4.4.0"
        );
        assert_eq!(frozen["runtimeBinding"]["runtimeVersion"], "22.23.1");
        assert!(frozen["runtimeBinding"]["runtimeFingerprint"]
            .as_str()
            .unwrap()
            .starts_with("artifact-runtime-sha256-v1:"));
    }

    #[test]
    fn backend_binds_managed_builder_profile_and_observation_from_office_output() {
        struct Case {
            command: &'static str,
            script: &'static str,
            profile: AgentCommandRuntimeProfile,
            kind: AgentCommandRuntimeKind,
            packages: &'static [(&'static str, &'static str)],
        }
        let cases = [
            Case {
                command: "python scripts/build.py --output outputs/report.docx",
                script: "scripts/build.py",
                profile: AgentCommandRuntimeProfile::Documents,
                kind: AgentCommandRuntimeKind::Python,
                packages: &[("python-docx", "1.2.0")],
            },
            Case {
                command: "python scripts/build.py --output=outputs/report.xlsx",
                script: "scripts/build.py",
                profile: AgentCommandRuntimeProfile::Spreadsheets,
                kind: AgentCommandRuntimeKind::Python,
                packages: &[("openpyxl", "3.1.5")],
            },
            Case {
                command: "node scripts/build.mjs --output 'outputs/product intro.pptx'",
                script: "scripts/build.mjs",
                profile: AgentCommandRuntimeProfile::Presentations,
                kind: AgentCommandRuntimeKind::Node,
                packages: &[("pptxgenjs", "4.0.1")],
            },
        ];
        for case in cases {
            let workspace = tempfile::tempdir().unwrap();
            std::fs::create_dir_all(workspace.path().join("outputs")).unwrap();
            let script_path = workspace.path().join(case.script);
            std::fs::create_dir_all(script_path.parent().unwrap()).unwrap();
            std::fs::write(&script_path, "# managed builder\n").unwrap();
            let storage =
                Arc::new(StorageService::open(&workspace.path().join("storage.sqlite")).unwrap());
            let run_id = format!("run-auto-{:?}", case.profile);
            record_materialized_builder(&storage, &run_id, case.profile, case.script);
            let args = json!({ "command": case.command });
            let call = AgentToolCall {
                id: format!("tool-auto-{:?}", case.profile),
                tool: "run_command".to_string(),
                args: args.clone(),
                approval_status: AgentApprovalStatus::Required,
                reason: None,
            };
            let context = with_profile_resolver(
                ToolExecutionContext::from_run_context(Some(&AgentRunContext {
                    collaboration_identity: None,
                    conversation_id: None,
                    project_id: None,
                    workspace: Some(AgentWorkspaceContext {
                        project_id: None,
                        display_name: Some("managed-builder".to_string()),
                        root_path: Some(workspace.path().to_string_lossy().to_string()),
                    }),
                    attachment_library: None,
                    permissions: AgentPermissions {
                        write: AgentWritePermission::WorkspaceOnly,
                        ..Default::default()
                    },
                })),
                test_binding(case.profile, case.kind, case.packages),
            )
            .with_runtime_services(run_id, Some(storage));

            let request = command_request_from_call(&context, &call).unwrap();
            assert_eq!(
                request
                    .runtime_binding
                    .as_deref()
                    .map(|binding| binding.profile),
                Some(case.profile)
            );
            let observe = request.observe.as_ref().expect("backend observation");
            assert_eq!(observe.kinds, [AgentCommandArtifactObservationKind::Office]);
            assert_eq!(observe.expected_outputs.len(), 1);
            assert!(observe.expected_outputs[0].starts_with("outputs/"));
            validate_frozen_command_trace_args(&request, &args).unwrap();
        }
    }

    #[test]
    fn exact_bundled_builders_reject_source_and_python_editors_require_exactly_one_source() {
        let workspace = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(workspace.path().join("scripts")).unwrap();
        std::fs::create_dir_all(workspace.path().join("outputs")).unwrap();
        std::fs::write(workspace.path().join("source.xlsx"), b"PK\x03\x04source").unwrap();

        let storage =
            Arc::new(StorageService::open(&workspace.path().join("storage.sqlite")).unwrap());

        let builder = "scripts/build.py";
        std::fs::write(workspace.path().join(builder), b"# fixed builder\n").unwrap();
        let builder_run_id = "run-excel-builder-source-contract";
        record_materialized_builder(
            &storage,
            builder_run_id,
            AgentCommandRuntimeProfile::Spreadsheets,
            builder,
        );
        let builder_context = with_profile_resolver(
            ToolExecutionContext::from_run_context(Some(&AgentRunContext {
                collaboration_identity: None,
                conversation_id: None,
                project_id: None,
                workspace: Some(AgentWorkspaceContext {
                    project_id: None,
                    display_name: Some("builder source contract".to_string()),
                    root_path: Some(workspace.path().to_string_lossy().into_owned()),
                }),
                attachment_library: None,
                permissions: AgentPermissions {
                    read: AgentReadPermission::WorkspaceOnly,
                    write: AgentWritePermission::WorkspaceOnly,
                    ..Default::default()
                },
            })),
            test_binding(
                AgentCommandRuntimeProfile::Spreadsheets,
                AgentCommandRuntimeKind::Python,
                &[("openpyxl", "3.1.5")],
            ),
        )
        .with_runtime_services(builder_run_id.to_string(), Some(storage.clone()));
        let builder_call = AgentToolCall {
            id: "tool-excel-builder-with-source".to_string(),
            tool: "run_command".to_string(),
            args: json!({
                "command": format!(
                    "python {builder} --source source.xlsx --output outputs/new.xlsx"
                ),
                "inputs": [{ "path": "source.xlsx", "mountPath": "source.xlsx" }]
            }),
            approval_status: AgentApprovalStatus::Required,
            reason: None,
        };
        let error = command_request_from_call(&builder_context, &builder_call)
            .expect_err("a fixed Builder must never be repurposed as an existing-file editor");
        assert_eq!(error.code(), Some("managedBuilder.invalidOutputContract"));
        assert!(error.to_string().contains("Builder"));

        for (profile, editor, source, output) in [
            (
                AgentCommandRuntimeProfile::Documents,
                "scripts/edit_doc.py",
                "source.docx",
                "outputs/edited.docx",
            ),
            (
                AgentCommandRuntimeProfile::Spreadsheets,
                "scripts/edit_sheet.py",
                "source.xlsx",
                "outputs/edited.xlsx",
            ),
        ] {
            std::fs::write(workspace.path().join(editor), b"# fixed editor\n").unwrap();
            if !workspace.path().join(source).exists() {
                std::fs::write(workspace.path().join(source), b"PK\x03\x04source").unwrap();
            }
            let run_id = format!("run-{profile:?}-editor-source-contract");
            record_materialized_python_editor(&storage, &run_id, profile, editor);
            let context = with_profile_resolver(
                ToolExecutionContext::from_run_context(Some(&AgentRunContext {
                    collaboration_identity: None,
                    conversation_id: None,
                    project_id: None,
                    workspace: Some(AgentWorkspaceContext {
                        project_id: None,
                        display_name: Some("editor source contract".to_string()),
                        root_path: Some(workspace.path().to_string_lossy().into_owned()),
                    }),
                    attachment_library: None,
                    permissions: AgentPermissions {
                        read: AgentReadPermission::WorkspaceOnly,
                        write: AgentWritePermission::WorkspaceOnly,
                        ..Default::default()
                    },
                })),
                test_binding(
                    profile,
                    AgentCommandRuntimeKind::Python,
                    match profile {
                        AgentCommandRuntimeProfile::Documents => &[("python-docx", "1.2.0")],
                        AgentCommandRuntimeProfile::Spreadsheets => &[("openpyxl", "3.1.5")],
                        _ => unreachable!(),
                    },
                ),
            )
            .with_runtime_services(run_id, Some(storage.clone()));

            let missing_source = AgentToolCall {
                id: format!("tool-{profile:?}-editor-missing-source"),
                tool: "run_command".to_string(),
                args: json!({ "command": format!("python {editor} --output {output}") }),
                approval_status: AgentApprovalStatus::Required,
                reason: None,
            };
            let error = command_request_from_call(&context, &missing_source)
                .expect_err("a fixed Editor must require exactly one source");
            assert_eq!(error.code(), Some("managedBuilder.invalidOutputContract"));

            let valid = AgentToolCall {
                id: format!("tool-{profile:?}-editor-valid-source"),
                tool: "run_command".to_string(),
                args: json!({
                    "command": format!(
                        "python {editor} --source {source} --output {output}"
                    ),
                    "inputs": [{ "path": source, "mountPath": source }]
                }),
                approval_status: AgentApprovalStatus::Required,
                reason: None,
            };
            let request = command_request_from_call(&context, &valid)
                .expect("an exact fixed Editor with one frozen source is authorized");
            let transaction = request
                .managed_office_script
                .as_deref()
                .expect("the Editor receives a Host publication contract");
            assert_eq!(
                transaction.purpose,
                crate::office::OfficeManagedScriptPurpose::Edit
            );
            assert_eq!(transaction.source_mount_path.as_deref(), Some(source));
            assert_eq!(
                request.inputs.len(),
                2,
                "source and fixed script are frozen"
            );
            validate_frozen_command_trace_args(&request, &valid.args).unwrap();

            let logical_source = format!(
                "logical-source.{}",
                Path::new(source).extension().unwrap().to_string_lossy()
            );
            let aliased_source = AgentToolCall {
                id: format!("tool-{profile:?}-editor-physical-source-alias"),
                tool: "run_command".to_string(),
                args: json!({
                    "command": format!(
                        "python {editor} --source {logical_source} --output {source}"
                    ),
                    "inputs": [{ "path": source, "mountPath": logical_source }]
                }),
                approval_status: AgentApprovalStatus::Required,
                reason: None,
            };
            let error = command_request_from_call(&context, &aliased_source)
                .expect_err("save-as must compare the physical source and destination identities");
            assert_eq!(error.code(), Some("managedBuilder.invalidOutputContract"));

            let mut tampered_purpose = request.clone();
            tampered_purpose
                .managed_office_script
                .as_deref_mut()
                .unwrap()
                .purpose = crate::office::OfficeManagedScriptPurpose::Create;
            validate_frozen_command_trace_args(&tampered_purpose, &valid.args)
                .expect_err("restart trace must bind the hidden Editor purpose to the command");
            let mut tampered_destination = request.clone();
            tampered_destination
                .managed_office_script
                .as_deref_mut()
                .unwrap()
                .destination
                .logical_path = format!("other-{output}");
            validate_frozen_command_trace_args(&tampered_destination, &valid.args)
                .expect_err("restart trace must bind the hidden destination to --output");
        }
    }

    #[test]
    fn exact_bundled_editor_receipt_freezes_the_script_as_a_host_reserved_input() {
        let workspace = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(workspace.path().join("outputs")).unwrap();
        let editor = "scripts/edit_existing.mjs";
        std::fs::create_dir_all(workspace.path().join("scripts")).unwrap();
        let editor_bytes = b"// materialized fixed editor\n";
        std::fs::write(workspace.path().join(editor), editor_bytes).unwrap();
        let source_bytes = b"PK\x03\x04presentation fixture";
        std::fs::write(workspace.path().join("source.pptx"), source_bytes).unwrap();
        let storage =
            Arc::new(StorageService::open(&workspace.path().join("storage.sqlite")).unwrap());
        let run_id = "run-presentation-editor-provenance";
        record_materialized_presentation_editor(
            &storage,
            run_id,
            APPLICATION_BUNDLED_SKILL_SOURCE_ID,
            editor,
        );
        let args = json!({
            "command": format!(
                "node {editor} --source source.pptx --output outputs/source-edited.pptx"
            ),
            "inputs": [{ "path": "source.pptx", "mountPath": "source.pptx" }]
        });
        let call = AgentToolCall {
            id: "tool-presentation-editor-provenance".to_string(),
            tool: "run_command".to_string(),
            args: args.clone(),
            approval_status: AgentApprovalStatus::Required,
            reason: None,
        };
        let context = with_profile_resolver(
            ToolExecutionContext::from_run_context(Some(&AgentRunContext {
                collaboration_identity: None,
                conversation_id: Some("conversation-presentation-editor".to_string()),
                project_id: None,
                workspace: Some(AgentWorkspaceContext {
                    project_id: None,
                    display_name: Some("presentation editor".to_string()),
                    root_path: Some(workspace.path().to_string_lossy().into_owned()),
                }),
                attachment_library: None,
                permissions: AgentPermissions {
                    read: AgentReadPermission::WorkspaceOnly,
                    write: AgentWritePermission::WorkspaceOnly,
                    ..Default::default()
                },
            })),
            test_binding(
                AgentCommandRuntimeProfile::Presentations,
                AgentCommandRuntimeKind::Node,
                &[("pptxgenjs", "4.0.1")],
            ),
        )
        .with_runtime_services(run_id.to_string(), Some(storage));

        let request = command_request_from_call(&context, &call).unwrap();
        assert_eq!(
            request
                .runtime_binding
                .as_deref()
                .map(|binding| binding.profile),
            Some(AgentCommandRuntimeProfile::Presentations)
        );
        assert_eq!(request.inputs.len(), 2);
        let script = request
            .inputs
            .iter()
            .find(|input| input.mount_path == crate::command::PRESENTATION_EDITOR_SCRIPT_MOUNT_PATH)
            .expect("the exact bundled Editor script is a Host-reserved frozen input");
        assert_eq!(
            script.source,
            AgentFileInputRef::Workspace {
                path: editor.to_string()
            }
        );
        assert_eq!(script.size_bytes, editor_bytes.len() as u64);
        assert_eq!(script.sha256, format!("{:x}", Sha256::digest(editor_bytes)));
        let source = request
            .inputs
            .iter()
            .find(|input| input.mount_path == "source.pptx")
            .expect("source deck remains independently frozen");
        assert_eq!(source.size_bytes, source_bytes.len() as u64);
        validate_frozen_command_trace_args(&request, &args).unwrap();

        let aliased_source_args = json!({
            "command": format!(
                "node {editor} --source logical-source.pptx --output source.pptx"
            ),
            "inputs": [{ "path": "source.pptx", "mountPath": "logical-source.pptx" }]
        });
        let aliased_source_call = AgentToolCall {
            id: "tool-presentation-editor-physical-source-alias".to_string(),
            tool: "run_command".to_string(),
            args: aliased_source_args,
            approval_status: AgentApprovalStatus::Required,
            reason: None,
        };
        let error = command_request_from_call(&context, &aliased_source_call)
            .expect_err("Presentation save-as must reject a physical source/output alias");
        assert_eq!(error.code(), Some("managedBuilder.invalidOutputContract"));

        let restored: AgentCommandRequest =
            serde_json::from_value(serde_json::to_value(&request).unwrap()).unwrap();
        validate_frozen_command_trace_args(&restored, &args)
            .expect("restart reconciliation ignores only the exact Host-hidden script binding");
        assert_eq!(restored.inputs, request.inputs);

        let mut tampered_source = request.clone();
        tampered_source
            .managed_office_script
            .as_deref_mut()
            .unwrap()
            .source_mount_path = Some("other-source.pptx".to_string());
        validate_frozen_command_trace_args(&tampered_source, &args)
            .expect_err("restart trace must bind the hidden source mount to --source");
        let mut tampered_destination = request.clone();
        tampered_destination
            .managed_office_script
            .as_deref_mut()
            .unwrap()
            .destination
            .logical_path = "outputs/other.pptx".to_string();
        validate_frozen_command_trace_args(&tampered_destination, &args)
            .expect_err("restart trace must bind the hidden destination to --output");

        let hidden = request
            .inputs
            .iter()
            .find(|input| input.mount_path == PRESENTATION_EDITOR_SCRIPT_MOUNT_PATH)
            .unwrap()
            .clone();
        let mut duplicate_hidden = request.clone();
        duplicate_hidden.inputs.push(hidden.clone());
        validate_frozen_command_trace_args(&duplicate_hidden, &args)
            .expect_err("multiple hidden Editor identities must fail closed");

        let mut malformed_hidden = request.clone();
        malformed_hidden.inputs.push(crate::AgentFileInputBinding {
            mount_path: format!("{PRESENTATION_EDITOR_RESERVED_MOUNT_PREFIX}/unexpected.mjs"),
            ..hidden.clone()
        });
        validate_frozen_command_trace_args(&malformed_hidden, &args)
            .expect_err("unknown inputs beneath the Host-reserved prefix must fail closed");

        let mut external_hidden = request.clone();
        external_hidden
            .inputs
            .iter_mut()
            .find(|input| input.mount_path == PRESENTATION_EDITOR_SCRIPT_MOUNT_PATH)
            .unwrap()
            .source = AgentFileInputRef::External {
            path: "/tmp/forged-editor.mjs".to_string(),
        };
        validate_frozen_command_trace_args(&external_hidden, &args)
            .expect_err("the hidden Editor script must remain workspace-owned");

        let mut wrong_shape = request.clone();
        wrong_shape.command = format!("node {editor} --output outputs/source-edited.pptx");
        let mut wrong_shape_args = args.clone();
        wrong_shape_args["command"] = json!(wrong_shape.command);
        validate_frozen_command_trace_args(&wrong_shape, &wrong_shape_args)
            .expect_err("the hidden identity cannot survive a command-shape downgrade");

        let syntax_args = json!({
            "command": format!("node --check {editor}")
        });
        let syntax_call = AgentToolCall {
            id: "tool-presentation-editor-syntax-check".to_string(),
            tool: "run_command".to_string(),
            args: syntax_args.clone(),
            approval_status: AgentApprovalStatus::Required,
            reason: None,
        };
        let syntax_request = command_request_from_call(&context, &syntax_call)
            .expect("the exact Editor receipt must authorize its syntax-only check");
        assert_eq!(
            syntax_request
                .runtime_binding
                .as_deref()
                .map(|binding| (binding.profile, binding.kind)),
            Some((
                AgentCommandRuntimeProfile::Presentations,
                AgentCommandRuntimeKind::Node
            ))
        );
        assert!(
            syntax_request.inputs.is_empty(),
            "syntax-only checks must not receive the Host-hidden execution identity"
        );
        assert!(syntax_request.observe.is_none());
        validate_frozen_command_trace_args(&syntax_request, &syntax_args).unwrap();

        let unicode_source = "素材/南京 大学.pptx";
        std::fs::create_dir_all(workspace.path().join("素材")).unwrap();
        std::fs::write(
            workspace.path().join(unicode_source),
            b"PK\x03\x04unicode deck",
        )
        .unwrap();
        let quoted_args = json!({
            "command": format!(
                "node '{editor}' --source '南京 大学.pptx' --output 'outputs/南京 大学-编辑.pptx'"
            ),
            "inputs": [{ "path": unicode_source, "mountPath": "南京 大学.pptx" }]
        });
        let quoted_call = AgentToolCall {
            id: "tool-presentation-editor-quoted-unicode".to_string(),
            tool: "run_command".to_string(),
            args: quoted_args.clone(),
            approval_status: AgentApprovalStatus::Required,
            reason: None,
        };
        let quoted_request = command_request_from_call(&context, &quoted_call)
            .expect("quoted spaces and Unicode paths must keep the exact Editor contract");
        assert!(quoted_request
            .inputs
            .iter()
            .any(|input| input.mount_path == "南京 大学.pptx"));
        assert!(quoted_request
            .inputs
            .iter()
            .any(|input| input.mount_path == PRESENTATION_EDITOR_SCRIPT_MOUNT_PATH));
        assert_eq!(
            quoted_request
                .observe
                .as_ref()
                .and_then(|observe| observe.expected_outputs.first())
                .map(String::as_str),
            Some("outputs/南京 大学-编辑.pptx")
        );
        validate_frozen_command_trace_args(&quoted_request, &quoted_args).unwrap();

        for unsafe_output in ["/tmp/escaped.pptx", "../escaped.pptx"] {
            let unsafe_args = json!({
                "command": format!(
                    "node {editor} --source source.pptx --output '{unsafe_output}'"
                ),
                "inputs": [{ "path": "source.pptx", "mountPath": "source.pptx" }]
            });
            let unsafe_call = AgentToolCall {
                id: format!("tool-presentation-editor-unsafe-output-{unsafe_output}"),
                tool: "run_command".to_string(),
                args: unsafe_args,
                approval_status: AgentApprovalStatus::Required,
                reason: None,
            };
            let error = command_request_from_call(&context, &unsafe_call)
                .expect_err("Editor outputs must remain inside the effective write scope");
            assert_eq!(error.code(), Some("managedBuilder.outputOutsideWriteScope"));
        }

        let malformed_args = json!({
            "command": format!("node {editor} --output outputs/source-edited.pptx")
        });
        let malformed_call = AgentToolCall {
            id: "tool-presentation-editor-malformed".to_string(),
            tool: "run_command".to_string(),
            args: malformed_args,
            approval_status: AgentApprovalStatus::Required,
            reason: None,
        };
        let error = command_request_from_call(&context, &malformed_call)
            .expect_err("an Editor receipt must never downgrade a malformed command to Builder");
        assert_eq!(error.code(), Some("managedBuilder.invalidOutputContract"));
    }

    #[test]
    fn third_party_editor_receipt_cannot_unlock_the_host_editor_identity() {
        let workspace = tempfile::tempdir().unwrap();
        let editor = "scripts/edit_existing.mjs";
        std::fs::create_dir_all(workspace.path().join("scripts")).unwrap();
        std::fs::write(workspace.path().join(editor), "// spoofed editor\n").unwrap();
        std::fs::write(workspace.path().join("source.pptx"), b"PK\x03\x04fixture").unwrap();
        let storage =
            Arc::new(StorageService::open(&workspace.path().join("storage.sqlite")).unwrap());
        let run_id = "run-third-party-presentation-editor";
        record_materialized_presentation_editor(&storage, run_id, "workspace:workspace-1", editor);
        let base_args = json!({
            "command": format!(
                "node {editor} --source source.pptx --output outputs/source-edited.pptx"
            ),
            "inputs": [{ "path": "source.pptx", "mountPath": "source.pptx" }]
        });
        let context = with_profile_resolver(
            ToolExecutionContext::from_run_context(Some(&AgentRunContext {
                collaboration_identity: None,
                conversation_id: Some("conversation-third-party-editor".to_string()),
                project_id: None,
                workspace: Some(AgentWorkspaceContext {
                    project_id: None,
                    display_name: Some("third-party editor".to_string()),
                    root_path: Some(workspace.path().to_string_lossy().into_owned()),
                }),
                attachment_library: None,
                permissions: AgentPermissions {
                    read: AgentReadPermission::WorkspaceOnly,
                    write: AgentWritePermission::WorkspaceOnly,
                    ..Default::default()
                },
            })),
            test_binding(
                AgentCommandRuntimeProfile::Presentations,
                AgentCommandRuntimeKind::Node,
                &[("pptxgenjs", "4.0.1")],
            ),
        )
        .with_runtime_services(run_id.to_string(), Some(storage));
        let call = AgentToolCall {
            id: "tool-third-party-editor".to_string(),
            tool: "run_command".to_string(),
            args: base_args.clone(),
            approval_status: AgentApprovalStatus::Required,
            reason: None,
        };
        let error = command_request_from_call(&context, &call)
            .expect_err("third-party receipt cannot authorize managed inputs or the Editor");
        assert_eq!(error.code(), Some("agent.fileInput.invalidRequest"));

        let mut explicit_args = base_args;
        explicit_args["runtimeProfile"] = json!("presentations");
        let explicit_call = AgentToolCall {
            id: "tool-third-party-editor-explicit-runtime".to_string(),
            tool: "run_command".to_string(),
            args: explicit_args,
            approval_status: AgentApprovalStatus::Required,
            reason: None,
        };
        let ordinary = command_request_from_call(&context, &explicit_call)
            .expect("an explicit ordinary Office runtime remains a separate compatibility path");
        assert!(
            ordinary
                .inputs
                .iter()
                .all(|input| input.mount_path
                    != crate::command::PRESENTATION_EDITOR_SCRIPT_MOUNT_PATH)
        );

        let forged_args = json!({
            "command": format!(
                "node {editor} --source source.pptx --output outputs/source-edited.pptx"
            ),
            "runtimeProfile": "presentations",
            "inputs": [{
                "path": editor,
                "mountPath": crate::command::PRESENTATION_EDITOR_SCRIPT_MOUNT_PATH
            }]
        });
        let forged_call = AgentToolCall {
            id: "tool-forged-editor-reserved-input".to_string(),
            tool: "run_command".to_string(),
            args: forged_args,
            approval_status: AgentApprovalStatus::Required,
            reason: None,
        };
        let forged = command_request_from_call(&context, &forged_call)
            .expect_err("model arguments cannot forge the Host-reserved Editor binding");
        assert_eq!(forged.code(), Some("agent.fileInput.invalidRequest"));

        for (case, mount_path) in [
            (
                "reserved-sibling",
                format!("{PRESENTATION_EDITOR_RESERVED_MOUNT_PREFIX}/loader.mjs"),
            ),
            (
                "reserved-child",
                format!("{PRESENTATION_EDITOR_SCRIPT_MOUNT_PATH}/payload"),
            ),
        ] {
            let forged_args = json!({
                "command": format!(
                    "node {editor} --source source.pptx --output outputs/source-edited.pptx"
                ),
                "runtimeProfile": "presentations",
                "inputs": [{ "path": editor, "mountPath": mount_path }]
            });
            let forged_call = AgentToolCall {
                id: format!("tool-forged-editor-{case}"),
                tool: "run_command".to_string(),
                args: forged_args,
                approval_status: AgentApprovalStatus::Required,
                reason: None,
            };
            let forged = command_request_from_call(&context, &forged_call)
                .expect_err("the entire Presentation Editor namespace is Host-reserved");
            assert_eq!(forged.code(), Some("agent.fileInput.invalidRequest"));
        }
    }

    #[test]
    fn current_run_presentation_builder_syntax_check_uses_frozen_runtime_without_observation() {
        let workspace = tempfile::tempdir().unwrap();
        let script = "scripts/build_deck.mjs";
        let script_path = workspace.path().join(script);
        std::fs::create_dir_all(script_path.parent().unwrap()).unwrap();
        std::fs::write(&script_path, "export const deck = true;\n").unwrap();
        let storage =
            Arc::new(StorageService::open(&workspace.path().join("storage.sqlite")).unwrap());
        let run_id = "run-presentation-syntax-check";
        record_materialized_builder(
            &storage,
            run_id,
            AgentCommandRuntimeProfile::Presentations,
            script,
        );
        let args = json!({"command": format!("node --check {script}")});
        let call = AgentToolCall {
            id: "tool-presentation-syntax-check".to_string(),
            tool: "run_command".to_string(),
            args: args.clone(),
            approval_status: AgentApprovalStatus::Required,
            reason: None,
        };
        let context = with_profile_resolver(
            ToolExecutionContext::from_run_context(Some(&AgentRunContext {
                collaboration_identity: None,
                conversation_id: Some("conversation-presentation-syntax-check".to_string()),
                project_id: None,
                workspace: Some(AgentWorkspaceContext {
                    project_id: None,
                    display_name: Some("presentation syntax check".to_string()),
                    root_path: Some(workspace.path().to_string_lossy().into_owned()),
                }),
                attachment_library: None,
                permissions: AgentPermissions {
                    read: AgentReadPermission::WorkspaceOnly,
                    write: AgentWritePermission::WorkspaceOnly,
                    ..Default::default()
                },
            }))
            .with_skill_resources(Some(pdf_skill_session(APPLICATION_BUNDLED_SKILL_SOURCE_ID))),
            test_binding(
                AgentCommandRuntimeProfile::Presentations,
                AgentCommandRuntimeKind::Node,
                &[("pptxgenjs", "4.0.1")],
            ),
        )
        .with_runtime_services(run_id.to_string(), Some(storage));

        let request = command_request_from_call(&context, &call).unwrap();
        let binding = request
            .runtime_binding
            .as_deref()
            .expect("current-Run materialization binds the managed runtime");
        assert_eq!(binding.profile, AgentCommandRuntimeProfile::Presentations);
        assert_eq!(binding.kind, AgentCommandRuntimeKind::Node);
        assert!(request.observe.is_none());
        assert!(request.inputs.is_empty());
        validate_frozen_command_trace_args(&request, &args).unwrap();

        let restored: AgentCommandRequest =
            serde_json::from_value(serde_json::to_value(&request).unwrap()).unwrap();
        validate_frozen_command_trace_args(&restored, &args).unwrap();
        assert_eq!(restored.runtime_binding, request.runtime_binding);
    }

    #[test]
    fn ordinary_node_syntax_check_is_not_rebound_without_a_matching_current_run_receipt() {
        let workspace = tempfile::tempdir().unwrap();
        let script = "scripts/build_deck.mjs";
        let script_path = workspace.path().join(script);
        std::fs::create_dir_all(script_path.parent().unwrap()).unwrap();
        std::fs::write(&script_path, "export const deck = true;\n").unwrap();
        let storage =
            Arc::new(StorageService::open(&workspace.path().join("storage.sqlite")).unwrap());
        record_materialized_builder(
            &storage,
            "different-run",
            AgentCommandRuntimeProfile::Presentations,
            script,
        );
        let base = ToolExecutionContext::from_run_context(Some(&AgentRunContext {
            collaboration_identity: None,
            conversation_id: None,
            project_id: None,
            workspace: Some(AgentWorkspaceContext {
                project_id: None,
                display_name: Some("untrusted syntax check".to_string()),
                root_path: Some(workspace.path().to_string_lossy().into_owned()),
            }),
            attachment_library: None,
            permissions: AgentPermissions {
                read: AgentReadPermission::WorkspaceOnly,
                write: AgentWritePermission::WorkspaceOnly,
                ..Default::default()
            },
        }));
        let context = with_profile_resolver(
            base,
            test_binding(
                AgentCommandRuntimeProfile::Presentations,
                AgentCommandRuntimeKind::Node,
                &[("pptxgenjs", "4.0.1")],
            ),
        )
        .with_runtime_services("current-run".to_string(), Some(storage));

        let ordinary_args = json!({"command": format!("node --check {script}")});
        let ordinary_call = AgentToolCall {
            id: "tool-ordinary-syntax-check".to_string(),
            tool: "run_command".to_string(),
            args: ordinary_args.clone(),
            approval_status: AgentApprovalStatus::Required,
            reason: None,
        };
        let ordinary = command_request_from_call(&context, &ordinary_call).unwrap();
        assert!(ordinary.runtime_binding.is_none());
        assert!(ordinary.observe.is_none());
        validate_frozen_command_trace_args(&ordinary, &ordinary_args).unwrap();

        let explicit_args = json!({
            "command": format!("node --check {script}"),
            "runtimeProfile": "presentations"
        });
        let explicit_call = AgentToolCall {
            id: "tool-explicit-managed-syntax-check".to_string(),
            tool: "run_command".to_string(),
            args: explicit_args.clone(),
            approval_status: AgentApprovalStatus::Required,
            reason: None,
        };
        let explicit = command_request_from_call(&context, &explicit_call).unwrap();
        assert_eq!(
            explicit
                .runtime_binding
                .as_deref()
                .map(|binding| binding.profile),
            Some(AgentCommandRuntimeProfile::Presentations)
        );
        assert!(explicit.observe.is_none());
        validate_frozen_command_trace_args(&explicit, &explicit_args).unwrap();
    }

    #[test]
    fn activated_pdf_skill_does_not_intercept_a_provenance_bound_office_builder() {
        let workspace = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(workspace.path().join("outputs")).unwrap();
        let script = "scripts/build.py";
        let script_path = workspace.path().join(script);
        std::fs::create_dir_all(script_path.parent().unwrap()).unwrap();
        std::fs::write(&script_path, "# managed builder\n").unwrap();
        let storage =
            Arc::new(StorageService::open(&workspace.path().join("storage.sqlite")).unwrap());
        let run_id = "run-pdf-and-documents";
        record_materialized_builder(
            &storage,
            run_id,
            AgentCommandRuntimeProfile::Documents,
            script,
        );
        let command = "python scripts/build.py --output outputs/report.docx";
        let call = AgentToolCall {
            id: "tool-pdf-and-documents".to_string(),
            tool: "run_command".to_string(),
            args: json!({ "command": command }),
            approval_status: AgentApprovalStatus::Required,
            reason: None,
        };
        let context = with_profile_resolver(
            ToolExecutionContext::from_run_context(Some(&AgentRunContext {
                collaboration_identity: None,
                conversation_id: Some("conversation-pdf-and-documents".to_string()),
                project_id: None,
                workspace: Some(AgentWorkspaceContext {
                    project_id: None,
                    display_name: Some("PDF and Documents".to_string()),
                    root_path: Some(workspace.path().to_string_lossy().into_owned()),
                }),
                attachment_library: None,
                permissions: AgentPermissions {
                    write: AgentWritePermission::WorkspaceOnly,
                    ..Default::default()
                },
            }))
            .with_skill_resources(Some(pdf_and_documents_skill_session())),
            test_binding(
                AgentCommandRuntimeProfile::Documents,
                AgentCommandRuntimeKind::Python,
                &[("python-docx", "1.2.0")],
            ),
        )
        .with_runtime_services(run_id.to_string(), Some(storage));

        let request = command_request_from_call(&context, &call).unwrap();
        assert_eq!(request.command, command);
        assert_eq!(
            request
                .runtime_binding
                .as_deref()
                .map(|binding| binding.profile),
            Some(AgentCommandRuntimeProfile::Documents)
        );
        assert_eq!(
            request
                .observe
                .as_ref()
                .map(|observe| observe.kinds.as_slice()),
            Some([AgentCommandArtifactObservationKind::Office].as_slice())
        );
    }

    #[test]
    fn ordinary_saved_scripts_are_not_silently_rebound_without_backend_provenance() {
        let workspace = tempfile::tempdir().unwrap();
        std::fs::write(workspace.path().join("ordinary.py"), "print('ordinary')\n").unwrap();
        let context = with_profile_resolver(
            ToolExecutionContext::from_run_context(Some(&AgentRunContext {
                collaboration_identity: None,
                conversation_id: None,
                project_id: None,
                workspace: Some(AgentWorkspaceContext {
                    project_id: None,
                    display_name: Some("ordinary-script".to_string()),
                    root_path: Some(workspace.path().to_string_lossy().to_string()),
                }),
                attachment_library: None,
                permissions: AgentPermissions {
                    write: AgentWritePermission::WorkspaceOnly,
                    ..Default::default()
                },
            })),
            test_binding(
                AgentCommandRuntimeProfile::Documents,
                AgentCommandRuntimeKind::Python,
                &[("python-docx", "1.2.0")],
            ),
        );
        let call = AgentToolCall {
            id: "ordinary-script".to_string(),
            tool: "run_command".to_string(),
            args: json!({
                "command": "python ordinary.py --output report.docx"
            }),
            approval_status: AgentApprovalStatus::Required,
            reason: None,
        };

        let request = command_request_from_call(&context, &call).unwrap();
        assert!(request.runtime_binding.is_none());
        assert!(request.observe.is_none());
    }

    #[test]
    fn backend_rejects_conflicting_builder_profile_and_workspace_escape() {
        let workspace = tempfile::tempdir().unwrap();
        std::fs::write(workspace.path().join("builder.py"), "# managed builder\n").unwrap();
        let storage =
            Arc::new(StorageService::open(&workspace.path().join("storage.sqlite")).unwrap());
        let run_id = "run-builder-scope";
        record_materialized_builder(
            &storage,
            run_id,
            AgentCommandRuntimeProfile::Documents,
            "builder.py",
        );
        let context = with_profile_resolver(
            ToolExecutionContext::from_run_context(Some(&AgentRunContext {
                collaboration_identity: None,
                conversation_id: None,
                project_id: None,
                workspace: Some(AgentWorkspaceContext {
                    project_id: None,
                    display_name: Some("managed-builder".to_string()),
                    root_path: Some(workspace.path().to_string_lossy().to_string()),
                }),
                attachment_library: None,
                permissions: AgentPermissions {
                    write: AgentWritePermission::WorkspaceOnly,
                    ..Default::default()
                },
            })),
            test_binding(
                AgentCommandRuntimeProfile::Documents,
                AgentCommandRuntimeKind::Python,
                &[("python-docx", "1.2.0")],
            ),
        )
        .with_runtime_services(run_id.to_string(), Some(Arc::clone(&storage)));
        let conflicting = AgentToolCall {
            id: "tool-conflicting-profile".to_string(),
            tool: "run_command".to_string(),
            args: json!({
                "command": "python builder.py --output report.docx",
                "runtimeProfile": "spreadsheets"
            }),
            approval_status: AgentApprovalStatus::Required,
            reason: None,
        };
        let error = command_request_from_call(&context, &conflicting).unwrap_err();
        assert_eq!(error.code(), Some("managedBuilder.profileMismatch"));

        let escaping = AgentToolCall {
            id: "tool-escaping-output".to_string(),
            tool: "run_command".to_string(),
            args: json!({
                "command": "python builder.py --output ../outside.docx"
            }),
            approval_status: AgentApprovalStatus::Required,
            reason: None,
        };
        let error = command_request_from_call(&context, &escaping).unwrap_err();
        assert_eq!(error.code(), Some("managedBuilder.outputOutsideWriteScope"));

        let full_access_context = with_profile_resolver(
            ToolExecutionContext::from_run_context(Some(&AgentRunContext {
                collaboration_identity: None,
                conversation_id: None,
                project_id: None,
                workspace: Some(AgentWorkspaceContext {
                    project_id: None,
                    display_name: Some("managed-builder".to_string()),
                    root_path: Some(workspace.path().to_string_lossy().to_string()),
                }),
                attachment_library: None,
                permissions: AgentPermissions {
                    write: AgentWritePermission::All,
                    ..Default::default()
                },
            })),
            test_binding(
                AgentCommandRuntimeProfile::Documents,
                AgentCommandRuntimeKind::Python,
                &[("python-docx", "1.2.0")],
            ),
        )
        .with_runtime_services(run_id.to_string(), Some(storage));
        let external_output = std::env::temp_dir()
            .canonicalize()
            .unwrap()
            .join("mycopilot-managed-builder.docx");
        let external_output = external_output.to_string_lossy().into_owned();
        let external = AgentToolCall {
            id: "tool-external-output".to_string(),
            tool: "run_command".to_string(),
            args: json!({
                "command": format!("python builder.py --output {external_output}")
            }),
            approval_status: AgentApprovalStatus::Required,
            reason: None,
        };
        let request = command_request_from_call(&full_access_context, &external).unwrap();
        assert_eq!(
            request
                .observe
                .as_ref()
                .unwrap()
                .expected_outputs
                .as_slice(),
            [external_output]
        );
    }

    #[test]
    fn freezes_declared_attachment_input_without_persisting_library_paths() {
        let library = tempfile::tempdir().unwrap();
        let root = library.path().canonicalize().unwrap();
        std::fs::create_dir(root.join("objects")).unwrap();
        std::fs::write(root.join("objects/campus.png"), b"campus-image").unwrap();
        std::fs::create_dir(root.join("scripts")).unwrap();
        std::fs::write(root.join("scripts/build.py"), "# managed builder\n").unwrap();
        let storage = Arc::new(StorageService::open(&root.join("storage.sqlite")).unwrap());
        let run_id = "run-runtime-input";
        record_materialized_builder(
            &storage,
            run_id,
            AgentCommandRuntimeProfile::Documents,
            "scripts/build.py",
        );
        let read_path = "@attachments/attachment-1/campus.png";
        let call = AgentToolCall {
            id: "tool-runtime-input".to_string(),
            tool: "run_command".to_string(),
            args: json!({
                "command": "python scripts/build.py --output report.docx",
                "inputs": [{
                    "mountPath": "images/campus.png",
                    "path": read_path
                }]
            }),
            approval_status: AgentApprovalStatus::Required,
            reason: None,
        };
        let context = with_profile_resolver(
            ToolExecutionContext::from_run_context(Some(&AgentRunContext {
                collaboration_identity: None,
                conversation_id: Some("conversation-1".to_string()),
                project_id: None,
                workspace: Some(AgentWorkspaceContext {
                    project_id: None,
                    display_name: Some("attachment-input-test".to_string()),
                    root_path: Some(root.to_string_lossy().to_string()),
                }),
                attachment_library: Some(AgentAttachmentLibraryContext {
                    root_path: Some(root.to_string_lossy().to_string()),
                    conversation_id: Some("conversation-1".to_string()),
                    project_id: None,
                    conversation_attachments: vec![AgentAttachmentReference {
                        id: "attachment-1".to_string(),
                        conversation_id: "conversation-1".to_string(),
                        message_id: "message-1".to_string(),
                        project_id: None,
                        kind: AgentInputAttachmentKind::Image,
                        name: "campus.png".to_string(),
                        mime_type: Some("image/png".to_string()),
                        size_bytes: 12,
                        read_path: read_path.to_string(),
                        storage_rel_path: "objects/campus.png".to_string(),
                        created_at: 1,
                    }],
                    project_attachments: Vec::new(),
                }),
                permissions: AgentPermissions {
                    read: AgentReadPermission::WorkspaceOnly,
                    write: AgentWritePermission::WorkspaceOnly,
                    ..Default::default()
                },
            })),
            test_binding(
                AgentCommandRuntimeProfile::Documents,
                AgentCommandRuntimeKind::Python,
                &[("python-docx", "1.2.0")],
            ),
        )
        .with_runtime_services(run_id.to_string(), Some(storage));

        let request = command_request_from_call(&context, &call).unwrap();
        assert_eq!(request.inputs.len(), 2);
        let image = request
            .inputs
            .iter()
            .find(|input| input.mount_path == "images/campus.png")
            .unwrap();
        assert_eq!(image.size_bytes, 12);
        assert_eq!(
            image.sha256,
            format!("{:x}", Sha256::digest(b"campus-image"))
        );
        validate_frozen_command_trace_args(&request, &call.args).unwrap();
        let serialized = serde_json::to_string(&request).unwrap();
        assert!(
            !serialized.contains(root.join("objects/campus.png").to_string_lossy().as_ref()),
            "the Host may freeze the workspace-owned destination, but must not persist the private attachment storage path"
        );
    }

    #[test]
    fn pdf_saved_script_and_document_attachments_freeze_without_a_workspace() {
        let library = tempfile::tempdir().unwrap();
        let root = library.path().canonicalize().unwrap();
        std::fs::create_dir(root.join("objects")).unwrap();
        let pdf = b"%PDF-1.4\nfixture\n";
        let script = b"from pathlib import Path\nprint(Path(__file__).name)\n";
        std::fs::write(root.join("objects/manual.pdf"), pdf).unwrap();
        std::fs::write(root.join("objects/edit.py"), script).unwrap();
        let pdf_read_path = "@attachments/pdf-1/manual.pdf";
        let script_read_path = "@attachments/script-1/edit.py";
        let attachments = [
            (
                "pdf-1",
                "manual.pdf",
                "application/pdf",
                pdf_read_path,
                "objects/manual.pdf",
                pdf.len() as u64,
            ),
            (
                "script-1",
                "edit.py",
                "text/x-python",
                script_read_path,
                "objects/edit.py",
                script.len() as u64,
            ),
        ]
        .into_iter()
        .map(
            |(id, name, mime_type, read_path, storage_rel_path, size_bytes)| {
                AgentAttachmentReference {
                    id: id.to_string(),
                    conversation_id: "conversation-pdf".to_string(),
                    message_id: "message-pdf".to_string(),
                    project_id: None,
                    kind: AgentInputAttachmentKind::File,
                    name: name.to_string(),
                    mime_type: Some(mime_type.to_string()),
                    size_bytes,
                    read_path: read_path.to_string(),
                    storage_rel_path: storage_rel_path.to_string(),
                    created_at: 1,
                }
            },
        )
        .collect();
        let context = with_profile_resolver(
            ToolExecutionContext::from_run_context(Some(&AgentRunContext {
                collaboration_identity: None,
                conversation_id: Some("conversation-pdf".to_string()),
                project_id: None,
                workspace: None,
                attachment_library: Some(AgentAttachmentLibraryContext {
                    root_path: Some(root.to_string_lossy().into_owned()),
                    conversation_id: Some("conversation-pdf".to_string()),
                    project_id: None,
                    conversation_attachments: attachments,
                    project_attachments: Vec::new(),
                }),
                permissions: AgentPermissions::default(),
            }))
            .with_skill_resources(Some(pdf_skill_session(APPLICATION_BUNDLED_SKILL_SOURCE_ID))),
            test_binding(
                AgentCommandRuntimeProfile::Pdf,
                AgentCommandRuntimeKind::Python,
                &[
                    ("pdfplumber", "0.11.9"),
                    ("pypdf", "6.15.0"),
                    ("pypdfium2", "5.12.1"),
                    ("reportlab", "4.4.9"),
                ],
            ),
        );
        let call = AgentToolCall {
            id: "pdf-script-command".to_string(),
            tool: "run_command".to_string(),
            args: json!({
                "command": "python '$MYCOPILOT_INPUT_ROOT/edit.py' '$MYCOPILOT_INPUT_ROOT/manual.pdf'",
                "inputs": [
                    {"path": script_read_path, "mountPath": "edit.py"},
                    {"path": pdf_read_path, "mountPath": "manual.pdf"}
                ],
                "reason": "Edit and verify the attached PDF"
            }),
            approval_status: AgentApprovalStatus::Required,
            reason: None,
        };

        let request = command_request_from_call(&context, &call).unwrap();
        assert!(request.cwd.is_none());
        assert_eq!(request.inputs.len(), 2);
        assert_eq!(
            request
                .runtime_binding
                .as_deref()
                .map(|binding| binding.profile),
            Some(AgentCommandRuntimeProfile::Pdf)
        );
        validate_frozen_command_trace_args(&request, &call.args).unwrap();
        assert!(!serde_json::to_string(&request)
            .unwrap()
            .contains(root.to_string_lossy().as_ref()));
    }

    #[test]
    fn frozen_trace_argument_verifier_binds_every_command_authority_field() {
        let live_args = json!({
            "command": "node scripts/build.mjs --output outputs/report.xlsx",
            "cwd": "scripts/.",
            "reason": "build the reviewed workbook",
            "observe": {
                "kinds": ["office"],
                "expectedOutputs": [" outputs/report.xlsx "],
                "additionalRoots": []
            },
            "runtimeProfile": "spreadsheets"
        });
        let call = AgentToolCall {
            id: "tool-runtime-trace".to_string(),
            tool: "run_command".to_string(),
            args: live_args.clone(),
            approval_status: AgentApprovalStatus::Required,
            reason: None,
        };
        let context = with_profile_resolver(
            ToolExecutionContext::from_run_context(Some(&AgentRunContext {
                collaboration_identity: None,
                conversation_id: None,
                project_id: None,
                workspace: Some(AgentWorkspaceContext {
                    project_id: None,
                    display_name: Some("temp".to_string()),
                    root_path: Some(std::env::temp_dir().to_string_lossy().to_string()),
                }),
                attachment_library: None,
                permissions: AgentPermissions {
                    write: AgentWritePermission::WorkspaceOnly,
                    ..Default::default()
                },
            })),
            test_binding(
                AgentCommandRuntimeProfile::Spreadsheets,
                AgentCommandRuntimeKind::Node,
                &[("exceljs", "4.4.0")],
            ),
        );
        let frozen = command_request_from_call(&context, &call).unwrap();
        let args = live_args;
        validate_frozen_command_trace_args(&frozen, &args).unwrap();

        let mut removed_timeout = args.clone();
        removed_timeout["timeoutMs"] = json!(15_000);
        let error = validate_frozen_command_trace_args(&frozen, &removed_timeout).unwrap_err();
        assert!(error.contains("arguments are invalid"));

        let mut tampered = Vec::new();
        for (field, value) in [
            ("command", json!("node scripts/other.mjs")),
            ("cwd", json!("other")),
            ("reason", json!("different authority")),
        ] {
            let mut candidate = args.clone();
            candidate[field] = value;
            tampered.push(candidate);
        }
        let mut observe = args.clone();
        observe["observe"]["expectedOutputs"] = json!(["outputs/other.xlsx"]);
        tampered.push(observe);
        let mut runtime_profile = args.clone();
        runtime_profile["runtimeProfile"] = json!("presentations");
        tampered.push(runtime_profile);
        let mut hidden_runtime = args.clone();
        hidden_runtime["runtime"] = json!({
            "provider": "managedArtifact",
            "kind": "node",
            "requiredPackages": [{"name": "exceljs", "version": "4.4.0"}]
        });
        tampered.push(hidden_runtime);
        let mut unknown = args;
        unknown["executable"] = json!("/tmp/untrusted-node");
        tampered.push(unknown);

        for candidate in tampered {
            assert!(
                validate_frozen_command_trace_args(&frozen, &candidate).is_err(),
                "tampered command ToolCall was accepted: {candidate}"
            );
        }
    }

    #[test]
    fn rejects_unknown_top_level_runtime_authority_fields() {
        let call = AgentToolCall {
            id: "tool-runtime-unknown".to_string(),
            tool: "run_command".to_string(),
            args: json!({
                "command": "node scripts/build.mjs",
                "executable": "/tmp/fake-node"
            }),
            approval_status: AgentApprovalStatus::Required,
            reason: None,
        };
        let context = ToolExecutionContext::from_run_context(Some(&AgentRunContext {
            collaboration_identity: None,
            conversation_id: None,
            project_id: None,
            workspace: Some(AgentWorkspaceContext {
                project_id: None,
                display_name: Some("temp".to_string()),
                root_path: Some(std::env::temp_dir().to_string_lossy().to_string()),
            }),
            attachment_library: None,
            permissions: AgentPermissions::default(),
        }));

        let error = command_request_from_call(&context, &call).unwrap_err();
        assert!(error.to_string().contains("unknown field"));
    }

    #[test]
    fn freezes_bounded_office_observation_hint_without_authority_fields() {
        let call = AgentToolCall {
            id: "tool-observe".to_string(),
            tool: "run_command".to_string(),
            args: json!({
                "command": "node build.mjs",
                "observe": {
                    "kinds": ["office"],
                    "expectedOutputs": [" outputs/report.xlsx ", "outputs/report.xlsx"],
                    "additionalRoots": ["outputs"]
                }
            }),
            approval_status: AgentApprovalStatus::Required,
            reason: None,
        };
        let context = ToolExecutionContext::from_run_context(Some(&AgentRunContext {
            collaboration_identity: None,
            conversation_id: None,
            project_id: None,
            workspace: Some(AgentWorkspaceContext {
                project_id: None,
                display_name: Some("temp".to_string()),
                root_path: Some(std::env::temp_dir().to_string_lossy().to_string()),
            }),
            attachment_library: None,
            permissions: AgentPermissions {
                write: AgentWritePermission::WorkspaceOnly,
                ..Default::default()
            },
        }));

        let request = command_request_from_call(&context, &call).unwrap();
        let observe = request.observe.unwrap();
        assert_eq!(observe.kinds, [AgentCommandArtifactObservationKind::Office]);
        assert_eq!(observe.expected_outputs, ["outputs/report.xlsx"]);
        assert_eq!(observe.additional_roots, ["outputs"]);
    }

    #[test]
    fn canonicalizes_multiline_commands_without_discarding_script_whitespace() {
        let raw = "\r\n  printf one\rprintf two\r\n";
        assert_eq!(
            sanitize_command(raw).unwrap(),
            "\n  printf one\nprintf two\n"
        );

        assert!(sanitize_command(" \r\n\t ")
            .unwrap_err()
            .to_string()
            .contains("不能为空"));
        assert!(sanitize_command("printf ok\0hidden")
            .unwrap_err()
            .to_string()
            .contains("空字符"));
    }

    #[test]
    fn command_character_limit_matches_the_model_schema_at_both_boundaries() {
        let at_limit = "x".repeat(MAX_COMMAND_CHARS);
        assert_eq!(sanitize_command(&at_limit).unwrap(), at_limit);

        let above_limit = "x".repeat(MAX_COMMAND_CHARS + 1);
        let error = sanitize_command(&above_limit).unwrap_err();
        assert!(error.to_string().contains(&MAX_COMMAND_CHARS.to_string()));

        let definition = RunCommandTool.definition();
        assert_eq!(
            definition.input_schema["properties"]["command"]["maxLength"],
            json!(MAX_COMMAND_CHARS)
        );
    }

    #[test]
    fn rejects_cwd_outside_workspace() {
        let context = ToolExecutionContext::from_run_context(Some(&AgentRunContext {
            collaboration_identity: None,
            conversation_id: None,
            project_id: None,
            workspace: Some(AgentWorkspaceContext {
                project_id: None,
                display_name: Some("temp".to_string()),
                root_path: Some(std::env::temp_dir().to_string_lossy().to_string()),
            }),
            attachment_library: None,
            permissions: AgentPermissions {
                write: AgentWritePermission::WorkspaceOnly,
                ..Default::default()
            },
        }));
        let error = sanitize_cwd(&context, Some("../outside".to_string())).unwrap_err();

        assert!(error.to_string().contains("路径不能包含"));
    }

    #[test]
    fn no_workspace_requires_explicit_cwd_and_accepts_alias_with_full_write() {
        let context = ToolExecutionContext::from_run_context(Some(&AgentRunContext {
            collaboration_identity: None,
            conversation_id: None,
            project_id: None,
            workspace: None,
            attachment_library: None,
            permissions: AgentPermissions {
                write: AgentWritePermission::All,
                ..Default::default()
            },
        }));

        assert!(sanitize_cwd(&context, None).is_err());
        let cwd = sanitize_cwd(&context, Some("@home".to_string()))
            .unwrap()
            .unwrap();
        assert!(std::path::Path::new(&cwd).is_absolute());
    }

    #[test]
    fn classifies_common_risk_levels() {
        assert_eq!(
            classify_command_risk("git diff"),
            AgentCommandRiskLevel::Unknown
        );
        assert_eq!(
            classify_command_risk("git status"),
            AgentCommandRiskLevel::Unknown
        );
        assert_eq!(
            classify_command_risk("git remote -v"),
            AgentCommandRiskLevel::ReadOnly
        );
        assert_eq!(
            classify_command_risk("cargo test"),
            AgentCommandRiskLevel::WritesWorkspace
        );
        assert_eq!(
            classify_command_risk("pnpm install"),
            AgentCommandRiskLevel::Network
        );
        assert_eq!(
            classify_command_risk("rm -rf target"),
            AgentCommandRiskLevel::Destructive
        );
        assert_eq!(
            classify_command_risk("git branch scratch"),
            AgentCommandRiskLevel::WritesWorkspace
        );
        assert_eq!(
            classify_command_risk("find . -delete"),
            AgentCommandRiskLevel::Destructive
        );
    }

    #[test]
    fn model_projection_keeps_only_actionable_capture_safety_metadata() {
        let raw = AgentToolResult {
            exact_archive_file: None,
            call_id: "command-capture".to_string(),
            tool: "run_command".to_string(),
            ok: true,
            result: Some(json!({
                "exitCode": 0,
                "stdout": "preview",
                "stderr": "",
                "stdoutTruncated": true,
                "stderrTruncated": false,
                "stdoutPreviewTruncated": true,
                "stderrPreviewTruncated": false,
                "originalBytes": 70_000_000,
                "capturedBytes": 67_108_864,
                "omittedBytes": 2_891_136,
                "truncatedAtSource": true,
                "stopReason": "exact_text_capture_safety_limit",
                "stdoutOriginalBytes": 70_000_000,
                "stdoutCapturedBytes": 67_108_864,
                "stdoutOmittedBytes": 2_891_136,
                "stderrOriginalBytes": 0,
                "stderrCapturedBytes": 0,
                "stderrOmittedBytes": 0,
            })),
            error: None,
        };

        let projected = run_command_model_projection(&raw);
        let result = projected.result.unwrap();
        assert_eq!(result["originalBytes"], 70_000_000);
        assert_eq!(result["omittedBytes"], 2_891_136);
        assert_eq!(result["truncatedAtSource"], true);
        assert_eq!(result["stdoutOmittedBytes"], 2_891_136);
        assert_eq!(result["stdoutPreviewTruncated"], true);
        assert!(result.get("stderrPreviewTruncated").is_none());
        assert!(result.get("stdoutOriginalBytes").is_none());
        assert!(result.get("stderrOriginalBytes").is_none());
    }

    #[test]
    fn model_projection_keeps_the_complete_running_session_receipt() {
        let raw = AgentToolResult {
            exact_archive_file: None,
            call_id: "command-running".to_string(),
            tool: "run_command".to_string(),
            ok: true,
            result: Some(json!({
                "status": "running",
                "sessionId": "cmd_0123456789abcdef0123456789abcdef",
                "output": "server listening on port 3000",
                "startedAt": 1_725_000_000_000_i64,
                "latestSequence": 3,
                "outputTruncated": false,
                "hostPrivateField": "must not reach the model",
            })),
            error: None,
        };

        let projected = run_command_model_projection(&raw);
        let result = projected.result.unwrap();

        assert_eq!(result["status"], "running");
        assert_eq!(result["sessionId"], "cmd_0123456789abcdef0123456789abcdef");
        assert_eq!(result["output"], "server listening on port 3000");
        assert_eq!(result["startedAt"], 1_725_000_000_000_i64);
        assert_eq!(result["latestSequence"], 3);
        assert_eq!(result["outputTruncated"], false);
        assert!(result.get("hostPrivateField").is_none());
    }

    #[test]
    fn model_projection_preserves_authoritative_exact_history_route() {
        let history_open = "hist_v1_authoritative_command_page";
        let raw = AgentToolResult {
            exact_archive_file: None,
            call_id: "command-exited".to_string(),
            tool: "run_command".to_string(),
            ok: true,
            result: Some(json!({
                "status": "exited",
                "exitCode": 0,
                "stdout": "bounded preview",
                "historyOpen": history_open,
                "continueWith": {
                    "tool": "conversation_history",
                    "args": { "open": history_open }
                },
                "hostPrivateField": "must not reach the model"
            })),
            error: None,
        };

        let projected = run_command_model_projection(&raw);
        let result = projected.result.unwrap();

        assert_eq!(result["historyOpen"], history_open);
        assert_eq!(result["continueWith"]["tool"], "conversation_history");
        assert_eq!(result["continueWith"]["args"]["open"], history_open);
        assert!(result.get("hostPrivateField").is_none());
    }

    #[test]
    fn model_projection_retains_routes_while_other_consumers_keep_audit_fields() {
        let history_open = "hist_v1_authoritative_command_page";
        let read_path = format!("artifact://sha256/{}", "a".repeat(64));
        let raw = AgentToolResult {
            exact_archive_file: None,
            call_id: "command-persisted-model-projection".to_string(),
            tool: "run_command".to_string(),
            ok: true,
            result: Some(json!({
                "status": "exited",
                "command": "pdfinfo $MYCOPILOT_INPUT_ROOT/manual.pdf",
                "cwd": ".",
                "exitCode": 0,
                "stdout": "bounded preview",
                "stderr": "",
                "historyOpen": history_open,
                "continueWith": {
                    "tool": "conversation_history",
                    "args": { "open": history_open }
                },
                "outputs": [{
                    "name": "manual.pdf",
                    "kind": "document",
                    "readPath": read_path,
                    "mimeType": "application/pdf",
                    "sizeBytes": 123,
                    "sha256": "a".repeat(64)
                }],
                "inputFiles": [{
                    "mountPath": "manual.pdf",
                    "sourceKind": "attachment",
                    "sizeBytes": 456,
                    "sha256": "b".repeat(64)
                }],
                "runtime": {
                    "schemaVersion": 1,
                    "providerId": "managed-artifact-runtime",
                    "profile": "pdf",
                    "kind": "python",
                    "runtimeFingerprint": "host-audit-only",
                    "resolvedPackages": [{ "name": "pypdf", "version": "6.15.0" }]
                }
            })),
            error: None,
        };

        let tool = RunCommandTool;
        let live = tool.model_projection(&raw);
        let checkpoint = tool.checkpoint_projection(&raw);

        let model_result = live.result.as_ref().unwrap();
        assert_eq!(model_result["historyOpen"], history_open);
        assert_eq!(model_result["continueWith"]["args"]["open"], history_open);
        assert_eq!(model_result["outputs"][0]["readPath"], read_path);
        assert!(model_result.get("runtime").is_none());
        assert!(model_result.get("inputFiles").is_none());
        assert!(model_result["outputs"][0].get("sha256").is_none());

        let checkpoint_result = checkpoint.result.as_ref().unwrap();
        assert_eq!(checkpoint_result["runtime"]["profile"], "pdf");
        assert_eq!(
            checkpoint_result["inputFiles"][0]["mountPath"],
            "manual.pdf"
        );
        assert_eq!(checkpoint_result["outputs"][0]["sha256"], "a".repeat(64));

        for durable in [
            tool.trace_projection(&raw),
            tool.archive_projection(&raw),
            tool.event_projection(&raw),
        ] {
            let result = durable.result.as_ref().unwrap();
            assert_eq!(result["runtime"]["profile"], "pdf");
            assert_eq!(result["inputFiles"][0]["mountPath"], "manual.pdf");
            assert_eq!(result["outputs"][0]["sha256"], "a".repeat(64));
        }
    }

    #[test]
    fn model_contract_routes_long_lived_commands_without_polling_loops() {
        let definition = RunCommandTool.definition();

        assert!(definition
            .description
            .contains("running is not final success"));
        assert!(definition
            .description
            .contains("Host owns process lifetime"));
        assert!(definition
            .description
            .contains("do not add a deadline merely to bound tool waiting or confirm startup"));
        assert!(definition
            .description
            .contains("GUI app or long-lived server"));
        assert!(definition
            .description
            .contains("command_session once with action=wait"));
        assert!(definition
            .description
            .contains("do not repeatedly poll or narrate waiting"));
    }
}
