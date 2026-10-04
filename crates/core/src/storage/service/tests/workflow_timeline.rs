use super::*;
use crate::{
    ConversationTurnTrace, ConversationTurnTraceItem, ConversationTurnTraceTerminalStatus,
};
use serde_json::{json, Value};

thread_local! {
    static HISTORY_QUERY_COUNTS: std::cell::Cell<[usize; 3]> = const { std::cell::Cell::new([0; 3]) };
}

fn count_history_queries(sql: &str) {
    let index = if sql.contains("FROM conversation_turn_traces AS trace")
        && sql.contains("trace.completed_at")
    {
        Some(0)
    } else if sql.contains("FROM conversation_turn_trace_items AS item") {
        Some(1)
    } else if sql.contains("FROM workflow_mail_message_origins o JOIN workflow_mail_inputs") {
        Some(2)
    } else {
        None
    };
    if let Some(index) = index {
        HISTORY_QUERY_COUNTS.with(|counts| {
            let mut next = counts.get();
            next[index] += 1;
            counts.set(next);
        });
    }
}

fn seed(service: &StorageService, startup: bool, proof: bool) {
    seed_with_trace_truncation(service, startup, proof, false);
}

fn seed_with_trace_truncation(
    service: &StorageService,
    startup: bool,
    proof: bool,
    truncated: bool,
) {
    let mut chat = conversation("mail-chat", None, "human");
    let mut assistant = chat.messages[0].clone();
    assistant.id = "assistant".into();
    assistant.role = "assistant".into();
    assistant.content = "Final answer".into();
    assistant.created_at = 100;
    let mut mail = chat.messages[0].clone();
    mail.id = "delivery".into();
    mail.content = "Host envelope containing exact letter".into();
    // Deliberately older than the assistant: timestamps are not consumption positions.
    mail.created_at = 2;
    if startup {
        chat.messages.push(mail.clone());
    }
    chat.messages.push(assistant);
    if !startup {
        chat.messages.push(mail);
    }
    service.save_conversation(chat).unwrap();
    let input = json!({
        "id":"input", "instanceId":"workflow", "nodeId":"receiver", "conversationId":"mail-chat",
        "executionVersion":"epoch", "status":"completed", "mailStatus":"processed", "runId":"run",
        "deliveryId":"delivery", "content":"Host envelope containing exact letter", "createdAt":2, "error":null,
        "messages":[{
            "id":"source-mail", "instanceId":"workflow", "workflowName":"Review", "sourceNodeId":"writer",
            "sourceNodeName":"Writer", "sourceConversationId":"writer-chat", "sourceConversationTitle":"Draft",
            "targetNodeId":"receiver", "targetNodeName":"Reviewer", "targetConversationId":"mail-chat",
            "targetConversationTitle":"Review", "replyToMessageId":null, "content":"Exact collaborator body", "createdAt":2
        }]
    });
    {
        let connection = service.state.connection().unwrap();
        connection.execute("INSERT INTO workflow_mail_inputs(input_id,instance_id,execution_version,node_id,conversation_id,input_json,status,run_id,delivery_id,created_at,updated_at) VALUES ('input','workflow','epoch','receiver','mail-chat',?1,'completed','run','delivery',2,2)", [input.to_string()]).unwrap();
        connection.execute("INSERT INTO workflow_mail_messages(message_id,instance_id,execution_version,node_id,recipient_conversation_id,mail_status,message_json,input_id,created_at) VALUES ('source-mail','workflow','epoch','receiver','mail-chat','processed',?1,'input',2)", [input["messages"][0].to_string()]).unwrap();
        connection.execute("INSERT INTO workflow_mail_message_origins(message_id,conversation_id,input_id) VALUES ('delivery','mail-chat','input')", []).unwrap();
    }
    let mut trace = ConversationTurnTrace {
        schema_version: crate::CONVERSATION_TURN_TRACE_SCHEMA_VERSION,
        run_id: "run".into(),
        conversation_id: "mail-chat".into(),
        assistant_message_id: "assistant".into(),
        terminal_status: ConversationTurnTraceTerminalStatus::Completed,
        terminal_error: None,
        truncated,
        items: vec![ConversationTurnTraceItem::AssistantNarration {
            sequence: 0,
            content: "Before acceptance".into(),
            provider_turn_id: None,
            first_tool_call_id: None,
            truncated: false,
        }],
    };
    if proof {
        trace
            .items
            .push(ConversationTurnTraceItem::WorkflowDelivery {
                sequence: 1,
                input_id: "input".into(),
                instance_id: "workflow".into(),
                workflow_name: "Review".into(),
                content: input["content"].as_str().unwrap().into(),
                created_at: 2,
                truncated,
            });
        trace
            .items
            .push(ConversationTurnTraceItem::AssistantNarration {
                sequence: 2,
                content: "After acceptance".into(),
                provider_turn_id: None,
                first_tool_call_id: None,
                truncated: false,
            });
    }
    service
        .replace_conversation_turn_trace(&trace, 100, 200)
        .unwrap();
}

