use super::{AgentTool, ToolExecutionContext};
use crate::protocol::{
    AgentCommandSafetyPolicy, AgentError, AgentProposedAction, AgentReadPermission, AgentResult,
    AgentSkillScriptInterpreter, AgentSkillScriptRequest, AgentSkillScriptRequirements,
    AgentToolCall, AgentToolDefinition, AgentToolSafety, AgentWritePermission,
};
use crate::skills::{
    preflight_skill_python_script, SkillResourceUri, SkillScriptPreflightOutcome,
    SkillScriptRuntimeError, DEFAULT_SKILL_SCRIPT_TIMEOUT_MS, MAX_SKILL_SCRIPT_ARGUMENTS,
    MAX_SKILL_SCRIPT_ARGUMENT_BYTES, MAX_SKILL_SCRIPT_TIMEOUT_MS,
};
use serde::Deserialize;
use serde_json::{json, Value};

pub(super) struct SkillsPreflightScriptTool;
pub(super) struct SkillsRunScriptTool;

impl AgentTool for SkillsPreflightScriptTool {
    fn permission_policy(&self) -> super::AgentToolPermissionPolicy {
        super::AgentToolPermissionPolicy::Default
    }

    fn definition(&self) -> AgentToolDefinition {
        AgentToolDefinition {
            name: "skills_preflight_script".to_string(),
            description: "Check a currently activated Python Skill script and its declared runtime dependencies without executing the Skill script or installing anything. Returns only interpreter identity metadata, dependency names/statuses, and a revision-bound runtime fingerprint; environment values are never disclosed.".to_string(),
            input_schema: script_input_schema(false),
            safety: AgentToolSafety::ReadOnly,
            requires_workspace: true,
            requires_approval: false,
            approval_mode: crate::protocol::AgentToolApprovalMode::Never,
        }
    }

    fn execute(&self, context: &ToolExecutionContext, args: Value) -> AgentResult<Value> {
        context.check_cancelled()?;
        require_unrestricted_script_runtime(context)?;
        let args = parse_args(args, "skills_preflight_script")?;
        let uri = parse_uri(&args.script_uri)?;
        let outcome = preflight_skill_python_script(
            context.skill_resources()?,
            &context.workspace_root()?,
            &uri,
            args.interpreter,
            &args.requirements,
        )
        .map_err(map_runtime_error)?;
        Ok(json!({
            "scriptUri": uri.as_str(),
            "skillId": uri.package().skill_id().as_str(),
            "skillRevision": uri.package().revision().as_str(),
            "resourcePath": uri.path().as_str(),
            "ready": outcome.ready_plan().is_some(),
            "resourceDigest": outcome.ready_plan().map(|plan| plan.resource_digest()),
            "preflight": outcome.report(),
        }))
    }
}

impl AgentTool for SkillsRunScriptTool {
    fn permission_policy(&self) -> super::AgentToolPermissionPolicy {
        super::AgentToolPermissionPolicy::Default
    }

    fn definition(&self) -> AgentToolDefinition {
        AgentToolDefinition {
            name: "skills_run_script".to_string(),
            description: "Request execution of one revision-bound scripts/*.py resource from a currently activated Skill. Arguments are a structured argv and never pass through a shell. The host repeats dependency/integrity checks immediately before execution. Until an OS process sandbox and trusted capability grants exist, execution requires unrestricted read/write scope and Full Access, plus explicit approval for every script. Missing dependencies are reported but never installed automatically.".to_string(),
            input_schema: script_input_schema(true),
            safety: AgentToolSafety::RequiresApproval,
            requires_workspace: true,
            requires_approval: true,
            approval_mode: crate::protocol::AgentToolApprovalMode::Always,
        }
    }

    fn execute(&self, _context: &ToolExecutionContext, _args: Value) -> AgentResult<Value> {
        Err(AgentError::new(
            "skills_run_script requires host authorization and cannot execute inside the agent runtime.",
        ))
    }

