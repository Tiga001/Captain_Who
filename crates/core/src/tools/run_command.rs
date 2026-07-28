use super::{
    clean_relative_path, schema::agent_file_input_ref_schema, AgentTool, ToolExecutionContext,
};
use crate::command::{
    classify_command_risk, infer_managed_artifact_builder_command,
    infer_managed_artifact_command_kind, validate_command_runtime_binding,
    validate_command_runtime_request, validate_managed_artifact_builder_output_scope,
    validate_managed_artifact_command_shape, MAX_ADDITIONAL_ROOTS, MAX_COMMAND_CHARS,
    MAX_EXPECTED_OUTPUTS, MAX_OBSERVATION_PATH_CHARS,
};
use crate::file_input::{
    normalize_agent_file_input_specs, prepare_agent_file_input_bindings,
    AgentFileInputExecutionContext, MAX_AGENT_FILE_INPUTS, MAX_AGENT_FILE_INPUT_MOUNT_PATH_CHARS,
};
use crate::protocol::{
    AgentApprovalStatus, AgentCommandArtifactObservationKind,
    AgentCommandArtifactObservationRequest, AgentCommandRequest, AgentCommandRuntimeProfile,
    AgentCommandRuntimeRequest, AgentError, AgentFileInputSpec, AgentProposedAction, AgentResult,
    AgentSkillMaterializationResult, AgentSkillMaterializationResultStatus, AgentToolCall,
    AgentToolDefinition, AgentToolResult, AgentToolSafety,
};
use crate::skills::{
    SkillResourceUri, APPLICATION_BUNDLED_SKILL_SOURCE_ID, DOCUMENTS_LOCAL_ID,
    PRESENTATIONS_LOCAL_ID, SPREADSHEETS_LOCAL_ID,
};
use crate::system_paths::expand_system_path;
use serde::Deserialize;
use serde_json::{json, Map, Value};
use std::collections::BTreeSet;
use std::path::{Component, Path, PathBuf};

