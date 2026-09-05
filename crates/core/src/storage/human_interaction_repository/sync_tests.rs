use super::*;

fn admission(tool: &str) -> HumanInteractionSyncAdmission {
    HumanInteractionSyncAdmission {
        owner: owner(tool),
        input: questions(),
        checkpoint: serde_json::json!({"version":1,"privateResume":{"runId":"run","toolCallId":tool}}),
        usage: Some(crate::storage::models::AgentUsageRecordInsert {
            id: "usage".into(),
            conversation_id: "chat".into(),
            message_id: "assistant".into(),
            run_id: "run".into(),
            project_id: None,
            model_id: "model-a".into(),
            model_name: "model-a".into(),
            started_at: Some(2),
            completed_at: None,
            status: Some("waiting_for_user_input".into()),
            error: None,
            created_at: 2,
            input_tokens: Some(8),
            output_tokens: Some(2),
            output_thinking_tokens: None,
            total_tokens: Some(10),
            cached_input_tokens: None,
            cache_creation_input_tokens: None,
            billable_request_count: 1,
            input_price: None,
            cached_input_price: None,
            output_price: None,
            estimated_cost: None,
        }),
        content: "Question pending".into(),
        predecessor: None,
        pending_action_predecessor: None,
    }
}
fn stop(connection: &Connection, now: i64) {
    connection.execute("INSERT INTO agent_tree_run_stops(run_id,root_run_id,root_agent_id,root_conversation_id,stopped_at) VALUES('run','run','root','chat',?1)",[now]).unwrap();
}
fn accepted(fixture: &Fixture) -> HumanInteractionRequestSnapshot {
    let request = admit_sync(&mut fixture.connect(), &admission("call"), 10).unwrap();
    submit(&mut fixture.connect(), &submission(&request, "submit"), 11).unwrap()
}
fn status(
    fixture: &Fixture,
    request: &HumanInteractionRequestSnapshot,
) -> HumanInteractionRequestSnapshot {
    load_request(&fixture.connect(), "chat", &request.request_id).unwrap()
}

#[test]
fn sync_admission_atomically_freezes_checkpoint_usage_and_wait_without_user_message() {
    let fixture = Fixture::new();
    let request = admit_sync(&mut fixture.connect(), &admission("call"), 10).unwrap();
    let connection = fixture.connect();
    assert_eq!(request.status, HumanInteractionRequestStatus::Open);
    assert!(has_sync_wait(&connection, "chat").unwrap());
    assert!(is_sync_run_waiting(&connection, "run").unwrap());
    let (run,users,usage):(String,i64,i64)=connection.query_row("SELECT json_extract(agent_run_json,'$.status'),(SELECT COUNT(*) FROM messages WHERE role='user'),(SELECT billable_request_count FROM agent_usage_records) FROM messages WHERE id='assistant'",[],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).unwrap();
    assert_eq!(run, "waiting_for_user_input");
    assert_eq!(users, 0);
    assert_eq!(usage, 1);
    assert_eq!(
        load_sync_for_run(&connection, "run").unwrap().unwrap().1,
        admission("call").checkpoint
    );
}

#[test]
fn sync_invalid_owner_or_usage_rolls_back_every_admission_fact() {
    let fixture = Fixture::new();
    let mut input = admission("call");
    input.usage.as_mut().unwrap().run_id = "wrong".into();
    assert!(admit_sync(&mut fixture.connect(), &input, 10).is_err());
    input.usage = None;
    input.owner.agent_id = "wrong".into();
    assert!(admit_sync(&mut fixture.connect(), &input, 10).is_err());
    let connection = fixture.connect();
    let (requests,suspensions,usage):(i64,i64,i64)=connection.query_row("SELECT (SELECT COUNT(*) FROM human_interaction_requests),(SELECT COUNT(*) FROM human_interaction_suspensions),(SELECT COUNT(*) FROM agent_usage_records)",[],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).unwrap();
    assert_eq!((requests, suspensions, usage), (0, 0, 0));
}