fn deliveries(chat: &ChatConversationRecord) -> Vec<Value> {
    chat.messages
        .iter()
        .filter_map(|message| message.agent_run_json.as_ref())
        .flat_map(|run| {
            serde_json::from_str::<Value>(run).unwrap()["timeline"]
                .as_array()
                .unwrap()
                .clone()
        })
        .filter(|item| item["type"] == "workflow_delivery")
        .collect()
}

#[test]
fn workflow_timeline_view_reuses_one_durable_trace_and_origin_read() {
    for bulk in [false, true] {
        let fixture = StorageFixture::new();
        let service = fixture.service();
        seed(&service, false, true);
        HISTORY_QUERY_COUNTS.with(|counts| counts.set([0; 3]));
        service
            .state
            .connection()
            .unwrap()
            .trace(Some(count_history_queries));
        let view = if bulk {
            service.load_conversation_views().unwrap().remove(0)
        } else {
            service
                .load_conversation_view("mail-chat")
                .unwrap()
                .unwrap()
        };
        service.state.connection().unwrap().trace(None);
        assert_eq!(
            HISTORY_QUERY_COUNTS.with(std::cell::Cell::get),
            [1, 1, 1],
            "trace headers, trace bodies and mail origins must each be read only once"
        );
        assert_eq!(deliveries(&view.conversation).len(), 1);
        assert!(!view
            .conversation
            .messages
            .iter()
            .any(|m| m.id == "delivery"));
        // The evidence is presentation-only; internal and admission history retain the letter.
        assert!(service
            .load_conversation("mail-chat")
            .unwrap()
            .unwrap()
            .messages
            .iter()
            .any(|m| m.id == "delivery"));
        assert!(service
            .load_conversation_for_turn("mail-chat")
            .unwrap()
            .0
            .unwrap()
            .messages
            .iter()
            .any(|m| m.id == "delivery"));
    }
}

#[test]
fn workflow_timeline_reload_save_reload_preserves_raw_mail_and_consumption_order() {
    let fixture = StorageFixture::new();
    let service = fixture.service();
    seed(&service, false, true);
    let raw = service.load_conversation("mail-chat").unwrap().unwrap();
    assert_eq!(raw.messages.len(), 3);
    let view = service
        .load_conversation_view("mail-chat")
        .unwrap()
        .unwrap()
        .conversation;
    assert_eq!(
        view.messages
            .iter()
            .map(|m| m.id.as_str())
            .collect::<Vec<_>>(),
        vec!["human", "assistant"]
    );
    let rendered = deliveries(&view);
    assert_eq!(rendered.len(), 1);
    assert_eq!(rendered[0]["deliveryId"], "delivery");
    assert_eq!(rendered[0]["traceSequence"], 1);
    assert_eq!(
        rendered[0]["sources"][0]["content"],
        "Exact collaborator body"
    );
    let run: Value =
        serde_json::from_str(view.messages[1].agent_run_json.as_ref().unwrap()).unwrap();
    assert_eq!(run["timeline"][0]["content"], "Before acceptance");
    assert_eq!(run["timeline"][1]["type"], "workflow_delivery");
    assert_eq!(run["timeline"][2]["content"], "After acceptance");
    service.save_conversation(view).unwrap();
    let reloaded = service
        .load_conversation_view("mail-chat")
        .unwrap()
        .unwrap()
        .conversation;
    assert_eq!(deliveries(&reloaded), rendered);
    let mut next = reloaded;
    let mut human = next.messages[0].clone();
    human.id = "next-user".into();
    human.content = "Next task".into();
    human.created_at = 300;
    next.messages.push(human);
    service.save_conversation(next).unwrap();
    let raw = service.load_conversation("mail-chat").unwrap().unwrap();
    assert_eq!(
        raw.messages
            .iter()
            .map(|m| m.id.as_str())
            .collect::<Vec<_>>(),
        vec!["human", "assistant", "delivery", "next-user"]
    );
    assert_eq!(deliveries(&raw), rendered);
    assert_eq!(
        service
            .workflow_execution_load_input("input")
            .unwrap()
            .unwrap()
            .mail_status,
        crate::workflow_execution::MailStatus::Processed
    );
}