const DEFAULT_TIMEOUT_MS: u64 = 120_000;
const MAX_TIMEOUT_MS: u64 = 600_000;

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
            description: "Run one single-line non-interactive shell command through the host for builds, tests, queries, dependency management, or program execution. Command policy may execute it automatically, request explicit user approval, or deny catastrophic/unsupported operations. This policy is not an OS sandbox. Prefer apply_patch for reviewable source edits. The command must not contain literal newlines or null characters. For a backend-verified Office Skill Builder materialized in this run, use one direct Python/Node command with `--output <file.docx|file.xlsx|file.pptx>`; the host binds the matching managed runtime and observes the output, so runtimeProfile and observe are not required. inputs may bind authorized attachments, workspace/external files, generated Artifacts, or activated Skill resources into a private read-only input root. The managed script reads MYCOPILOT_INPUT_ROOT plus each declared mountPath; it must never open @attachments or skill:// directly.".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "command": { "type": "string", "description": "A single-line, non-interactive shell command without literal newline or null characters. Side effects and every compound shell segment are evaluated by the host policy." },
                    "cwd": { "type": "string", "description": "Working directory. May be workspace-relative, absolute, or @home/@desktop/@documents/@downloads when permissions allow. Required when no workspace exists." },
                    "timeoutMs": { "type": "integer", "minimum": 1, "maximum": MAX_TIMEOUT_MS },
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
                        "description": "Optional read-only inputs for a managed Office Builder or explicit runtimeProfile command. The host freezes hash/size, revalidates after approval, and materializes each source below MYCOPILOT_INPUT_ROOT at mountPath. This field is unavailable for ordinary shell commands.",
                        "items": {
                            "type": "object",
                            "properties": {
                                "mountPath": {
                                    "type": "string",
                                    "minLength": 1,
                                    "maxLength": MAX_AGENT_FILE_INPUT_MOUNT_PATH_CHARS,
                                    "description": "Stable safe relative path beneath MYCOPILOT_INPUT_ROOT, for example images/campus.png."
                                },
                                "source": agent_file_input_ref_schema()
                            },
                            "required": ["mountPath", "source"],
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
    for field in ["exitCode", "stdout", "stderr", "error"] {
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
    (!output.is_empty()).then_some(Value::Object(output))
}

fn project_artifact_observation(value: &Value) -> Option<Value> {
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
    timeout_ms: Option<u64>,
    reason: Option<String>,
    observe: Option<AgentCommandArtifactObservationRequest>,
    runtime_profile: Option<AgentCommandRuntimeProfile>,
    #[serde(default)]
    inputs: Vec<AgentFileInputSpec>,
}

fn command_request_from_call(
    context: &ToolExecutionContext,
    call: &AgentToolCall,
) -> AgentResult<AgentCommandRequest> {
    let args: RunCommandArgs = serde_json::from_value(call.args.clone())
        .map_err(|error| AgentError::new(format!("run_command 参数无效：{error}")))?;
    let command = sanitize_command(&args.command)?;
    let cwd = sanitize_cwd(context, args.cwd)?;
    let reason = args
        .reason
        .or_else(|| call.reason.clone())
        .map(|reason| reason.trim().to_string())
        .filter(|reason| !reason.is_empty());
    let host_builder_profile =
        trusted_materialized_builder_profile(context, &command, cwd.as_deref())?;
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
    let runtime_binding = builder_config
        .runtime_profile
        .map(|profile| prepare_runtime_binding(context, &command, profile))
        .transpose()?;
    if !args.inputs.is_empty() && runtime_binding.is_none() {
        return Err(AgentError::structured(
            "agent.fileInput.invalidRequest",
            "run_command.inputs 只适用于带有 `--output` Office 文件的 Managed Builder，或显式设置了 runtimeProfile 的托管脚本命令。",
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
    .with_storage(context.storage_optional());
    let workspace_root = context.workspace_root_optional()?;
    let inputs = prepare_agent_file_input_bindings(
        workspace_root.as_deref(),
        context.permissions(),
        &input_context,
        &args.inputs,
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

    Ok(AgentCommandRequest {
        id: call.id.clone(),
        command: command.clone(),
        cwd,
        timeout_ms: Some(
            args.timeout_ms
                .unwrap_or(DEFAULT_TIMEOUT_MS)
                .clamp(1, MAX_TIMEOUT_MS),
        ),
        approval_status: AgentApprovalStatus::Required,
        risk_level: Some(classify_command_risk(&command)),
        reason,
        observe: builder_config.observe,
        inputs,
        runtime: None,
        runtime_binding: runtime_binding.map(Box::new),
    })
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
) -> AgentResult<Option<AgentCommandRuntimeProfile>> {
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
        let Some(profile) = profile_for_bundled_builder_receipt(&receipt, builder.kind)? else {
            continue;
        };
        profiles.insert(profile);
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
    let Some(profile) = profiles.into_iter().next() else {
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
    Ok(Some(profile))
}

fn profile_for_bundled_builder_receipt(
    receipt: &AgentSkillMaterializationResult,
    command_kind: crate::AgentCommandRuntimeKind,
) -> AgentResult<Option<AgentCommandRuntimeProfile>> {
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
    let profile = match (local_id, path, command_kind) {
        (DOCUMENTS_LOCAL_ID, "templates/builder.py", crate::AgentCommandRuntimeKind::Python) => {
            AgentCommandRuntimeProfile::Documents
        }
        (SPREADSHEETS_LOCAL_ID, "templates/builder.py", crate::AgentCommandRuntimeKind::Python) => {
            AgentCommandRuntimeProfile::Spreadsheets
        }
        (PRESENTATIONS_LOCAL_ID, "templates/builder.mjs", crate::AgentCommandRuntimeKind::Node) => {
            AgentCommandRuntimeProfile::Presentations
        }
        _ => return Ok(None),
    };
    Ok(Some(profile))
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
    if let Some(builder) = builder {
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
    if host_builder_profile.is_some() && inferred_profile.is_none() {
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
    let observe = if runtime_profile.is_some() {
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
    let kind = infer_managed_artifact_command_kind(command).map_err(|message| {
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
    let timeout_ms = Some(
        args.timeout_ms
            .unwrap_or(DEFAULT_TIMEOUT_MS)
            .clamp(1, MAX_TIMEOUT_MS),
    );
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
    let inputs = normalize_agent_file_input_specs(&args.inputs)
        .map_err(|_| "run_command frozen ToolCall inputs are invalid".to_string())?;
    let frozen_inputs = frozen
        .inputs
        .iter()
        .map(|binding| AgentFileInputSpec {
            mount_path: binding.mount_path.clone(),
            source: binding.source.clone(),
        })
        .collect::<Vec<_>>();
    let mut expected_observe = explicit_observe.clone();
    let runtime_matches = match (&frozen.runtime_binding, &frozen.runtime) {
        (Some(binding), None) => {
            let derived = derive_managed_builder_config(
                &command,
                args.runtime_profile,
                args.observe.clone(),
                args.runtime_profile.is_none().then_some(binding.profile),
            )
            .map_err(|_| {
                "run_command frozen ToolCall Managed Builder config is invalid".to_string()
            })?;
            // Pending actions created before backend observation binding may legitimately have
            // neither a model-authored nor a frozen observation. Preserve that narrow legacy
            // shape; every newly prepared managed command freezes a non-empty Office observer.
            expected_observe = if frozen.observe.is_none() && explicit_observe.is_none() {
                None
            } else {
                derived.observe
            };
            validate_command_runtime_binding(binding).is_ok()
                && args.runtime.is_none()
                && derived.runtime_profile == Some(binding.profile)
                && infer_managed_artifact_command_kind(&command).ok() == Some(binding.kind)
        }
        (None, Some(legacy)) => {
            args.runtime_profile.is_none()
                && args
                    .runtime
                    .map(|runtime| sanitize_runtime(&command, runtime))
                    .transpose()
                    .map_err(|_| {
                        "run_command frozen ToolCall legacy runtime request is invalid".to_string()
                    })?
                    .as_ref()
                    == Some(legacy)
        }
        (None, None) => args.runtime_profile.is_none() && args.runtime.is_none(),
        (Some(_), Some(_)) => false,
    };

    if command != frozen.command
        || cwd != frozen.cwd
        || timeout_ms != frozen.timeout_ms
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

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct FrozenRunCommandArgs {
    command: String,
    cwd: Option<String>,
    timeout_ms: Option<u64>,
    reason: Option<String>,
    observe: Option<AgentCommandArtifactObservationRequest>,
    runtime_profile: Option<AgentCommandRuntimeProfile>,
    #[serde(default)]
    inputs: Vec<AgentFileInputSpec>,
    /// Legacy field accepted only while reconciling already-persisted pending actions.
    runtime: Option<AgentCommandRuntimeRequest>,
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

fn sanitize_runtime(
    command: &str,
    runtime: AgentCommandRuntimeRequest,
) -> AgentResult<AgentCommandRuntimeRequest> {
    validate_command_runtime_request(&runtime).map_err(AgentError::new)?;
    validate_managed_artifact_command_shape(command, &runtime).map_err(AgentError::new)?;
    Ok(runtime)
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
    let command = command.trim();
    if command.is_empty() {
        return Err(AgentError::new("run_command.command 不能为空。"));
    }
    if command.chars().count() > MAX_COMMAND_CHARS {
        return Err(AgentError::new(format!(
            "run_command.command 过长，最多允许 {MAX_COMMAND_CHARS} 个字符。"
        )));
    }
    if command.contains('\0') || command.contains('\n') || command.contains('\r') {
        return Err(AgentError::new(
            "run_command.command 不能包含空字符或换行符。请改成单行命令；可审查的源代码编辑应优先使用 apply_patch。",
        ));
    }

    Ok(command.to_string())
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
        SkillId, SkillPackageUri, SkillResourcePath, SkillRevision,
        APPLICATION_BUNDLED_SKILL_SOURCE_ID,
    };
    use crate::storage::models::AgentActionAuditRecord;
    use crate::storage::service::StorageService;
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
            bundle_version: "2026.07.3".to_string(),
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

    #[test]
    fn model_schema_exposes_profiles_but_no_runtime_authority() {
        let definition = RunCommandTool.definition();
        let properties = definition.input_schema["properties"].as_object().unwrap();
        assert!(properties.contains_key("runtimeProfile"));
        assert!(!properties.contains_key("runtime"));
        let serialized = serde_json::to_string(&definition.input_schema).unwrap();
        for forbidden in [
            "requiredPackages",
            "managedArtifact",
            "pptxgenjs",
            "4.0.1",
            "3.12.0",
        ] {
            assert!(
                !serialized.contains(forbidden),
                "model schema leaked backend runtime authority: {forbidden}"
            );
        }
    }

    #[test]
    fn builds_command_request_for_approval() {
        let call = AgentToolCall {
            id: "tool-1".to_string(),
            tool: "run_command".to_string(),
            args: json!({
                "command": " cargo test ",
                "cwd": "agent/rust",
                "timeoutMs": 999_999,
                "reason": "verify tests"
            }),
            approval_status: AgentApprovalStatus::Required,
            reason: None,
        };
        let context = ToolExecutionContext::from_run_context(Some(&AgentRunContext {
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
        assert_eq!(request.command, "cargo test");
        assert_eq!(request.cwd.as_deref(), Some("agent/rust"));
        assert_eq!(request.timeout_ms, Some(MAX_TIMEOUT_MS));
        assert_eq!(request.approval_status, AgentApprovalStatus::Required);
        assert_eq!(
            request.risk_level,
            Some(AgentCommandRiskLevel::WritesWorkspace)
        );
        assert_eq!(request.reason.as_deref(), Some("verify tests"));
        assert!(request.observe.is_none());
        assert!(request.runtime.is_none());
        assert!(request.runtime_binding.is_none());
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
                packages: &[("openpyxl", "3.1.5"), ("xlsxwriter", "3.2.9")],
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
    fn ordinary_saved_scripts_are_not_silently_rebound_without_backend_provenance() {
        let workspace = tempfile::tempdir().unwrap();
        std::fs::write(workspace.path().join("ordinary.py"), "print('ordinary')\n").unwrap();
        let context = with_profile_resolver(
            ToolExecutionContext::from_run_context(Some(&AgentRunContext {
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
        let external = AgentToolCall {
            id: "tool-external-output".to_string(),
            tool: "run_command".to_string(),
            args: json!({
                "command": "python builder.py --output /tmp/mycopilot-managed-builder.docx"
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
            ["/tmp/mycopilot-managed-builder.docx"]
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
                    "source": {
                        "type": "attachment",
                        "readPath": read_path
                    }
                }]
            }),
            approval_status: AgentApprovalStatus::Required,
            reason: None,
        };
        let context = with_profile_resolver(
            ToolExecutionContext::from_run_context(Some(&AgentRunContext {
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
        assert_eq!(request.inputs.len(), 1);
        assert_eq!(request.inputs[0].mount_path, "images/campus.png");
        assert_eq!(request.inputs[0].size_bytes, 12);
        assert_eq!(
            request.inputs[0].sha256,
            format!("{:x}", Sha256::digest(b"campus-image"))
        );
        let serialized = serde_json::to_string(&request).unwrap();
        assert!(!serialized.contains(root.to_string_lossy().as_ref()));
    }

    #[test]
    fn frozen_trace_argument_verifier_binds_every_command_authority_field() {
        let args = json!({
            "command": "node scripts/build.mjs --output outputs/report.xlsx",
            "cwd": "scripts/.",
            "timeoutMs": 999_999,
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
            args: args.clone(),
            approval_status: AgentApprovalStatus::Required,
            reason: None,
        };
        let context = with_profile_resolver(
            ToolExecutionContext::from_run_context(Some(&AgentRunContext {
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
        validate_frozen_command_trace_args(&frozen, &args).unwrap();

        let mut tampered = Vec::new();
        for (field, value) in [
            ("command", json!("node scripts/other.mjs")),
            ("cwd", json!("other")),
            ("timeoutMs", json!(1)),
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
    fn rejects_unsafe_command_shape_before_approval() {
        let error = sanitize_command("echo one\necho two").unwrap_err();

        assert!(error.to_string().contains("换行符"));
    }

    #[test]
    fn rejects_cwd_outside_workspace() {
        let context = ToolExecutionContext::from_run_context(Some(&AgentRunContext {
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
}
