use super::*;

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
    };

    let result = command_tool_result("command-1", false, &execution);
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

    let result = command_tool_result(&request.id, false, &execution);
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
