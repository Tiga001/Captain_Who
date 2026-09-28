#[cfg(test)]
mod tree_stop_batch_tests {
    use super::*;
    use mycopilot_core::storage::models::{ChatConversationRecord, ChatMessageRecord};

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

    #[test]
    fn immediate_stop_materializes_root_and_fences_before_runtime_preparation() {
        let fixture = tempfile::tempdir().unwrap();
        let storage =
            Arc::new(StorageService::open(&fixture.path().join("early-stop.sqlite")).unwrap());
        let service = AgentService::new(storage.clone());
        storage
            .save_conversation(ChatConversationRecord {
                id: "early-stop-conversation".into(),
                project_id: None,
                model_id: None,
                title: "Early stop".into(),
                messages: vec![ChatMessageRecord {
                    human_interaction_response: None,
                    id: "early-stop-assistant".into(),
                    role: "assistant".into(),
                    content: String::new(),
                    created_at: 1,
                    status: Some("pending".into()),
                    attachments: Vec::new(),
                    folder_references_json: None,
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
        let trace = mycopilot_core::ConversationTraceSnapshot::default().in_progress_audit_trace(
            "early-stop-run",
            "early-stop-conversation",
            "early-stop-assistant",
        );
        storage
            .append_in_progress_conversation_turn_trace(&trace, 1, 1)
            .unwrap();
        assert!(storage
            .get_agent_node_by_conversation("early-stop-conversation")
            .unwrap()
            .is_none());
        let stop = service
            .begin_descendant_agent_tree_stop("early-stop-run", "early-stop-conversation")
            .unwrap()
            .unwrap();
        assert_eq!(stop.stop.root_run_id, "early-stop-run");
        assert_eq!(
            storage.get_agent_tree_run_stop("early-stop-run").unwrap(),
            Some(stop.stop)
        );
        assert!(storage
            .get_agent_node_by_conversation("early-stop-conversation")
            .unwrap()
            .is_some());
    }
}
