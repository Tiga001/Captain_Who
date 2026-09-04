#[cfg(test)]
mod tree_stop_batch_tests {
    use super::*;

    #[test]
    fn one_invalid_wake_does_not_prevent_later_runtime_cancellation() {
        let fixture = tempfile::tempdir().unwrap();
        let storage =
            Arc::new(StorageService::open(&fixture.path().join("tree-stop-batch.sqlite")).unwrap());
        let service = AgentService::new(storage);
        let later_cancellation = AgentCancellationToken::new();
        service.register_cancellation("run-later", later_cancellation.clone());

        let progress =
            service.cancel_agent_tree_wake_batch(mycopilot_core::AgentTreeWakeCancellationBatch {
                root_agent_id: "agent-root".to_string(),
                root_conversation_id: "conversation-root".to_string(),
                cancelled_before_admission: Vec::new(),
                active_wakes: vec![
                    mycopilot_core::ActiveAgentTreeWake {
                        agent_id: String::new(),
                        conversation_id: String::new(),
                        wake_id: String::new(),
                        run_id: String::new(),
                        assistant_message_id: String::new(),
                        claim_token: String::new(),
                        status: mycopilot_core::AgentWakeStatus::Running,
                    },
                    mycopilot_core::ActiveAgentTreeWake {
                        agent_id: "agent-later".to_string(),
                        conversation_id: "conversation-later".to_string(),
                        wake_id: "wake-later".to_string(),
                        run_id: "run-later".to_string(),
                        assistant_message_id: "assistant-later".to_string(),
                        claim_token: "claim-later".to_string(),
                        status: mycopilot_core::AgentWakeStatus::Running,
                    },
                ],
            });

        assert!(later_cancellation.is_cancelled());
        assert!(progress.affected);
        assert_eq!(progress.errors.len(), 1);
    }
}