#[test]
fn workflow_timeline_preserves_startup_and_unproven_or_mismatched_messages() {
    for (startup, proof) in [(true, true), (false, false)] {
        let fixture = StorageFixture::new();
        let service = fixture.service();
        seed(&service, startup, proof);
        let view = service
            .load_conversation_view("mail-chat")
            .unwrap()
            .unwrap()
            .conversation;
        assert!(view.messages.iter().any(|m| m.id == "delivery"));
        assert!(deliveries(&view).is_empty());
    }
    let fixture = StorageFixture::new();
    let service = fixture.service();
    seed(&service, false, true);
    {
        let c = service.state.connection().unwrap();
        c.execute("UPDATE workflow_mail_inputs SET input_json=json_set(input_json,'$.runId','another-run') WHERE input_id='input'",[]).unwrap();
    }
    let view = service
        .load_conversation_view("mail-chat")
        .unwrap()
        .unwrap()
        .conversation;
    assert!(view.messages.iter().any(|m| m.id == "delivery"));
    assert!(deliveries(&view).is_empty());
}

#[test]
fn workflow_timeline_assistant_fork_carries_only_consumed_mail_attachments() {
    let fixture = StorageFixture::new();
    let service = fixture.service();
    seed(&service, false, true);
    let mut chat = service.load_conversation("mail-chat").unwrap().unwrap();
    let mut later = chat.messages[0].clone();
    later.id = "future-human".into();
    later.content = "Do not copy future task".into();
    later.created_at = 300;
    chat.messages.push(later);
    service.save_conversation(chat).unwrap();
    let fork = service
        .fork_conversation_request_view(assistant_reply_fork_request(
            "mail-fork",
            "mail-chat",
            "assistant",
        ))
        .unwrap()
        .conversation;
    assert!(fork
        .messages
        .iter()
        .all(|m| m.content != "Do not copy future task"));
    let copied = deliveries(&fork);
    assert_eq!(copied.len(), 1);
    assert_eq!(
        copied[0]["sources"][0]["content"],
        "Exact collaborator body"
    );
    assert_ne!(copied[0]["deliveryId"], "delivery");
    assert_eq!(fork.messages.len(), 2);
    let raw = service.load_conversation(&fork.id).unwrap().unwrap();
    assert_eq!(
        raw.messages.len(),
        3,
        "fork keeps its own hidden delivery row and provenance"
    );
    assert_eq!(deliveries(&raw), copied);
    let reloaded = service
        .load_conversation_view(&fork.id)
        .unwrap()
        .unwrap()
        .conversation;
    assert_eq!(deliveries(&reloaded), copied);
}

#[test]
fn workflow_timeline_refuses_rewriting_original_letter_through_conversation_save() {
    let fixture = StorageFixture::new();
    let service = fixture.service();
    seed(&service, false, true);
    let mut raw = service.load_conversation("mail-chat").unwrap().unwrap();
    raw.messages
        .iter_mut()
        .find(|m| m.id == "delivery")
        .unwrap()
        .content = "Forged body".into();
    assert!(service.save_conversation(raw).is_err());
    let view = service
        .load_conversation_view("mail-chat")
        .unwrap()
        .unwrap()
        .conversation;
    assert_eq!(
        deliveries(&view)[0]["sources"][0]["content"],
        "Exact collaborator body"
    );
}

#[test]
fn workflow_timeline_discards_cached_delivery_without_durable_authority() {
    for remove_trace in [true, false] {
        let fixture = StorageFixture::new();
        let service = fixture.service();
        seed(&service, false, true);
        let view = service
            .load_conversation_view("mail-chat")
            .unwrap()
            .unwrap()
            .conversation;
        assert_eq!(deliveries(&view).len(), 1);
        service.save_conversation(view).unwrap();
        {
            let connection = service.state.connection().unwrap();
            if remove_trace {
                connection.execute("DELETE FROM conversation_turn_traces WHERE assistant_message_id='assistant'", []).unwrap();
            } else {
                connection
                    .execute(
                        "DELETE FROM workflow_mail_message_origins WHERE message_id='delivery'",
                        [],
                    )
                    .unwrap();
            }
        }
        let view = service
            .load_conversation_view("mail-chat")
            .unwrap()
            .unwrap()
            .conversation;
        assert!(
            deliveries(&view).is_empty(),
            "cached display has no authority without its trace and origin"
        );
        assert!(view.messages.iter().any(|message| message.id == "delivery"));
    }
}

#[test]
fn workflow_timeline_keeps_top_level_letter_when_consumption_trace_is_truncated() {
    let fixture = StorageFixture::new();
    let service = fixture.service();
    seed_with_trace_truncation(&service, false, true, true);
    let view = service
        .load_conversation_view("mail-chat")
        .unwrap()
        .unwrap()
        .conversation;
    assert!(view.messages.iter().any(|message| message.id == "delivery"));
    assert!(deliveries(&view).is_empty());
}
