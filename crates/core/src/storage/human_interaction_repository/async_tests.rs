use super::*;
use crate::storage::{
    chat_repository, conversation_trace_repository, guidance_repository,
    models::AgentRunGuidanceRecord, service::StorageService,
};

fn accepted(fixture: &Fixture, tool: &str) -> HumanInteractionRequestSnapshot {
    let request = admit_async(&mut fixture.connect(), &owner(tool), &questions(), 10).unwrap();
    submit(&mut fixture.connect(), &submission(&request, tool), 20).unwrap()
}
fn record(request: &HumanInteractionRequestSnapshot, id: &str) -> AgentRunGuidanceRecord {
    AgentRunGuidanceRecord {
        guidance_id: id.into(),
        client_message_id: format!("client-{id}"),
        run_id: "run".into(),
        conversation_id: "chat".into(),
        assistant_message_id: "assistant".into(),
        content: async_human_interaction_answer_content(request).unwrap(),
        status: crate::AgentGuidanceStatus::Queued,
        attachment_ids: vec![],
        folder_references_json: "[]".to_string(),
        applied_trace_sequence: None,
        terminal_reason: None,
        created_at: 21,
        updated_at: 21,
    }
}
fn bind(
    fixture: &Fixture,
    request: &HumanInteractionRequestSnapshot,
    id: &str,
) -> Option<HumanInteractionAsyncBinding> {
    bind_async_to_guidance(
        &mut fixture.connect(),
        &request.response.as_ref().unwrap().response_id,
        &record(request, id),
        request.delivery.as_ref().unwrap().revision,
        21,
    )
    .unwrap()
}
fn reload(
    fixture: &Fixture,
    request: &HumanInteractionRequestSnapshot,
) -> HumanInteractionRequestSnapshot {
    load_request(&fixture.connect(), "chat", &request.request_id).unwrap()
}
fn stop(fixture: &Fixture, run: &str) {
    fixture.connect().execute("INSERT INTO agent_tree_run_stops(run_id,root_run_id,root_agent_id,root_conversation_id,stopped_at) VALUES(?1,?1,'root','chat',40)",[run]).unwrap();
}
fn empty_trace(run: &str, assistant: &str) -> crate::ConversationTurnTrace {
    crate::ConversationTurnTrace {
        schema_version: crate::CONVERSATION_TURN_TRACE_SCHEMA_VERSION,
        run_id: run.into(),
        conversation_id: "chat".into(),
        assistant_message_id: assistant.into(),
        terminal_status: crate::ConversationTurnTraceTerminalStatus::InProgress,
        terminal_error: None,
        truncated: false,
        items: vec![],
    }
}
fn new_turn(
    fixture: &Fixture,
    request: &HumanInteractionRequestSnapshot,
    run: &str,
    assistant: &str,
    user: &str,
    corrupt: bool,
) -> Result<HumanInteractionAsyncBinding> {
    let mut conversation = chat_repository::get_conversation(&fixture.connect(), "chat")
        .unwrap()
        .unwrap();
    conversation.updated_at = 30;
    conversation
        .messages
        .push(crate::storage::models::ChatMessageRecord {
            human_interaction_response: None,
            id: user.into(),
            role: "user".into(),
            content: if corrupt {
                "forged answer".into()
            } else {
                async_human_interaction_answer_content(request).unwrap()
            },
            created_at: 30,
            status: None,
            attachments: vec![],
            folder_references_json: None,
            agent_run_json: None,
            ui_state_json: None,
        });
    conversation
        .messages
        .push(crate::storage::models::ChatMessageRecord {
            human_interaction_response: None,
            id: assistant.into(),
            role: "assistant".into(),
            content: String::new(),
            created_at: 30,
            status: Some("in_progress".into()),
            attachments: vec![],
            folder_references_json: None,
            agent_run_json: None,
            ui_state_json: None,
        });
    let revision: i64 = fixture
        .connect()
        .query_row(
            "SELECT revision FROM conversations WHERE id='chat'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    StorageService::open_for_development_reset(&fixture.path)
        .unwrap()
        .save_human_interaction_conversation_and_begin_turn(
            conversation,
            Some(revision),
            crate::AgentTurnPermissionSource::HostAuthenticatedRoot(Default::default()),
            &[],
            &empty_trace(run, assistant),
            30,
            30,
            &HumanInteractionAsyncTurnAdmission {
                response_id: request.response.as_ref().unwrap().response_id.clone(),
                expected_delivery_revision: request.delivery.as_ref().unwrap().revision,
                user_message_id: user.into(),
            },
        )
        .map(|(_, _, binding)| binding)
        .map_err(unavailable)
}
fn finish_source(fixture: &Fixture) {
    fixture
        .connect()
        .execute(
            "UPDATE conversation_turn_traces SET terminal_status='completed',completed_at=25 WHERE run_id='run'",
            [],
        )
        .unwrap();
}

#[test]
fn async_admission_multiple_batches_and_policy_dedup_are_durable() {
    let fixture = Fixture::new();
    let first = admit_async(&mut fixture.connect(), &owner("a"), &questions(), 10).unwrap();
    let second = admit_async(&mut fixture.connect(), &owner("b"), &questions(), 11).unwrap();
    assert_ne!(first.request_id, second.request_id);
    update_settings(
        &mut fixture.connect(),
        &HumanInteractionSettingsUpdate {
            enabled: false,
            expected_revision: 0,
        },
        12,
    )
    .unwrap();
    finish_source(&fixture);
    assert_eq!(
        admit_async(&mut fixture.connect(), &owner("a"), &questions(), 13).unwrap(),
        first
    );
    assert!(admit_async(&mut fixture.connect(), &owner("c"), &questions(), 14).is_err());
    assert_eq!(count(&fixture.connect(), "human_interaction_requests"), 2);
    assert_eq!(
        count(&fixture.connect(), "human_interaction_async_bindings"),
        0
    );
}

#[test]
fn async_guidance_fifo_claims_share_one_run_and_rejection_releases() {
    let fixture = Fixture::new();
    let first = accepted(&fixture, "a");
    let second = accepted(&fixture, "b");
    assert!(bind(&fixture, &second, "b-early").is_none());
    assert!(bind(&fixture, &first, "a").is_some());
    assert!(bind(&fixture, &first, "a-again").is_none());
    assert!(bind(&fixture, &second, "b").is_some());
    assert_eq!(count(&fixture.connect(), "messages"), 1);
    guidance_repository::mark_guidance_terminal(
        &fixture.connect(),
        "a",
        crate::AgentGuidanceStatus::Rejected,
        "queue closed",
        22,
    )
    .unwrap();
    let pending = reload(&fixture, &first);
    assert_eq!(
        pending.delivery.as_ref().unwrap().status,
        HumanInteractionDeliveryStatus::Pending
    );
    assert!(bind(&fixture, &pending, "a-retry").is_some());
    reconcile_async(&mut fixture.connect(), 25).unwrap();
    assert_eq!(list_pending_async(&fixture.connect()).unwrap().len(), 2);
}

#[test]
fn async_guidance_requires_exact_trace_and_atomically_marks_applied() {
    let fixture = Fixture::new();
    let request = accepted(&fixture, "a");
    bind(&fixture, &request, "g").unwrap();
    assert!(guidance_repository::mark_guidance_applied(&fixture.connect(), "g", 0, 22).is_err());
    let mut trace = empty_trace("run", "assistant");
    let record = record(&request, "g");
    trace
        .items
        .push(crate::ConversationTurnTraceItem::UserGuidance {
            sequence: 0,
            guidance_id: record.guidance_id,
            client_message_id: record.client_message_id,
            content: record.content.clone(),
            attachments: vec![],
            folder_references: Vec::new(),
            created_at: 21,
            truncated: false,
        });
    fixture
        .connect()
        .execute(
            "UPDATE conversation_turn_traces SET schema_version=?1",
            [crate::CONVERSATION_TURN_TRACE_SCHEMA_VERSION],
        )
        .unwrap();
    conversation_trace_repository::commit_trace_in_connection(&fixture.connect(), &trace, 2, 22)
        .unwrap();
    assert!(guidance_repository::mark_guidance_applied(&fixture.connect(), "g", 0, 22).is_err());
    let context = vec![crate::ConversationModelContextItem {
        images: Vec::new(),
        sequence: 0,
        ordinal: 0,
        role: "user".into(),
        content: record.content,
        tool_call_id: None,
        tool_calls: vec![],
        is_error: false,
    }];
    let service = StorageService::open_for_development_reset(&fixture.path).unwrap();
    assert!(service
        .append_in_progress_conversation_turn_trace_and_apply_guidances(&trace, &context, 2, 22)
        .unwrap());
    assert_eq!(
        reload(&fixture, &request).delivery.unwrap().status,
        HumanInteractionDeliveryStatus::Applied
    );
    let revision = reload(&fixture, &request).delivery.unwrap().revision;
    assert!(!service
        .append_in_progress_conversation_turn_trace_and_apply_guidances(&trace, &context, 2, 23)
        .unwrap());
    assert_eq!(
        reload(&fixture, &request).delivery.unwrap().revision,
        revision
    );
    assert!(list_pending_async(&fixture.connect()).unwrap().is_empty());
    assert_eq!(count(&fixture.connect(), "messages"), 1);
}

#[test]
fn async_stop_cancels_only_accepted_scope_and_preserves_open_batches() {
    let fixture = Fixture::new();
    let submitted = accepted(&fixture, "a");
    bind(&fixture, &submitted, "g").unwrap();
    let open = fixture.request(HumanInteractionMode::Async, "b");
    stop(&fixture, "run");
    assert_eq!(
        reload(&fixture, &submitted).delivery.unwrap().status,
        HumanInteractionDeliveryStatus::Cancelled
    );
    assert_eq!(
        reload(&fixture, &open).status,
        HumanInteractionRequestStatus::Open
    );
    let after = submit(&mut fixture.connect(), &submission(&open, "after-stop"), 45).unwrap();
    assert_eq!(
        after.delivery.unwrap().status,
        HumanInteractionDeliveryStatus::Pending
    );
    assert_eq!(list_pending_async(&fixture.connect()).unwrap().len(), 1);
    reconcile_async(&mut fixture.connect(), 46).unwrap();
    assert_eq!(list_pending_async(&fixture.connect()).unwrap().len(), 1);
    assert_eq!(
        guidance_repository::load_guidance(&fixture.connect(), "g")
            .unwrap()
            .unwrap()
            .status,
        crate::AgentGuidanceStatus::Abandoned
    );
}

#[test]
fn async_new_turn_atomic_ownership_rollback_and_safe_prestart_recovery() {
    let fixture = Fixture::new();
    let request = accepted(&fixture, "a");
    finish_source(&fixture);
    assert!(new_turn(&fixture, &request, "bad", "bad-assistant", "bad-user", true).is_err());
    assert_eq!(count(&fixture.connect(), "messages"), 1);
    let binding = new_turn(
        &fixture,
        &request,
        "next",
        "next-assistant",
        "answer-user",
        false,
    )
    .unwrap();
    assert_eq!(
        load_async_binding_for_run(&fixture.connect(), "next")
            .unwrap()
            .unwrap(),
        binding
    );
    assert_eq!(count(&fixture.connect(), "messages"), 3);
    assert_eq!(
        count(&fixture.connect(), "human_interaction_message_projections"),
        1
    );
    let loaded = chat_repository::get_conversation(&fixture.connect(), "chat")
        .unwrap()
        .unwrap();
    let answer = loaded
        .messages
        .iter()
        .find(|message| message.id == "answer-user")
        .unwrap();
    let display = human_interaction_answer_display(&request).unwrap();
    assert_eq!(answer.human_interaction_response.as_ref(), Some(&display));
    let incoming: crate::storage::models::ChatMessageRecord =
        serde_json::from_value(serde_json::to_value(answer).unwrap()).unwrap();
    assert!(
        incoming.human_interaction_response.is_none(),
        "Renderer input cannot certify its own answer proof"
    );
    assert!(fixture
        .connect()
        .execute(
            "UPDATE messages SET content='different answer' WHERE id='answer-user'",
            []
        )
        .is_err());
    reconcile_async(&mut fixture.connect(), 35).unwrap();
    let pending = list_pending_async(&fixture.connect()).unwrap().remove(0);
    assert_eq!(pending.user_message_id.as_deref(), Some("answer-user"));
    assert_eq!(count(&fixture.connect(), "messages"), 2);
    assert_eq!(
        count(&fixture.connect(), "human_interaction_message_projections"),
        0
    );
    assert_eq!(
        conversation_trace_repository::get_trace_for_message(&fixture.connect(), "next-assistant")
            .unwrap()
            .unwrap()
            .terminal_status,
        crate::ConversationTurnTraceTerminalStatus::Failed
    );
    assert!(!start_async_turn(&mut fixture.connect(), &binding, 36).unwrap());
    assert!(new_turn(
        &fixture,
        &pending.request,
        "retry",
        "retry-assistant",
        "wrong-user",
        false
    )
    .is_err());
    assert!(new_turn(
        &fixture,
        &pending.request,
        "retry",
        "retry-assistant",
        "answer-user",
        false
    )
    .is_ok());
}

#[test]
fn async_started_turn_unknown_outcome_fails_closed_and_completed_observation_applies_once() {
    for observed in [false, true] {
        let fixture = Fixture::new();
        let request = accepted(&fixture, "a");
        finish_source(&fixture);
        let binding = new_turn(
            &fixture,
            &request,
            "next",
            "next-assistant",
            "answer-user",
            false,
        )
        .unwrap();
        assert!(start_async_turn(&mut fixture.connect(), &binding, 31).unwrap());
        assert!(!start_async_turn(&mut fixture.connect(), &binding, 32).unwrap());
        if observed {
            fixture.connect().execute("INSERT INTO model_request_observations(id,schema_version,run_id,conversation_id,assistant_message_id,request_index,purpose,model,api_style,status,observation_json,started_at,completed_at) VALUES('observation',3,'next','chat','next-assistant',1,'agent_loop','model','open_ai_compatible','completed','{}',31,33)",[]).unwrap();
        }
        reconcile_async(&mut fixture.connect(), 35).unwrap();
        assert_eq!(
            reload(&fixture, &request).delivery.unwrap().status,
            if observed {
                HumanInteractionDeliveryStatus::Applied
            } else {
                HumanInteractionDeliveryStatus::Failed
            }
        );
        assert!(list_pending_async(&fixture.connect()).unwrap().is_empty());
        assert_eq!(count(&fixture.connect(), "messages"), 3);
    }
}

#[test]
fn async_stop_between_new_turn_admission_and_launch_terminalizes_without_replay() {
    let fixture = Fixture::new();
    let request = accepted(&fixture, "a");
    finish_source(&fixture);
    let binding = new_turn(
        &fixture,
        &request,
        "next",
        "next-assistant",
        "answer-user",
        false,
    )
    .unwrap();
    stop(&fixture, "next");
    assert!(!start_async_turn(&mut fixture.connect(), &binding, 41).unwrap());
    settle_async_start_failure(&mut fixture.connect(), "next", 42).unwrap();
    assert_eq!(
        reload(&fixture, &request).delivery.unwrap().status,
        HumanInteractionDeliveryStatus::Cancelled
    );
    assert_eq!(
        conversation_trace_repository::get_trace_for_message(&fixture.connect(), "next-assistant")
            .unwrap()
            .unwrap()
            .terminal_status,
        crate::ConversationTurnTraceTerminalStatus::Cancelled
    );
    assert_eq!(count(&fixture.connect(), "messages"), 3);
    assert!(list_pending_async(&fixture.connect()).unwrap().is_empty());
}

#[test]
fn async_guidance_concurrent_claim_has_one_winner() {
    let fixture = Fixture::new();
    let request = accepted(&fixture, "a");
    let barrier = Arc::new(Barrier::new(2));
    let results = std::thread::scope(|scope| {
        let handles = (0..2)
            .map(|i| {
                let barrier = barrier.clone();
                let request = &request;
                let fixture = &fixture;
                scope.spawn(move || {
                    barrier.wait();
                    bind(fixture, request, &format!("guidance-{i}")).is_some()
                })
            })
            .collect::<Vec<_>>();
        handles
            .into_iter()
            .map(|h| h.join().unwrap())
            .collect::<Vec<_>>()
    });
    assert_eq!(results.iter().filter(|won| **won).count(), 1);
    assert_eq!(count(&fixture.connect(), "agent_run_guidances"), 1);
}

#[test]
fn async_submit_stop_race_records_the_commit_order_without_reopening() {
    for _ in 0..8 {
        let fixture = Fixture::new();
        let request = fixture.request(HumanInteractionMode::Async, "question");
        let barrier = Arc::new(Barrier::new(2));
        std::thread::scope(|scope| {
            let submit_barrier = barrier.clone();
            let stop_barrier = barrier.clone();
            let fixture_ref = &fixture;
            let request_ref = &request;
            let submitting = scope.spawn(move || {
                submit_barrier.wait();
                submit(
                    &mut fixture_ref.connect(),
                    &submission(request_ref, "response"),
                    40,
                )
                .unwrap()
            });
            let stopping = scope.spawn(move || {
                stop_barrier.wait();
                stop(fixture_ref, "run");
            });
            submitting.join().unwrap();
            stopping.join().unwrap();
        });
        let settled = reload(&fixture, &request);
        assert_eq!(settled.status, HumanInteractionRequestStatus::Submitted);
        let scope: Option<String> = fixture
            .connect()
            .query_row(
                "SELECT stop_scope_run_id FROM human_interaction_async_bindings",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            settled.delivery.unwrap().status,
            if scope.is_some() {
                HumanInteractionDeliveryStatus::Cancelled
            } else {
                HumanInteractionDeliveryStatus::Pending
            }
        );
        assert_eq!(count(&fixture.connect(), "human_interaction_responses"), 1);
        assert_eq!(count(&fixture.connect(), "agent_run_guidances"), 0);
    }
}

#[test]
fn async_answer_fork_copies_verified_history_without_live_questions_permissions_or_usage() {
    let fixture = Fixture::new();
    let request = accepted(&fixture, "answered-call");
    let open = fixture.request(HumanInteractionMode::Async, "still-open-call");
    finish_source(&fixture);
    let binding = new_turn(
        &fixture,
        &request,
        "answer-run",
        "answer-assistant",
        "answer-user",
        false,
    )
    .unwrap();
    assert!(start_async_turn(&mut fixture.connect(), &binding, 31).unwrap());
    let mut connection = fixture.connect();
    connection.execute("INSERT INTO model_request_observations(id,schema_version,run_id,conversation_id,assistant_message_id,request_index,purpose,model,api_style,status,observation_json,started_at,completed_at) VALUES('answer-observation',3,'answer-run','chat','answer-assistant',1,'agent_loop','model','open_ai_compatible','completed','{}',31,33)",[]).unwrap();
    connection
        .execute(
            "UPDATE conversation_turn_traces SET schema_version=?1",
            [crate::CONVERSATION_TURN_TRACE_SCHEMA_VERSION],
        )
        .unwrap();
    let mut trace = empty_trace("answer-run", "answer-assistant");
    trace.terminal_status = crate::ConversationTurnTraceTerminalStatus::Completed;
    conversation_trace_repository::commit_trace_in_connection(&connection, &trace, 30, 34).unwrap();
    connection.execute("INSERT INTO agent_usage_records(id,conversation_id,message_id,run_id,model_id,model_name,created_at,input_tokens,total_tokens,billable_request_count) VALUES('answer-usage','chat','answer-assistant','answer-run','model','model',30,11,11,1)",[]).unwrap();
    assert!(count(&connection, "agent_effective_permission_snapshots") > 0);
    let plan = crate::storage::conversation_fork_repository::build_fork_plan_at_point(
        &connection,
        "fork-actual-answer",
        "chat",
        &crate::storage::models::ConversationForkPoint::AssistantReply {
            assistant_message_id: "answer-assistant".into(),
        },
        40,
    )
    .unwrap();
    crate::storage::conversation_fork_repository::commit_fork_plan(&mut connection, &plan).unwrap();
    let copied = chat_repository::get_conversation(&connection, &plan.target.id)
        .unwrap()
        .unwrap();
    assert_eq!(
        copied
            .messages
            .iter()
            .filter(|message| message.human_interaction_response.is_some())
            .count(),
        1
    );
    assert_eq!(
        copied
            .messages
            .iter()
            .find_map(|message| message.human_interaction_response.as_ref()),
        Some(&human_interaction_answer_display(&request).unwrap())
    );
    for table in [
        "human_interaction_requests",
        "agent_effective_permission_snapshots",
        "agent_usage_records",
    ] {
        let sql = format!("SELECT COUNT(*) FROM {table} WHERE conversation_id=?1");
        assert_eq!(
            connection
                .query_row(&sql, [&plan.target.id], |row| row.get::<_, i64>(0))
                .unwrap(),
            0,
            "{table}"
        );
    }
    assert_eq!(count(&connection, "human_interaction_responses"), 1);
    assert_eq!(count(&connection, "human_interaction_deliveries"), 1);
    assert_eq!(count(&connection, "human_interaction_async_bindings"), 1);
    assert_eq!(
        reload(&fixture, &open).status,
        HumanInteractionRequestStatus::Open
    );
    assert_eq!(
        connection
            .query_row(
                "SELECT SUM(billable_request_count) FROM agent_usage_records",
                [],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
        1
    );
}
