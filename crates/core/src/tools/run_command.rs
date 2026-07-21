use super::{clean_relative_path, AgentTool, ToolExecutionContext};
use crate::command::{
    classify_command_risk, infer_managed_artifact_command_kind, validate_command_runtime_binding,
    validate_command_runtime_request, validate_managed_artifact_command_shape,
    MAX_ADDITIONAL_ROOTS, MAX_COMMAND_CHARS, MAX_EXPECTED_OUTPUTS, MAX_OBSERVATION_PATH_CHARS,
};
use crate::protocol::{
    AgentApprovalStatus, AgentCommandArtifactObservationKind,
    AgentCommandArtifactObservationRequest, AgentCommandRequest, AgentCommandRuntimeProfile,
    AgentCommandRuntimeRequest, AgentError, AgentProposedAction, AgentResult, AgentToolCall,
    AgentToolDefinition, AgentToolSafety,
};
use crate::system_paths::expand_system_path;
use serde::Deserialize;
use serde_json::{json, Value};

const DEFAULT_TIMEOUT_MS: u64 = 120_000;
const MAX_TIMEOUT_MS: u64 = 600_000;

pub(super) struct RunCommandTool;

impl AgentTool for RunCommandTool {
    fn permission_policy(&self) -> super::AgentToolPermissionPolicy {
        super::AgentToolPermissionPolicy::Default
    }

    fn definition(&self) -> AgentToolDefinition {
        AgentToolDefinition {
            name: "run_command".to_string(),
            description: "Run one single-line non-interactive shell command through the host for builds, tests, queries, dependency management, or program execution. Command policy may execute it automatically, request explicit user approval, or deny catastrophic/unsupported operations. This policy is not an OS sandbox. Prefer apply_patch for reviewable source edits. The command must not contain literal newlines or null characters. Use observe to request best-effort, permission-neutral tracking and validation of Office files changed by the command.".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "command": { "type": "string", "description": "A single-line, non-interactive shell command without literal newline or null characters. Side effects and every compound shell segment are evaluated by the host policy." },
                    "cwd": { "type": "string", "description": "Working directory. May be workspace-relative, absolute, or @home/@desktop/@documents/@downloads when permissions allow. Required when no workspace exists." },
                    "timeoutMs": { "type": "integer", "minimum": 1, "maximum": MAX_TIMEOUT_MS },
                    "reason": { "type": "string", "description": "Why this command is needed and what result is expected." },
                    "observe": {
                        "type": "object",
                        "description": "Best-effort artifact observation hint. It does not grant command, read, or write permission. When present, the workspace is observed automatically; expected outputs and additional roots are resolved relative to cwd.",
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
                        "description": "Run a saved .mjs or .py artifact script in the matching fixed, host-owned Office environment. The host infers Node/Python from the direct script command and freezes exact packages, runtime version, and integrity identity. Never provide package versions. This does not grant command/file permission and never falls back to PATH."
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
    let observe = args.observe.map(sanitize_observe).transpose()?;
    let runtime_binding = args
        .runtime_profile
        .map(|profile| prepare_runtime_binding(context, &command, profile))
        .transpose()?;

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
        observe,
        runtime: None,
        runtime_binding: runtime_binding.map(Box::new),
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
    let observe = args
        .observe
        .map(sanitize_observe)
        .transpose()
        .map_err(|_| "run_command frozen ToolCall observe hint is invalid".to_string())?;
    let runtime_matches = match (&frozen.runtime_binding, &frozen.runtime) {
        (Some(binding), None) => {
            validate_command_runtime_binding(binding).is_ok()
                && args.runtime.is_none()
                && args.runtime_profile == Some(binding.profile)
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
        || observe != frozen.observe
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
        AgentApprovalStatus, AgentCommandRiskLevel, AgentCommandRuntimeBinding,
        AgentCommandRuntimeKind, AgentCommandRuntimeResolvedPackage, AgentPermissions,
        AgentRunContext, AgentToolCall, AgentWorkspaceContext, AgentWritePermission,
        AGENT_COMMAND_RUNTIME_BINDING_SCHEMA_VERSION,
    };
    use serde_json::json;
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
}