    fn proposed_action(
        &self,
        context: &ToolExecutionContext,
        call: &AgentToolCall,
    ) -> AgentResult<AgentProposedAction> {
        context.check_cancelled()?;
        require_unrestricted_script_runtime(context)?;
        let args = parse_args(call.args.clone(), "skills_run_script")?;
        validate_argv(&args.args)?;
        let uri = parse_uri(&args.script_uri)?;
        let outcome = preflight_skill_python_script(
            context.skill_resources()?,
            &context.workspace_root()?,
            &uri,
            args.interpreter,
            &args.requirements,
        )
        .map_err(map_runtime_error)?;
        let SkillScriptPreflightOutcome::Ready { report, plan } = outcome else {
            let report = outcome.report().clone();
            return Err(AgentError::structured(
                report
                    .error_code
                    .clone()
                    .unwrap_or_else(|| "skill_script.preflight_not_ready".to_string()),
                report.message.clone().unwrap_or_else(|| {
                    "Skill script dependency preflight is not ready.".to_string()
                }),
                json!({
                    "type": "skill_script_preflight",
                    "code": report.error_code,
                    "recovery": "installDependencyOrChangeRequest",
                    "preflight": report,
                }),
            ));
        };
        Ok(AgentProposedAction::SkillScript {
            script: Box::new(AgentSkillScriptRequest {
                id: call.id.clone(),
                script_uri: uri.to_string(),
                skill_id: uri.package().skill_id().as_str().to_string(),
                skill_revision: uri.package().revision().as_str().to_string(),
                resource_path: uri.path().as_str().to_string(),
                resource_digest: plan.resource_digest().to_string(),
                interpreter: args.interpreter,
                args: args.args,
                requirements: args.requirements,
                preflight: report,
                timeout_ms: Some(
                    args.timeout_ms
                        .unwrap_or(DEFAULT_SKILL_SCRIPT_TIMEOUT_MS)
                        .clamp(1, MAX_SKILL_SCRIPT_TIMEOUT_MS),
                ),
                approval_status: call.approval_status,
                reason: non_empty(args.reason.as_deref())
                    .map(|reason| reason.chars().take(2_000).collect())
                    .or_else(|| call.reason.clone()),
            }),
        })
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ScriptArgs {
    script_uri: String,
    #[serde(default = "python3")]
    interpreter: AgentSkillScriptInterpreter,
    #[serde(default)]
    args: Vec<String>,
    #[serde(default)]
    requirements: AgentSkillScriptRequirements,
    timeout_ms: Option<u64>,
    reason: Option<String>,
}

fn python3() -> AgentSkillScriptInterpreter {
    AgentSkillScriptInterpreter::Python3
}

fn parse_args(value: Value, tool: &str) -> AgentResult<ScriptArgs> {
    let args: ScriptArgs = serde_json::from_value(value)
        .map_err(|error| AgentError::new(format!("{tool} parameters are invalid: {error}")))?;
    if args.script_uri.trim().is_empty() {
        return Err(AgentError::new(format!(
            "{tool}.scriptUri cannot be empty."
        )));
    }
    Ok(args)
}

fn parse_uri(value: &str) -> AgentResult<SkillResourceUri> {
    SkillResourceUri::parse(value.trim()).map_err(|error| {
        AgentError::structured(
            "skill_script.invalid_resource_uri",
            error.to_string(),
            json!({
                "type": "skill_script",
                "code": error.code().stable_name(),
                "recovery": error.recovery().stable_name(),
            }),
        )
    })
}

fn validate_argv(args: &[String]) -> AgentResult<()> {
    let byte_count = args
        .iter()
        .try_fold(0usize, |total, argument| total.checked_add(argument.len()))
        .ok_or_else(|| AgentError::new("skills_run_script argv size overflowed."))?;
    if args.len() > MAX_SKILL_SCRIPT_ARGUMENTS || byte_count > MAX_SKILL_SCRIPT_ARGUMENT_BYTES {
        return Err(AgentError::structured(
            "skill_script.argv_too_large",
            "skills_run_script argv exceeds its bounded size limit.",
            json!({
                "type": "skill_script",
                "code": "argvTooLarge",
                "maxArguments": MAX_SKILL_SCRIPT_ARGUMENTS,
                "maxArgumentBytes": MAX_SKILL_SCRIPT_ARGUMENT_BYTES,
            }),
        ));
    }
    if args.iter().any(|argument| argument.contains('\0')) {
        return Err(AgentError::structured(
            "skill_script.invalid_argv",
            "skills_run_script argv cannot contain NUL bytes.",
            json!({ "type": "skill_script", "code": "invalidArgv" }),
        ));
    }
    Ok(())
}

fn map_runtime_error(error: SkillScriptRuntimeError) -> AgentError {
    AgentError::structured(
        format!("skill_script.{}", error.code().stable_name()),
        error.message(),
        json!({
            "type": "skill_script",
            "code": error.code().stable_name(),
            "recovery": error.recovery().stable_name(),
        }),
    )
}

fn non_empty(value: Option<&str>) -> Option<&str> {
    value.map(str::trim).filter(|value| !value.is_empty())
}

fn require_unrestricted_script_runtime(context: &ToolExecutionContext) -> AgentResult<()> {
    let permissions = context.permissions();
    if permissions.read == AgentReadPermission::All
        && permissions.write == AgentWritePermission::All
        && permissions.command_safety == AgentCommandSafetyPolicy::FullAccess
    {
        return Ok(());
    }
    Err(AgentError::structured(
        "skill_script.full_access_required",
        "Skill dependency inspection and execution require unrestricted read/write scope and Full Access until OS-level process isolation is available.",
        json!({
            "type": "skill_script_policy",
            "code": "fullAccessRequired",
            "recovery": "changePermissions",
        }),
    ))
}

fn script_input_schema(include_execution: bool) -> Value {
    let mut schema = json!({
        "type": "object",
        "properties": {
            "scriptUri": { "type": "string", "description": "Exact revision-bound script URI returned by skills_list_resources." },
            "interpreter": { "type": "string", "enum": ["python3"], "description": "Logical interpreter identifier; arbitrary executable paths are forbidden." },
            "requirements": {
                "type": "object",
                "properties": {
                    "pythonDistributions": { "type": "array", "maxItems": 128, "items": { "type": "string", "maxLength": 128 } },
                    "commands": { "type": "array", "maxItems": 128, "items": { "type": "string", "maxLength": 128 } }
                }
            }
        },
        "required": ["scriptUri"]
    });
    if include_execution {
        let properties = schema
            .get_mut("properties")
            .and_then(Value::as_object_mut)
            .expect("static script schema has properties");
        properties.insert(
            "args".to_string(),
            json!({ "type": "array", "maxItems": MAX_SKILL_SCRIPT_ARGUMENTS, "items": { "type": "string" }, "description": "Structured argv passed literally without shell parsing." }),
        );
        properties.insert(
            "timeoutMs".to_string(),
            json!({ "type": "integer", "minimum": 1, "maximum": MAX_SKILL_SCRIPT_TIMEOUT_MS }),
        );
        properties.insert(
            "reason".to_string(),
            json!({ "type": "string", "maxLength": 2000 }),
        );
    }
    schema
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{AgentPermissions, AgentRunContext};

    #[test]
    fn rejects_oversized_and_nul_argv_before_preflight() {
        assert!(validate_argv(&vec!["x".to_string(); MAX_SKILL_SCRIPT_ARGUMENTS + 1]).is_err());
        assert!(validate_argv(&["bad\0argument".to_string()]).is_err());
    }

    #[test]
    fn dependency_inspection_requires_the_same_unrestricted_scope_as_execution() {
        let restricted = ToolExecutionContext::from_run_context(None);
        let error = require_unrestricted_script_runtime(&restricted).unwrap_err();
        assert_eq!(error.code(), Some("skill_script.full_access_required"));

        let run_context = AgentRunContext {
            conversation_id: None,
            project_id: None,
            workspace: None,
            attachment_library: None,
            permissions: AgentPermissions {
                read: AgentReadPermission::All,
                write: AgentWritePermission::All,
                command_safety: AgentCommandSafetyPolicy::FullAccess,
                ..Default::default()
            },
        };
        let unrestricted = ToolExecutionContext::from_run_context(Some(&run_context));
        require_unrestricted_script_runtime(&unrestricted).unwrap();
    }
}
