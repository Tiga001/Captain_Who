use super::*;
use mycopilot_core::workflow_execution::{
    MutationAction, MutationRequest, SendOutput, SendRequest,
};

#[tokio::test]
async fn workflow_runtime_publication_real_storage_drains_final_mail_metadata() {
    let directory = tempdir().unwrap();
    let storage = Arc::new(
        StorageService::open(&directory.path().join("runtime-publication.sqlite")).unwrap(),
    );
    storage.save_model_settings(test_model_settings()).unwrap();
    let (source, _) = workflow_fixture(&storage);
    let service = AgentService::new_authorized_for_test(storage.clone());
    let (notifications, mut events) = crate::transport::outbound_channel();
    let publisher = service
        .start_workflow_runtime_publisher(notifications.clone())
        .unwrap();
    storage
        .upsert_chat_messages(
            &source,
            vec![ChatMessageRecord {
                human_interaction_response: None,
                id: "source-assistant".into(),
                role: "assistant".into(),
                content: String::new(),
                created_at: 1,
                status: Some("pending".into()),
                attachments: vec![],
                folder_references_json: None,
                agent_run_json: None,
                ui_state_json: None,
            }],
            0,
        )
        .unwrap();
    storage
        .append_in_progress_conversation_turn_trace(
            &mycopilot_core::ConversationTraceSnapshot::default().in_progress_trace(
                "source-run",
                &source,
                "source-assistant",
            ),
            1,
            1,
        )
        .unwrap();
    let binding = storage
        .workflow_execution_bind_run(&source, "source-run")
        .unwrap()
        .unwrap();
    let body = "Durable mail body must stay out of runtime notifications. ".repeat(256);
    let receipt = storage
        .workflow_execution_send(&SendRequest {
            model_input: None,
            recipient_versions: Default::default(),
            conversation_id: source.clone(),
            source_run_id: "source-run".into(),
            tool_call_id: "send".into(),
            execution_version: binding.execution_version.clone(),
            messages: vec![SendOutput {
                target_node_id: "b".into(),
                reply_to_message_id: None,
                message: body.clone(),
            }],
        })
        .unwrap();
    // This current-thread test does not yield until shutdown: every committed mutation and
    // dirty hint precedes the worker drain, without relying on timing-sensitive sleeps.
    for _ in 0..100 {
        service.publish_workflow_runtime("instance", &notifications);
    }
    storage
        .workflow_execution_mutate(&MutationRequest {
            conversation_id: source,
            source_run_id: "source-run".into(),
            tool_call_id: "recall".into(),
            execution_version: binding.execution_version,
            action: MutationAction::Recall,
            message_ids: receipt
                .messages
                .iter()
                .map(|message| message.id.clone())
                .collect(),
        })
        .unwrap();
    service.publish_workflow_runtime("instance", &notifications);
    publisher.shutdown(Duration::from_secs(2)).await.unwrap();

    let expected = storage.workflow_execution_runtime("instance").unwrap();
    assert_eq!(expected.inputs.len(), 1);
    assert_eq!(expected.inputs[0].mail_status, MailStatus::Recalled);
    assert_eq!(expected.inputs[0].status, InputStatus::Recalled);
    assert!(expected.inputs[0].content.is_empty());
    assert!(expected.inputs[0].messages[0].content.is_empty());
    let event = events.try_recv().unwrap();
    assert_eq!(event["method"], "agent.workflows.runtime.changed");
    assert_eq!(event["params"], serde_json::to_value(expected).unwrap());
    assert!(
        events.try_recv().is_err(),
        "burst must publish only the final projection"
    );
    let durable = storage
        .workflow_execution_load_input(&receipt.input_ids[0])
        .unwrap()
        .unwrap();
    assert_eq!(durable.mail_status, MailStatus::Recalled);
    assert_eq!(durable.messages[0].content, body);
    assert!(!durable.content.is_empty());
}
