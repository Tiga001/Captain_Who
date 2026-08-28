/// Stable backend identity for a provider-scoped action id.
///
/// This lightweight helper is shared by the Agent pending-action store and the MCP encrypted
/// payload repository. Keeping it outside the Agent orchestration module lets the reusable
/// core-server library validate envelope bindings without compiling the complete Agent service.
pub(crate) fn pending_action_storage_id(run_id: &str, action_id: &str) -> String {
    mycopilot_core::canonical_pending_action_id(run_id, action_id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn storage_identity_frames_run_and_action_without_ambiguity() {
        assert_eq!(
            pending_action_storage_id("run", "action"),
            "v2:3:run:action"
        );
        assert_ne!(
            pending_action_storage_id("a:b", "c"),
            pending_action_storage_id("a", "b:c")
        );
    }
}
