use super::*;
use crate::storage::{
    chat_repository, conversation_trace_repository, workflow_execution_repository,
};
use crate::workflow_execution::DeliveryPresentation;
use rusqlite::params;

fn full_presentations(
    service: &StorageService,
    conversation_id: &str,
    assistant_id: &str,
) -> Vec<DeliveryPresentation> {
    let c = service.state.connection().unwrap();
    let conversation = chat_repository::get_active_conversation(&c, conversation_id)
        .unwrap()
        .unwrap();
    let trace = conversation_trace_repository::get_trace_for_message(&c, assistant_id)
        .unwrap()
        .unwrap();
    let origins =
        workflow_execution_repository::delivery_origins_for_conversation(&c, conversation_id)
            .unwrap();
    workflow_execution_repository::delivery_presentations(&conversation, &trace, &origins)
}

fn add_later_delivery(service: &StorageService) {
    let mut input = service
        .workflow_execution_load_input("input")
        .unwrap()
        .unwrap();
    input.id = "later-input".into();
    input.delivery_id = Some("later-delivery".into());
    input.content = "Later exact host envelope".into();
    input.messages[0].id = "later-source-mail".into();
    input.messages[0].content = "Later exact collaborator body".into();
    let item = ConversationTurnTraceItem::WorkflowDelivery {
        sequence: 3,
        input_id: input.id.clone(),
        instance_id: input.instance_id.clone(),
        workflow_name: "Review".into(),
        content: input.content.clone(),
        created_at: 1,
        truncated: false,
    };
    let c = service.state.connection().unwrap();
    c.execute(
        "INSERT INTO messages(id,conversation_id,role,content,created_at,position)
         VALUES ('later-delivery','mail-chat','user',?1,1,100)",
        [&input.content],
    )
    .unwrap();
    c.execute(
        "INSERT INTO workflow_mail_inputs(input_id,instance_id,execution_version,node_id,
            conversation_id,input_json,status,run_id,delivery_id,created_at,updated_at)
         VALUES ('later-input','workflow','epoch','receiver','mail-chat',?1,
            'completed','run','later-delivery',1,1)",
        [serde_json::to_string(&input).unwrap()],
    )
    .unwrap();
    c.execute(
        "INSERT INTO workflow_mail_message_origins(message_id,conversation_id,input_id)
         VALUES ('later-delivery','mail-chat','later-input')",
        [],
    )
    .unwrap();
    c.execute(
        "INSERT INTO conversation_turn_trace_items(assistant_message_id,sequence,item_kind,item_json)
         VALUES ('assistant',3,'workflow_delivery',?1)",
        [serde_json::to_string(&item).unwrap()],
    ).unwrap();
}

#[test]
fn workflow_delivery_publication_matches_full_projection_and_exclusive_cursor() {
    let fixture = StorageFixture::new();
    let service = fixture.service();
    seed(&service, false, true);
    let initial = service
        .workflow_execution_delivery_presentations("mail-chat", "assistant")
        .unwrap();
    assert_eq!(
        initial,
        full_presentations(&service, "mail-chat", "assistant")
    );
    assert_eq!(initial.len(), 1);
    add_later_delivery(&service);
    let full = full_presentations(&service, "mail-chat", "assistant");
    assert_eq!(full.len(), 2);
    for cursor in [None, Some(0), Some(1), Some(2), Some(3), Some(u64::MAX)] {
        let expected: Vec<_> = full
            .iter()
            .filter(|row| cursor.is_none_or(|s| row.sequence > s))
            .cloned()
            .collect();
        let actual = service
            .workflow_execution_delivery_presentations_since("mail-chat", "assistant", cursor)
            .unwrap();
        assert_eq!(actual, expected);
        assert_eq!(
            actual
                .into_iter()
                .map(|row| serde_json::to_value(row.into_event()).unwrap())
                .collect::<Vec<_>>(),
            expected
                .into_iter()
                .map(|row| serde_json::to_value(row.into_event()).unwrap())
                .collect::<Vec<_>>()
        );
    }
    for (conversation, assistant) in [("missing", "assistant"), ("mail-chat", "missing")] {
        assert!(service
            .workflow_execution_delivery_presentations(conversation, assistant)
            .unwrap()
            .is_empty());
    }
}