#[test]
fn sync_late_wait_cannot_rewind_submission_claim_or_usage() {
    let fixture = Fixture::new();
    let request = accepted(&fixture);
    let resume = claim_sync(&mut fixture.connect(), &request.request_id, 12)
        .unwrap()
        .unwrap();
    assert!(advance_sync(
        &mut fixture.connect(),
        &resume.binding,
        HumanInteractionSyncTransition::ExecutionStarted,
        13
    )
    .unwrap());
    let mut late = admission("call");
    late.content = "late stale content".into();
    late.usage.as_mut().unwrap().billable_request_count = 99;
    let replay = admit_sync(&mut fixture.connect(), &late, 14).unwrap();
    assert_eq!(replay.status, HumanInteractionRequestStatus::Submitted);
    assert_eq!(
        replay.delivery.unwrap().status,
        HumanInteractionDeliveryStatus::Bound
    );
    let (state,count,content):(String,i64,String)=fixture.connect().query_row("SELECT json_extract(agent_run_json,'$.status'),(SELECT billable_request_count FROM agent_usage_records),content FROM messages WHERE id='assistant'",[],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).unwrap();
    assert_eq!(
        (state.as_str(), count, content.as_str()),
        ("running", 1, "Question pending")
    );
}

#[test]
fn sync_claim_exclusivity_binding_and_pre_execution_restart() {
    let fixture = Fixture::new();
    let request = accepted(&fixture);
    let old = claim_sync(&mut fixture.connect(), &request.request_id, 12)
        .unwrap()
        .unwrap();
    assert!(claim_sync(&mut fixture.connect(), &request.request_id, 12)
        .unwrap()
        .is_none());
    let reconciled = reconcile_sync(&mut fixture.connect(), 13).unwrap();
    assert_eq!(
        reconciled[0].delivery.as_ref().unwrap().status,
        HumanInteractionDeliveryStatus::Pending
    );
    let new = claim_sync(&mut fixture.connect(), &request.request_id, 14)
        .unwrap()
        .unwrap();
    assert_ne!(old.binding.claim_id, new.binding.claim_id);
    assert!(!advance_sync(
        &mut fixture.connect(),
        &old.binding,
        HumanInteractionSyncTransition::ExecutionStarted,
        15
    )
    .unwrap());
    let mut forged = new.binding.clone();
    forged.response_id = "foreign-response".into();
    assert!(!advance_sync(
        &mut fixture.connect(),
        &forged,
        HumanInteractionSyncTransition::ExecutionStarted,
        15
    )
    .unwrap());
    assert!(advance_sync(
        &mut fixture.connect(),
        &new.binding,
        HumanInteractionSyncTransition::ExecutionStarted,
        15
    )
    .unwrap());
}

#[test]
fn sync_restart_distinguishes_saved_answers_from_unknown_executing_and_model_request() {
    for in_flight in [false, true] {
        let fixture = Fixture::new();
        let request = accepted(&fixture);
        assert_eq!(list_sync_ready(&fixture.connect()).unwrap().len(), 1);
        reconcile_sync(&mut fixture.connect(), 12).unwrap();
        assert_eq!(list_sync_ready(&fixture.connect()).unwrap().len(), 1);
        let resume = claim_sync(&mut fixture.connect(), &request.request_id, 13)
            .unwrap()
            .unwrap();
        advance_sync(
            &mut fixture.connect(),
            &resume.binding,
            HumanInteractionSyncTransition::ExecutionStarted,
            14,
        )
        .unwrap();
        if in_flight {
            assert!(advance_sync(
                &mut fixture.connect(),
                &resume.binding,
                HumanInteractionSyncTransition::ModelInFlight,
                15
            )
            .unwrap());
        }
        let result = reconcile_sync(&mut fixture.connect(), 16).unwrap();
        let delivery = result[0].delivery.as_ref().unwrap();
        assert_eq!(delivery.status, HumanInteractionDeliveryStatus::Failed);
        assert_eq!(
            delivery.error_code.as_deref(),
            Some("resume_outcome_unknown")
        );
        assert!(result[0].response.is_some());
        assert!(!has_sync_wait(&fixture.connect(), "chat").unwrap());
        assert!(claim_sync(&mut fixture.connect(), &request.request_id, 17)
            .unwrap()
            .is_none());
    }
}

