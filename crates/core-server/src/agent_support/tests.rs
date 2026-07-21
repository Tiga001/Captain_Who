use super::*;
use mycopilot_core::{
    AgentCommandRuntimeBinding, AgentCommandRuntimeKind, AgentCommandRuntimePackageRequirement,
    AgentCommandRuntimeProfile, AgentCommandRuntimeProvider, AgentCommandRuntimeRequest,
    AgentCommandRuntimeResolvedPackage, AGENT_COMMAND_RUNTIME_BINDING_SCHEMA_VERSION,
};

#[test]
fn command_tool_call_projects_only_the_model_visible_runtime_profile() {
    let request = AgentCommandRequest {
        id: "command-runtime-profile-1".to_string(),
        command: "node scripts/build.mjs".to_string(),
        cwd: Some("workspace".to_string()),
        timeout_ms: Some(30_000),
        approval_status: mycopilot_core::AgentApprovalStatus::Required,
        risk_level: None,
        reason: Some("build a presentation".to_string()),
        observe: None,
        runtime: None,
        runtime_binding: Some(Box::new(AgentCommandRuntimeBinding {
            schema_version: AGENT_COMMAND_RUNTIME_BINDING_SCHEMA_VERSION,
            profile: AgentCommandRuntimeProfile::Presentations,
            profile_revision: "artifact-runtime-profile-sha256-v1:test".to_string(),
            provider_id: "mycopilot.artifact-runtime".to_string(),
            bundle_version: "2026.07.3".to_string(),
            bundle_revision: "artifact-runtime-bundle-sha256-v1:test".to_string(),
            kind: AgentCommandRuntimeKind::Node,
            runtime_version: "22.23.1".to_string(),
            runtime_fingerprint: "artifact-runtime-sha256-v1:test".to_string(),
            resolved_packages: vec![AgentCommandRuntimeResolvedPackage {
                name: "pptxgenjs".to_string(),
                version: "4.0.1".to_string(),
            }],
        })),
    };

    let call = command_tool_call(&request);
    assert_eq!(call.args["runtimeProfile"], "presentations");
    assert!(call.args["runtime"].is_null());
    let model_args = serde_json::to_string(&call.args).unwrap();
    assert!(!model_args.contains("pptxgenjs"));
    assert!(!model_args.contains("4.0.1"));
    assert!(!model_args.contains("runtimeFingerprint"));
}

#[test]
fn command_tool_call_keeps_the_frozen_managed_runtime_request() {
    let request = AgentCommandRequest {
        id: "command-runtime-1".to_string(),
        command: "node scripts/build.mjs".to_string(),
        cwd: Some("workspace".to_string()),
        timeout_ms: Some(30_000),
        approval_status: mycopilot_core::AgentApprovalStatus::Required,
        risk_level: None,
        reason: Some("build a workbook".to_string()),
        observe: None,
        runtime: Some(AgentCommandRuntimeRequest {
            provider: AgentCommandRuntimeProvider::ManagedArtifact,
            kind: AgentCommandRuntimeKind::Node,
            required_packages: vec![AgentCommandRuntimePackageRequirement {
                name: "exceljs".to_string(),
                version: "4.4.0".to_string(),
            }],
        }),
        runtime_binding: None,
    };

    let call = command_tool_call(&request);

    assert_eq!(call.args["runtime"]["provider"], "managedArtifact");
    assert_eq!(call.args["runtime"]["kind"], "node");
    assert_eq!(
        call.args["runtime"]["requiredPackages"][0]["name"],
        "exceljs"
    );
    assert_eq!(
        call.args["runtime"]["requiredPackages"][0]["version"],
        "4.4.0"
    );
}

#[test]
fn failed_command_tool_result_keeps_the_complete_execution_observation() {
    let execution = AgentCommandExecutionResult {
        command: "python3 -c 'import openpyxl'".to_string(),
        cwd: ".".to_string(),
        exit_code: Some(1),
        stdout: "dependency check started".to_string(),
        stderr: "ModuleNotFoundError: No module named 'openpyxl'".to_string(),
        timed_out: false,
        cancelled: false,
        duration_ms: 25,
        stdout_truncated: false,
        stderr_truncated: true,
        error: None,
        policy_evaluation: None,
        artifact_observation: None,
        runtime: None,
    };

    let result = command_tool_result("command-1", &execution);
    let observation = result.result.expect("structured command observation");

    assert!(!result.ok);
    assert_eq!(result.error.as_deref(), Some("命令执行失败。"));
    assert_eq!(observation["exitCode"], 1);
    assert_eq!(observation["stdout"], "dependency check started");
    assert_eq!(
        observation["stderr"],
        "ModuleNotFoundError: No module named 'openpyxl'"
    );
    assert_eq!(observation["timedOut"], false);
    assert_eq!(observation["cancelled"], false);
    assert_eq!(observation["stdoutTruncated"], false);
    assert_eq!(observation["stderrTruncated"], true);
}

#[test]
fn policy_rejection_keeps_stable_structured_diagnostics_in_tool_result() {
    use mycopilot_core::command::{
        evaluate_command_policy, CommandAuthorizationSource, CommandPolicyDecision,
    };
    use mycopilot_core::AgentCommandSafetyPolicy;

    let request = AgentCommandRequest {
        id: "command-policy-1".to_string(),
        command: "rm -rf /".to_string(),
        cwd: None,
        timeout_ms: Some(5_000),
        approval_status: mycopilot_core::AgentApprovalStatus::Approved,
        risk_level: None,
        reason: None,
        observe: None,
        runtime: None,
        runtime_binding: None,
    };
    let evaluation = evaluate_command_policy(
        &request.command,
        AgentCommandSafetyPolicy::FullAccess,
        CommandAuthorizationSource::Automatic,
    );
    assert_eq!(evaluation.decision, CommandPolicyDecision::Deny);
    let execution = failed_command_result(
        &request,
        "命令已被安全策略拒绝。".to_string(),
        Some(evaluation),
    );

    let result = command_tool_result(&request.id, &execution);
    let observation = result.result.expect("structured policy observation");

    assert_eq!(observation["policyEvaluation"]["decision"], "deny");
    assert_eq!(
        observation["policyEvaluation"]["code"],
        "command.catastrophic.filesystem_root"
    );
    assert_eq!(
        observation["policyEvaluation"]["findings"][0]["risk"],
        "catastrophic"
    );
}