#[test]
fn workflow_delivery_publication_does_not_load_unrelated_bodies_or_old_delivery_prefix() {
    let fixture = StorageFixture::new();
    let service = fixture.service();
    seed(&service, false, true);
    add_later_delivery(&service);
    let expected = full_presentations(&service, "mail-chat", "assistant").remove(1);
    let expected_trace_bytes: usize;
    {
        let c = service.state.connection().unwrap();
        // A TEXT decoder would fail on these large unrelated BLOB bodies. A long history must
        // never be pulled into the live notification simply to establish message order.
        c.execute_batch(
            "WITH RECURSIVE n(x) AS (SELECT 1 UNION ALL SELECT x+1 FROM n WHERE x<200)
             INSERT INTO messages(id,conversation_id,role,content,created_at,position)
             SELECT 'old-'||x,'mail-chat','user',zeroblob(16384),x,1000+x FROM n;
             UPDATE conversation_turn_trace_items SET item_json='{}'
                 WHERE assistant_message_id='assistant' AND sequence=1;
             UPDATE conversation_turn_trace_items SET item_json=json_object('invalid',hex(zeroblob(524288)))
                 WHERE assistant_message_id='assistant' AND item_kind='assistant_narration';
             UPDATE workflow_mail_inputs SET input_json='{}' WHERE input_id='input';
             INSERT INTO workflow_mail_inputs(input_id,instance_id,execution_version,node_id,
                 conversation_id,input_json,status,created_at,updated_at)
             VALUES ('unrelated-input','workflow','epoch','receiver','mail-chat',
                 json_object('invalid',hex(zeroblob(524288))),'completed',1,1);
             INSERT INTO workflow_mail_message_origins(message_id,conversation_id,input_id)
             VALUES ('old-1','mail-chat','unrelated-input');"
        ).unwrap();
        expected_trace_bytes = c
            .query_row(
                "SELECT length(item_json) FROM conversation_turn_trace_items
             WHERE assistant_message_id='assistant' AND sequence=3",
                [],
                |row| row.get(0),
            )
            .unwrap();
    }
    crate::storage::trace_performance_metrics::start();
    let actual = service
        .workflow_execution_delivery_presentations_since("mail-chat", "assistant", Some(1))
        .unwrap();
    let metrics = crate::storage::trace_performance_metrics::finish();
    assert_eq!(actual, vec![expected]);
    assert_eq!(metrics.trace_loads, 1);
    assert_eq!(metrics.trace_bytes, expected_trace_bytes);
    assert_eq!(metrics.context_loads, 0);
    assert!(service
        .workflow_execution_delivery_presentations_since("mail-chat", "assistant", Some(3))
        .unwrap()
        .is_empty());
    // Complete restore remains strict: the malformed older proof must not be presented as valid.
    assert!(service
        .workflow_execution_delivery_presentations("mail-chat", "assistant")
        .is_err());
    assert!(service
        .load_conversation_observer_snapshot("mail-chat")
        .is_err());
}

#[test]
fn workflow_delivery_publication_retains_provenance_and_startup_ambiguity_checks() {
    for (startup, proof, truncated) in [
        (true, true, false),
        (false, false, false),
        (false, true, true),
    ] {
        let fixture = StorageFixture::new();
        let service = fixture.service();
        seed_with_trace_truncation(&service, startup, proof, truncated);
        assert!(service
            .workflow_execution_delivery_presentations("mail-chat", "assistant")
            .unwrap()
            .is_empty());
    }
    for mutation in [
        "UPDATE workflow_mail_inputs SET input_json=json_set(input_json,'$.runId','foreign-run') WHERE input_id='input'",
        "UPDATE messages SET content='changed' WHERE id='delivery'",
        "UPDATE workflow_mail_inputs SET input_json=json_set(input_json,'$.instanceId','foreign-instance') WHERE input_id='input'",
        "DELETE FROM workflow_mail_message_origins WHERE message_id='delivery'",
        "INSERT INTO messages(id,conversation_id,role,content,created_at,position) SELECT 'duplicate-delivery',conversation_id,role,content,created_at,100 FROM messages WHERE id='delivery'; INSERT INTO workflow_mail_message_origins VALUES ('duplicate-delivery','mail-chat','input')",
    ] {
        let fixture = StorageFixture::new();
        let service = fixture.service();
        seed(&service, false, true);
        service.state.connection().unwrap().execute_batch(mutation).unwrap();
        let actual = service.workflow_execution_delivery_presentations("mail-chat", "assistant").unwrap();
        assert_eq!(actual, full_presentations(&service, "mail-chat", "assistant"));
        assert!(actual.is_empty(), "mutation: {mutation}");
    }
}

#[test]
fn workflow_delivery_publication_preserves_forked_origins_with_remapped_run_identity() {
    let fixture = StorageFixture::new();
    let service = fixture.service();
    seed(&service, false, true);
    let fork = service
        .fork_conversation_request_view(assistant_reply_fork_request(
            "mail-fork",
            "mail-chat",
            "assistant",
        ))
        .unwrap();
    let assistant = fork
        .conversation
        .messages
        .iter()
        .find(|message| message.role == "assistant")
        .unwrap();
    let actual = service
        .workflow_execution_delivery_presentations(&fork.conversation.id, &assistant.id)
        .unwrap();
    assert_eq!(actual.len(), 1);
    assert_ne!(actual[0].run_id, "run");
    assert_eq!(
        actual,
        full_presentations(&service, &fork.conversation.id, &assistant.id)
    );
}

#[test]
fn workflow_delivery_publication_rejects_mismatched_selected_trace_metadata() {
    let fixture = StorageFixture::new();
    let service = fixture.service();
    seed(&service, false, true);
    service
        .state
        .connection()
        .unwrap()
        .execute(
            "UPDATE conversation_turn_trace_items SET item_json=json_set(item_json,'$.sequence',?1)
         WHERE assistant_message_id='assistant' AND sequence=1",
            params![99],
        )
        .unwrap();
    assert!(service
        .workflow_execution_delivery_presentations("mail-chat", "assistant")
        .is_err());
}