#[test]
fn sync_stop_fence_cancels_open_or_bound_and_beats_late_submit_resume() {
    for submitted in [false, true] {
        let fixture = Fixture::new();
        let request = admit_sync(&mut fixture.connect(), &admission("call"), 10).unwrap();
        let resume = if submitted {
            submit(&mut fixture.connect(), &submission(&request, "submit"), 11).unwrap();
            claim_sync(&mut fixture.connect(), &request.request_id, 12).unwrap()
        } else {
            None
        };
        stop(&fixture.connect(), 13);
        assert!(admit_sync(&mut fixture.connect(), &admission("late"), 14).is_err());
        if let Some(resume) = resume {
            assert!(!advance_sync(
                &mut fixture.connect(),
                &resume.binding,
                HumanInteractionSyncTransition::ExecutionStarted,
                14
            )
            .unwrap());
            assert_eq!(
                status(&fixture, &request).delivery.unwrap().status,
                HumanInteractionDeliveryStatus::Cancelled
            );
        } else {
            assert!(submit(&mut fixture.connect(), &submission(&request, "late"), 14).is_err());
            assert_eq!(
                status(&fixture, &request).status,
                HumanInteractionRequestStatus::Cancelled
            );
        }
        assert!(load_sync_for_run(&fixture.connect(), "run")
            .unwrap()
            .is_some());
    }
}

#[test]
fn sync_submit_and_stop_concurrent_commit_orders_leave_no_resumable_answer() {
    let fixture = Fixture::new();
    let request = admit_sync(&mut fixture.connect(), &admission("call"), 10).unwrap();
    let barrier = Arc::new(Barrier::new(2));
    let submit_barrier = barrier.clone();
    let path = fixture.path.clone();
    let input = submission(&request, "submit");
    let submitter = std::thread::spawn(move || {
        let mut c = Connection::open(path).unwrap();
        c.busy_timeout(std::time::Duration::from_secs(5)).unwrap();
        submit_barrier.wait();
        submit(&mut c, &input, 11)
    });
    barrier.wait();
    stop(&fixture.connect(), 12);
    let _ = submitter.join().unwrap();
    assert!(!has_sync_wait(&fixture.connect(), "chat").unwrap());
    assert!(claim_sync(&mut fixture.connect(), &request.request_id, 13)
        .unwrap()
        .is_none());
    let final_request = status(&fixture, &request);
    assert!(
        final_request.status == HumanInteractionRequestStatus::Cancelled
            || final_request.delivery.unwrap().status == HumanInteractionDeliveryStatus::Cancelled
    );
}

#[test]
fn sync_successor_checkpoint_and_predecessor_delivery_commit_together() {
    let fixture = Fixture::new();
    let request = accepted(&fixture);
    let resume = claim_sync(&mut fixture.connect(), &request.request_id, 12)
        .unwrap()
        .unwrap();
    advance_sync(
        &mut fixture.connect(),
        &resume.binding,
        HumanInteractionSyncTransition::ExecutionStarted,
        13,
    )
    .unwrap();
    let mut next = admission("next-call");
    next.predecessor = Some(resume.binding.clone());
    next.usage.as_mut().unwrap().total_tokens = Some(15);
    next.owner.agent_id = "wrong".into();
    assert!(admit_sync(&mut fixture.connect(), &next, 14).is_err());
    assert_eq!(
        status(&fixture, &request).delivery.unwrap().status,
        HumanInteractionDeliveryStatus::Bound
    );
    next.owner.agent_id = "root".into();
    let admitted = admit_sync(&mut fixture.connect(), &next, 15).unwrap();
    assert_ne!(admitted.request_id, request.request_id);
    assert_eq!(
        status(&fixture, &request).delivery.unwrap().status,
        HumanInteractionDeliveryStatus::Applied
    );
    assert_eq!(
        fixture
            .connect()
            .query_row("SELECT total_tokens FROM agent_usage_records", [], |r| r
                .get::<_, i64>(0))
            .unwrap(),
        15
    );
    assert!(admit_sync(&mut fixture.connect(), &next, 16).is_ok());
}

#[test]
fn sync_stop_keeps_existing_async_batches_untouched() {
    let fixture = Fixture::new();
    let asynchronous = fixture.request(HumanInteractionMode::Async, "async");
    admit_sync(&mut fixture.connect(), &admission("sync"), 11).unwrap();
    stop(&fixture.connect(), 12);
    assert_eq!(
        status(&fixture, &asynchronous).status,
        HumanInteractionRequestStatus::Open
    );
    assert!(submit(
        &mut fixture.connect(),
        &submission(&asynchronous, "async-answer"),
        13
    )
    .is_ok());
}

