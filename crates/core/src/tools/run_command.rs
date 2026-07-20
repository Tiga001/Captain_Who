use super::{clean_relative_path, AgentTool, ToolExecutionContext};
use crate::command::{
    classify_command_risk, validate_command_runtime_request,
    validate_managed_artifact_command_shape, MAX_ADDITIONAL_ROOTS, MAX_COMMAND_CHARS,
    MAX_EXPECTED_OUTPUTS, MAX_OBSERVATION_PATH_CHARS,
};
use crate::protocol::{
    AgentApprovalStatus, AgentCommandArtifactObservationKind,
    AgentCommandArtifactObservationRequest, AgentCommandRequest, AgentCommandRuntimeRequest,
    AgentError, AgentProposedAction, AgentResult, AgentToolCall, AgentToolDefinition,
    AgentToolSafety,
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
                    "runtime": {
                        "type": "object",
                        "description": "Resolve a saved .mjs or .py artifact script through the fixed, host-owned runtime. This does not install packages or grant command/file permission, and never falls back to PATH.",
                        "properties": {
                            "provider": { "type": "string", "enum": ["managedArtifact"] },
                            "kind": { "type": "string", "enum": ["node", "python"] },
                            "requiredPackages": {
                                "type": "array",
                                "maxItems": 32,
                                "items": {
                                    "type": "object",
                                    "properties": {
                                        "name": { "type": "string", "minLength": 1, "maxLength": 128 },
                                        "version": { "type": "string", "minLength": 1, "maxLength": 64 }
                                    },
                                    "required": ["name", "version"],
                                    "additionalProperties": false
                                }
                            }
                        },
                        "required": ["provider", "kind", "requiredPackages"],
                        "additionalProperties": false
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
    runtime: Option<AgentCommandRuntimeRequest>,
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
    let runtime = args
        .runtime
        .map(|runtime| sanitize_runtime(&command, runtime))
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
        runtime,
    })
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
    let args: RunCommandArgs = serde_json::from_value(operation.clone())
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
    let runtime = args
        .runtime
        .map(|runtime| sanitize_runtime(&command, runtime))
        .transpose()
        .map_err(|_| "run_command frozen ToolCall runtime request is invalid".to_string())?;

    if command != frozen.command
        || cwd != frozen.cwd
        || timeout_ms != frozen.timeout_ms
        || observe != frozen.observe
        || runtime != frozen.runtime
        || (reason_was_present && reason != frozen.reason)
    {
        return Err(
            "run_command frozen ToolCall arguments differ from the prepared command".to_string(),
        );
    }
    Ok(())
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
        AgentApprovalStatus, AgentCommandRiskLevel, AgentPermissions, AgentRunContext,
        AgentToolCall, AgentWorkspaceContext, AgentWritePermission,
    };
    use serde_json::json;

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
    }

    #[test]
    fn freezes_strict_managed_runtime_request() {
        let call = AgentToolCall {
            id: "tool-runtime".to_string(),
            tool: "run_command".to_string(),
            args: json!({
                "command": "node scripts/build.mjs --output outputs/report.xlsx",
                "runtime": {
                    "provider": "managedArtifact",
                    "kind": "node",
                    "requiredPackages": [
                        {"name": "exceljs", "version": "4.4.0"}
                    ]
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
        let frozen = serde_json::to_value(&request).unwrap();
        assert_eq!(frozen["runtime"]["provider"], "managedArtifact");
        assert_eq!(frozen["runtime"]["kind"], "node");
        assert_eq!(frozen["runtime"]["requiredPackages"][0]["version"], "4.4.0");
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
            "runtime": {
                "provider": "managedArtifact",
                "kind": "node",
                "requiredPackages": [
                    {"name": "exceljs", "version": "4.4.0"}
                ]
            }
        });
        let call = AgentToolCall {
            id: "tool-runtime-trace".to_string(),
            tool: "run_command".to_string(),
            args: args.clone(),
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
        let mut runtime = args.clone();
        runtime["runtime"]["requiredPackages"][0]["version"] = json!("9.9.9");
        tampered.push(runtime);
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
