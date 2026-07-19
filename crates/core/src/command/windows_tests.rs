use super::*;

#[test]
fn windows_execution_fails_closed_until_native_containment_exists() {
    for (safety, source) in [
        (
            AgentCommandSafetyPolicy::Guarded,
            CommandAuthorizationSource::Automatic,
        ),
        (
            AgentCommandSafetyPolicy::FullAccess,
            CommandAuthorizationSource::Automatic,
        ),
        (
            AgentCommandSafetyPolicy::Guarded,
            CommandAuthorizationSource::ExplicitUser,
        ),
    ] {
        let evaluation = evaluate_command_policy("echo hello", safety, source);
        assert_eq!(evaluation.decision, CommandPolicyDecision::Deny);
        assert_eq!(evaluation.code, "command.unsupported.windows_execution");
    }
}