#[test]
fn sync_approval_predecessor_requires_durable_settlement_and_rolls_back_on_conflict() {
    let fixture = Fixture::new();
    fixture.connect().execute("INSERT INTO agent_pending_actions(action_id,run_id,conversation_id,assistant_message_id,action_type,tool_name,tool_call_id,status,target_status,action_json,agent_input_json,created_at,updated_at) VALUES('approval','run','chat','assistant','command','run_command','command-call','executing',NULL,'{}','{}',2,2)",[]).unwrap();
    let mut next = admission("after-approval");
    next.pending_action_predecessor = Some(HumanInteractionApprovalPredecessor {
        storage_id: "approval".into(),
        renderer_action_id: "command-call".into(),
        expected_status: "executing".into(),
        terminal_status: "completed".into(),
        terminal_agent_input_json: "{}".into(),
    });
    assert!(admit_sync(&mut fixture.connect(), &next, 10).is_err());
    assert_eq!(count(&fixture.connect(), "human_interaction_requests"), 0);
    fixture
        .connect()
        .execute(
            "UPDATE agent_pending_actions SET target_status='completed' WHERE action_id='approval'",
            [],
        )
        .unwrap();
    admit_sync(&mut fixture.connect(), &next, 11).unwrap();
    assert_eq!(
        fixture
            .connect()
            .query_row(
                "SELECT status FROM agent_pending_actions WHERE action_id='approval'",
                [],
                |r| r.get::<_, String>(0)
            )
            .unwrap(),
        "completed"
    );
}

fn terminal_trace(fixture: &Fixture, request: &HumanInteractionRequestSnapshot) {
    let trace = crate::ConversationTurnTrace {
        schema_version: crate::CONVERSATION_TURN_TRACE_SCHEMA_VERSION,
        run_id: "run".into(),
        conversation_id: "chat".into(),
        assistant_message_id: "assistant".into(),
        terminal_status: crate::ConversationTurnTraceTerminalStatus::Completed,
        terminal_error: None,
        truncated: false,
        items: vec![
            crate::ConversationTurnTraceItem::ToolCall {
                sequence: 0,
                call_id: request.tool_call_id.clone(),
                tool: "request_user_input".into(),
                provenance: crate::AgentToolIdentity::Builtin {
                    tool_name: "request_user_input".into(),
                },
                operation: serde_json::json!({"questions":[]}),
                approval_status: crate::AgentApprovalStatus::NotRequired,
                truncated: false,
            },
            crate::ConversationTurnTraceItem::ToolResult {
                sequence: 1,
                call_id: request.tool_call_id.clone(),
                tool: "request_user_input".into(),
                status: crate::ConversationTraceToolResultStatus::Succeeded,
                success: true,
                observation: serde_json::json!({"type":"human_interaction_response","schemaVersion":1,"requestId":request.request_id,"responseId":request.response.as_ref().unwrap().response_id,"answers":[]}),
                approval_status: crate::AgentApprovalStatus::NotRequired,
                error: None,
                truncated: false,
                archive: Default::default(),
            },
        ],
    };
    fixture
        .connect()
        .execute(
            "UPDATE conversation_turn_traces SET schema_version=?1",
            [crate::CONVERSATION_TURN_TRACE_SCHEMA_VERSION],
        )
        .unwrap();
    crate::storage::conversation_trace_repository::replace_trace(
        &mut fixture.connect(),
        &trace,
        2,
        20,
    )
    .unwrap();
}

#[test]
fn sync_terminal_consumption_is_applied_after_terminal_commit_and_during_restart() {
    for restart in [false, true] {
        let fixture = Fixture::new();
        let request = accepted(&fixture);
        let resume = claim_sync(&mut fixture.connect(), &request.request_id, 12)
            .unwrap()
            .unwrap();
        advance_sync(
            &mut fixture.connect(),
            &resume.binding,
            HumanInteractionSyncTransition::ExecutionStarted,
            13,
        )
        .unwrap();
        terminal_trace(&fixture, &request);
        if restart {
            reconcile_sync(&mut fixture.connect(), 21).unwrap();
        } else {
            assert!(advance_sync(
                &mut fixture.connect(),
                &resume.binding,
                HumanInteractionSyncTransition::Applied,
                21
            )
            .unwrap());
        }
        assert_eq!(
            status(&fixture, &request).delivery.unwrap().status,
            HumanInteractionDeliveryStatus::Applied
        );
    }
}
