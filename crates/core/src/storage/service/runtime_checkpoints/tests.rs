use super::*;

fn fixture() -> (tempfile::TempDir, StorageService, ChatConversationRecord) {
    let directory = tempfile::tempdir().unwrap();
    let service = StorageService::open_with_model_credentials(
        &directory.path().join("checkpoint.sqlite"),
        Arc::new(crate::image_generation::InMemoryCredentialStore::default()),
    )
    .unwrap();
    let projection = chat_repository::canonical_agent_run_lifecycle_projection(
        None, "run", "running", 1, 1, None,
    )
    .unwrap();
    let conversation = ChatConversationRecord {
        id: "conversation".into(),
        project_id: None,
        model_id: None,
        title: "Synthetic".into(),
        messages: vec![ChatMessageRecord {
            human_interaction_response: None,
            id: "assistant".into(),
            role: "assistant".into(),
            content: "initial".into(),
            created_at: 1,
            status: Some("pending".into()),
            attachments: vec![],
            agent_run_json: Some(projection),
            folder_references_json: None,
            ui_state_json: None,
        }],
        created_at: 1,
        updated_at: 1,
        pinned_at: None,
        archived_at: None,
        unread_at: None,
    };
    service.save_conversation(conversation.clone()).unwrap();
    service.state.write_sync(1024, crate::storage::StorageWriteClass::Normal, |connection| {
        connection.execute("INSERT INTO conversation_turn_traces
            (assistant_message_id,conversation_id,run_id,schema_version,terminal_status,truncated,created_at,updated_at)
            VALUES ('assistant','conversation','run',?1,'in_progress',0,1,1)", [crate::CONVERSATION_TURN_TRACE_SCHEMA_VERSION]).unwrap();
    }).unwrap();
    (directory, service, conversation)
}

fn begin(service: &StorageService) -> CoreRuntimeCheckpointState {
    service
        .begin_runtime_checkpoint("conversation", "assistant", "run")
        .unwrap()
        .unwrap()
}

fn checkpoint(state: &CoreRuntimeCheckpointState, patch: Value) -> CoreRuntimeCheckpoint {
    CoreRuntimeCheckpoint {
        conversation_id: "conversation".into(),
        assistant_message_id: "assistant".into(),
        run_id: "run".into(),
        generation: state.generation.clone(),
        expected_sequence: state.sequence,
        sequence: state.sequence + 1,
        content: "fresh content".into(),
        projection_json: patch.to_string(),
    }
}

fn stored(service: &StorageService) -> (String, Value) {
    let connection = service.state.read_connection().unwrap();
    connection
        .query_row(
            "SELECT content, agent_run_json FROM messages WHERE id='assistant'",
            [],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    serde_json::from_str::<Value>(&row.get::<_, String>(1)?).unwrap(),
                ))
            },
        )
        .unwrap()
}

#[test]
fn checkpoint_replays_lost_ack_and_rejects_divergent_or_skipped_sequence() {
    let (_directory, service, _) = fixture();
    let initial = begin(&service);
    assert_eq!(initial.sequence, 0);
    assert_eq!(initial.content, "initial");
    let input = checkpoint(&initial, serde_json::json!({ "firstResponseAt": 2 }));
    assert_eq!(
        service.apply_runtime_checkpoint(&input).unwrap(),
        CoreRuntimeCheckpointOutcome::Persisted
    );
    let committed = stored(&service);
    assert_eq!(committed.0, "fresh content");
    assert_eq!(
        service.apply_runtime_checkpoint(&input).unwrap(),
        CoreRuntimeCheckpointOutcome::Replayed
    );
    assert_eq!(stored(&service), committed);
    let mut divergent = input.clone();
    divergent.content = "different bytes".into();
    assert!(service.apply_runtime_checkpoint(&divergent).is_err());
    divergent.sequence = 3;
    divergent.expected_sequence = 2;
    assert!(service.apply_runtime_checkpoint(&divergent).is_err());
    assert_eq!(stored(&service), committed);
}

#[test]
fn checked_checkpoint_begin_rechecks_live_owner_and_rolls_back_generation() {
    use std::sync::atomic::{AtomicUsize, Ordering};

    let (_directory, service, _) = fixture();
    let initial = begin(&service);
    let previous = stored(&service);
    let checks = AtomicUsize::new(0);
    let result =
        service.begin_runtime_checkpoint_checked("conversation", "assistant", "run", None, || {
            if checks.fetch_add(1, Ordering::SeqCst) == 0 {
                Ok(())
            } else {
                Err("synthetic segment owner retired before commit".into())
            }
        });
    assert!(matches!(
        result,
        Err(CoreRuntimeCheckpointBeginError::Fenced(_))
    ));
    assert_eq!(checks.load(Ordering::SeqCst), 2);
    assert_eq!(stored(&service), previous);
    assert_eq!(
        stored(&service).1["coreCheckpoint"]["generation"],
        initial.generation
    );
}

#[test]
fn checkpoint_fingerprint_streaming_matches_existing_json_bytes() {
    let (_directory, service, _) = fixture();
    let initial = begin(&service);
    let mut input = checkpoint(
        &initial,
        serde_json::json!({
            "todo": [{"id":"中文\\\"\n", "text":"🧪 café", "nested":[null, true, 2.5]}],
            "firstResponseAt": 2,
        }),
    );
    input.content = "正文\n\t\\\"\u{0000}🧪".repeat(1024);
    let patch: Map<String, Value> = serde_json::from_str(&input.projection_json).unwrap();
    let old_bytes = serde_json::to_vec(&(
        &input.conversation_id,
        &input.assistant_message_id,
        &input.run_id,
        &input.generation,
        input.expected_sequence,
        input.sequence,
        &input.content,
        &patch,
    ))
    .unwrap();
    assert_eq!(
        checkpoint_fingerprint(&input, &patch).unwrap(),
        format!("{:x}", Sha256::digest(old_bytes))
    );
}

#[test]
fn checkpoint_failed_commit_keeps_sequence_retryable_and_does_not_ack() {
    let (_directory, service, _) = fixture();
    let initial = begin(&service);
    let input = checkpoint(&initial, serde_json::json!({}));
    service.state.write_sync(1024, crate::storage::StorageWriteClass::Normal, |connection| {
        connection.execute_batch("CREATE TRIGGER checkpoint_failure BEFORE UPDATE OF agent_run_json ON messages
            WHEN json_extract(NEW.agent_run_json,'$.coreCheckpoint.sequence') = 1
            BEGIN SELECT RAISE(ABORT,'synthetic disk commit failure'); END;").unwrap();
    }).unwrap();
    assert!(service
        .apply_runtime_checkpoint(&input)
        .unwrap_err()
        .contains("synthetic disk commit failure"));
    assert_eq!(stored(&service).0, "initial");
    assert_eq!(stored(&service).1["coreCheckpoint"]["sequence"], 0);
    service
        .state
        .write_sync(
            1024,
            crate::storage::StorageWriteClass::Normal,
            |connection| {
                connection
                    .execute_batch("DROP TRIGGER checkpoint_failure")
                    .unwrap();
            },
        )
        .unwrap();
    assert_eq!(
        service.apply_runtime_checkpoint(&input).unwrap(),
        CoreRuntimeCheckpointOutcome::Persisted
    );
}

#[test]
fn newer_generation_terminal_and_wrong_turn_retire_old_publications() {
    let (_directory, service, _) = fixture();
    let first = begin(&service);
    let second = begin(&service);
    assert_ne!(first.generation, second.generation);
    assert_eq!(
        service
            .apply_runtime_checkpoint(&checkpoint(&first, serde_json::json!({})))
            .unwrap(),
        CoreRuntimeCheckpointOutcome::Retired
    );
    let mut wrong = checkpoint(&second, serde_json::json!({}));
    wrong.run_id = "other-run".into();
    assert_eq!(
        service.apply_runtime_checkpoint(&wrong).unwrap(),
        CoreRuntimeCheckpointOutcome::Retired
    );
    let input = checkpoint(&second, serde_json::json!({}));
    service.state.write_sync(1024, crate::storage::StorageWriteClass::Control, |connection| {
        connection.execute("UPDATE conversation_turn_traces SET terminal_status='completed', completed_at=2, updated_at=2 WHERE assistant_message_id='assistant'", []).unwrap();
        chat_repository::update_message_status_and_content(connection, "conversation", "assistant", "terminal answer", Some("sent"), 2).unwrap();
        chat_repository::update_message_run_terminal_state(connection, "conversation", "assistant", "run", Some("sent"), "completed", 2, None).unwrap();
    }).unwrap();
    let terminal = stored(&service);
    assert_eq!(
        service.apply_runtime_checkpoint(&input).unwrap(),
        CoreRuntimeCheckpointOutcome::Retired
    );
    assert!(service
        .begin_runtime_checkpoint("conversation", "assistant", "run")
        .unwrap()
        .is_none());
    assert_eq!(stored(&service), terminal);
}

#[test]
fn checkpoint_preserves_durable_fields_waiting_status_and_anchored_timeline() {
    let (_directory, service, _) = fixture();
    let mut run = stored(&service).1;
    run["status"] = "waiting_for_approval".into();
    run["state"]["status"] = "waiting_for_approval".into();
    run["usage"] = serde_json::json!({"inputTokens": 8});
    run["timeline"] = serde_json::json!([
        {"id":"durable", "type":"message", "content":"committed", "streamId":"old", "traceSequence":0},
        {"id":"live-old", "type":"message", "content":"replace", "streamId":"old-live"}
    ]);
    service
        .state
        .write_sync(
            4096,
            crate::storage::StorageWriteClass::Normal,
            |connection| {
                connection
                    .execute(
                        "UPDATE messages SET agent_run_json=?1 WHERE id='assistant'",
                        [run.to_string()],
                    )
                    .unwrap();
            },
        )
        .unwrap();
    let state = begin(&service);
    let input = checkpoint(
        &state,
        serde_json::json!({
            "state": {"status":"running", "activeRunId":"wrong", "lastError":null, "updatedAt":4},
            "timeline":[{"id":"live-new", "type":"message", "content":"current", "streamId":"new"}]
        }),
    );
    service.apply_runtime_checkpoint(&input).unwrap();
    let run = stored(&service).1;
    assert_eq!(run["status"], "waiting_for_approval");
    assert_eq!(run["state"]["status"], "waiting_for_approval");
    assert_eq!(run["state"]["activeRunId"], "run");
    assert_eq!(run["usage"]["inputTokens"], 8);
    assert_eq!(run["timeline"].as_array().unwrap().len(), 2);
    assert_eq!(run["timeline"][0]["id"], "durable");
    assert_eq!(run["timeline"][1]["id"], "live-new");
    for forbidden in [
        serde_json::json!({"usage":{}}),
        serde_json::json!({"approvals":[]}),
        serde_json::json!({"status":"running"}),
        serde_json::json!({"timeline":[{"id":"forged", "type":"message", "content":"x", "traceSequence":1}]}),
    ] {
        let state = begin(&service);
        assert!(service
            .apply_runtime_checkpoint(&checkpoint(&state, forbidden))
            .is_err());
    }
}

#[test]
fn stale_renderer_history_and_full_save_cannot_overwrite_or_drop_core_owned_message() {
    let (_directory, service, mut stale) = fixture();
    let initial = begin(&service);
    service
        .apply_runtime_checkpoint(&checkpoint(
            &initial,
            serde_json::json!({"firstResponseAt":2}),
        ))
        .unwrap();
    let committed = stored(&service);
    service
        .save_chat_message_state(
            "conversation",
            ChatMessageStateRecord {
                id: "assistant".into(),
                content: "stale renderer".into(),
                status: Some("pending".into()),
                agent_run_json: stale.messages[0].agent_run_json.clone(),
            },
        )
        .unwrap();
    service
        .upsert_chat_messages("conversation", stale.messages.clone(), 0)
        .unwrap();
    let response = service.save_conversation(stale.clone()).unwrap();
    assert_eq!(response.messages[0].content, committed.0);
    assert_eq!(stored(&service), committed);
    stale.messages.clear();
    let response = service.save_conversation(stale).unwrap();
    assert_eq!(response.messages.len(), 1);
    assert_eq!(response.messages[0].content, committed.0);
    assert_eq!(stored(&service), committed);
    service
        .save_chat_message_ui_state("conversation", "assistant", Some("{\"collapsed\":true}"))
        .unwrap();
    assert_eq!(stored(&service), committed);
    let loaded = service.load_conversation("conversation").unwrap().unwrap();
    assert_eq!(loaded.messages[0].content, committed.0);
    let loaded_run: Value =
        serde_json::from_str(loaded.messages[0].agent_run_json.as_ref().unwrap()).unwrap();
    assert_eq!(loaded_run["coreCheckpoint"], committed.1["coreCheckpoint"]);
}

#[test]
fn renderer_cannot_install_a_checkpoint_marker_on_a_legacy_or_new_message() {
    let (_directory, service, mut legacy) = fixture();
    let mut run: Value =
        serde_json::from_str(legacy.messages[0].agent_run_json.as_ref().unwrap()).unwrap();
    run["coreCheckpoint"] = serde_json::json!({"generation":Uuid::new_v4().to_string(),"sequence":0,"fingerprint":"0".repeat(64)});
    let forged = run.to_string();
    assert!(service
        .save_chat_message_state(
            "conversation",
            ChatMessageStateRecord {
                id: "assistant".into(),
                content: "forged".into(),
                status: Some("pending".into()),
                agent_run_json: Some(forged.clone()),
            }
        )
        .is_err());
    legacy.messages[0].agent_run_json = Some(forged);
    assert!(service.save_conversation(legacy.clone()).is_err());
    legacy.messages[0].id = "new-forged".into();
    assert!(service
        .upsert_chat_messages("conversation", legacy.messages, 1)
        .is_err());
    assert_eq!(stored(&service).0, "initial");
}

#[test]
fn read_display_tool_patches_merge_by_exact_identity_without_replacing_other_tools() {
    let (_directory, service, _) = fixture();
    let first = begin(&service);
    let input = checkpoint(
        &first,
        serde_json::json!({"toolCalls":[
        {"id":"read-a","tool":"read_file","args":{"path":"a"},"approvalStatus":"not_required","reason":null}
    ], "toolResults":[{"callId":"read-a","tool":"read_file","ok":true,"result":{"content":"complete content"}}]}),
    );
    service.apply_runtime_checkpoint(&input).unwrap();
    let second = begin(&service);
    service.apply_runtime_checkpoint(&checkpoint(&second, serde_json::json!({"toolCalls":[
        {"id":"read-b","tool":"read_file","args":{"path":"b"},"approvalStatus":"not_required","reason":null}
    ]}))).unwrap();
    assert_eq!(stored(&service).1["toolCalls"].as_array().unwrap().len(), 2);
    assert_eq!(
        stored(&service).1["toolResults"][0]["result"]["content"],
        "complete content"
    );
}

#[test]
fn core_lifecycle_settlement_retains_deferred_tool_projection_but_legacy_stays_strict() {
    let (_directory, service, _) = fixture();
    begin(&service);
    let mut run = stored(&service).1;
    run["firstResponseAt"] = 2.into();
    run["timeline"] = serde_json::json!([{
        "id":"trace-tool", "type":"tool_call", "callId":"trace-owned",
        "identity":{"kind":"builtin", "toolName":"read_file"}, "traceSequence":0
    }]);
    // The exact serialized AgentToolIdentity is authoritative, not a guessed JSON tag.
    run["timeline"][0]["identity"] = serde_json::to_value(crate::AgentToolIdentity::Builtin {
        tool_name: "read_file".into(),
    })
    .unwrap();
    assert!(
        chat_repository::current_agent_run_projection_is_safe_for_trace_rebuild(
            run.as_object().unwrap(),
            "run"
        )
    );
    assert!(!chat_repository::current_agent_run_projection_is_safe(
        run.as_object().unwrap(),
        "run"
    ));
    let settled: Value = serde_json::from_str(
        &chat_repository::canonical_agent_run_lifecycle_projection(
            Some(&run.to_string()),
            "run",
            "completed",
            1,
            3,
            Some(3),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(settled["firstResponseAt"], 2);
    assert_eq!(settled["coreCheckpoint"], run["coreCheckpoint"]);
    run.as_object_mut().unwrap().remove("coreCheckpoint");
    let legacy: Value = serde_json::from_str(
        &chat_repository::canonical_agent_run_lifecycle_projection(
            Some(&run.to_string()),
            "run",
            "completed",
            1,
            3,
            Some(3),
        )
        .unwrap(),
    )
    .unwrap();
    assert!(legacy.get("firstResponseAt").is_none());
}

#[test]
fn writer_blocked_checkpoint_observes_terminal_commit_before_it_can_ack() {
    let (directory, service, _) = fixture();
    let initial = begin(&service);
    let service = Arc::new(service);
    let mut external =
        rusqlite::Connection::open(directory.path().join("checkpoint.sqlite")).unwrap();
    let transaction = external
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .unwrap();
    let writer = Arc::clone(&service);
    let input = checkpoint(&initial, serde_json::json!({}));
    let (attempting_tx, attempting_rx) = std::sync::mpsc::channel();
    let (result_tx, result_rx) = std::sync::mpsc::channel();
    let worker = std::thread::spawn(move || {
        attempting_tx.send(()).unwrap();
        result_tx
            .send(writer.apply_runtime_checkpoint(&input))
            .unwrap();
    });
    attempting_rx.recv().unwrap();
    assert!(result_rx
        .recv_timeout(std::time::Duration::from_millis(30))
        .is_err());
    transaction.execute("UPDATE conversation_turn_traces SET terminal_status='completed', completed_at=2, updated_at=2 WHERE assistant_message_id='assistant'", []).unwrap();
    transaction.commit().unwrap();
    assert_eq!(
        result_rx
            .recv_timeout(std::time::Duration::from_secs(5))
            .unwrap()
            .unwrap(),
        CoreRuntimeCheckpointOutcome::Retired
    );
    worker.join().unwrap();
    assert_eq!(stored(&service).0, "initial");
}

#[test]
fn checkpoint_fence_and_replay_survive_reopening_the_same_temporary_database() {
    let (directory, service, _) = fixture();
    let initial = begin(&service);
    let input = checkpoint(&initial, serde_json::json!({"lastResponseAt":2}));
    service.apply_runtime_checkpoint(&input).unwrap();
    drop(service);
    let reopened = StorageService::open_with_model_credentials(
        &directory.path().join("checkpoint.sqlite"),
        Arc::new(crate::image_generation::InMemoryCredentialStore::default()),
    )
    .unwrap();
    assert_eq!(
        reopened.apply_runtime_checkpoint(&input).unwrap(),
        CoreRuntimeCheckpointOutcome::Replayed
    );
    assert_eq!(stored(&reopened).0, "fresh content");
}

#[test]
fn begin_reauthorizes_resume_read_display_ids_from_builtin_trace_identity() {
    let (_directory, service, _) = fixture();
    let initial = begin(&service);
    service.apply_runtime_checkpoint(&checkpoint(&initial, serde_json::json!({"toolCalls":[
        {"id":"real","tool":"read_file","args":{},"approvalStatus":"not_required","reason":null},
        {"id":"extension","tool":"read_file","args":{},"approvalStatus":"not_required","reason":null},
        {"id":"absent","tool":"read_file","args":{},"approvalStatus":"not_required","reason":null}
    ]}))).unwrap();
    service.state.write_sync(4096, crate::storage::StorageWriteClass::Normal, |connection| {
        for (sequence, call_id, provenance) in [
            (0, "real", crate::AgentToolIdentity::Builtin { tool_name:"read_file".into() }),
            (1, "extension", crate::AgentToolIdentity::RuntimeExtension { extension_id:"fixture".into(), tool_name:"read_file".into() }),
        ] {
            let item = serde_json::to_string(&ConversationTurnTraceItem::ToolCall {
                sequence, call_id:call_id.into(), tool:"read_file".into(), provenance,
                operation:serde_json::json!({}), approval_status:crate::AgentApprovalStatus::NotRequired,
                truncated:false,
            }).unwrap();
            connection.execute("INSERT INTO conversation_turn_trace_items(assistant_message_id,sequence,item_kind,item_json)
                VALUES ('assistant',?1,'tool_call',?2)", rusqlite::params![sequence,item]).unwrap();
        }
    }).unwrap();
    assert_eq!(begin(&service).presentation_call_ids, vec!["real"]);
}

fn activity(field: &str, id: &str, status: &str, updated_at: u64) -> Value {
    match field {
        "readActivities" => serde_json::json!({
            "callId":id,"tool":"read_file","kind":"file","status":status,
            "path":"relative.txt","fileName":"relative.txt","updatedAt":updated_at
        }),
        "fileChanges" => serde_json::json!({
            "schemaVersion":crate::AGENT_FILE_CHANGE_PROTOCOL_SCHEMA_VERSION,"transactionId":id,
            "conversationId":"conversation","projectId":null,"filePath":"new.txt","operation":"create",
            "updateStrategy":null,"status":status,"baseRevision":null,"additions":1,"deletions":0,
            "lineCount":1,"byteCount":1,"mutationCount":1,"nextMutationIndex":1,"statsFinal":true,
            "summary":null,"createdAt":1,"updatedAt":updated_at
        }),
        "skillInstallations" => serde_json::json!({
            "status":status,"action":{
                "schemaVersion":crate::AGENT_SKILL_INSTALLATION_SCHEMA_VERSION,"id":id,
                "installRef":format!("install:{id}"),"approvalStatus":"required","expiresAt":100,
                "preview":{"name":"Synthetic package","description":"Fixture","sourceSummary":{},
                    "resolvedRevision":"revision","fileCount":1,"totalBytes":1,
                    "resourceSummary":{"total":1,"references":0,"assets":0,"scripts":0,"bytes":1},
                    "containsScripts":false,"warnings":[],"compatibility":"compatible",
                    "operation":"install","impact":"Adds fixture"}
            }
        }),
        _ => unreachable!(),
    }
}

#[test]
fn sparse_activity_patches_preserve_other_items_and_do_not_reopen_settled_status() {
    for (field, terminal, pending) in [
        ("readActivities", "completed", "running"),
        ("fileChanges", "applied", "applying"),
        ("skillInstallations", "installed", "installing"),
    ] {
        let (_directory, service, _) = fixture();
        let initial = begin(&service);
        let patch = serde_json::json!({ field:[activity(field,"first",terminal,10),activity(field,"second",terminal,10)] });
        service
            .apply_runtime_checkpoint(&checkpoint(&initial, patch))
            .unwrap();
        let next = begin(&service);
        service
            .apply_runtime_checkpoint(&checkpoint(
                &next,
                serde_json::json!({field:[activity(field,"first",pending,20)]}),
            ))
            .unwrap();
        let projection = stored(&service).1;
        assert_eq!(projection[field].as_array().unwrap().len(), 2, "{field}");
        assert_eq!(projection[field][0]["status"], terminal, "{field}");
        let next = begin(&service);
        service
            .apply_runtime_checkpoint(&checkpoint(&next, serde_json::json!({field:[]})))
            .unwrap();
        assert_eq!(
            stored(&service).1[field],
            projection[field],
            "empty {field} is not deletion"
        );
        let next = begin(&service);
        service
            .apply_runtime_checkpoint(&checkpoint(
                &next,
                serde_json::json!({field:[activity(field,"third",terminal,20)]}),
            ))
            .unwrap();
        assert_eq!(stored(&service).1[field].as_array().unwrap().len(), 3);
    }
}

#[test]
fn read_activity_result_can_resolve_its_path_but_cannot_change_call_tool_identity() {
    let (_directory, service, _) = fixture();
    let initial = begin(&service);
    service
        .apply_runtime_checkpoint(&checkpoint(
            &initial,
            serde_json::json!({"readActivities":[activity("readActivities","read", "running",1)]}),
        ))
        .unwrap();
    let next = begin(&service);
    let mut resolved = activity("readActivities", "read", "completed", 2);
    resolved["path"] = "/resolved/relative.txt".into();
    service
        .apply_runtime_checkpoint(&checkpoint(
            &next,
            serde_json::json!({"readActivities":[resolved.clone()]}),
        ))
        .unwrap();
    assert_eq!(
        stored(&service).1["readActivities"][0]["path"],
        "/resolved/relative.txt"
    );
    let next = begin(&service);
    resolved["tool"] = "read_word".into();
    assert!(service
        .apply_runtime_checkpoint(&checkpoint(
            &next,
            serde_json::json!({"readActivities":[resolved]})
        ))
        .is_err());
    assert_eq!(stored(&service).1["readActivities"][0]["tool"], "read_file");
}

#[test]
fn failed_read_without_a_path_does_not_poison_the_next_stream_checkpoint() {
    let (_directory, service, _) = fixture();
    service.state.write_sync(4096, crate::storage::StorageWriteClass::Normal, |connection| {
        for (sequence, kind, item) in [
            (0, "tool_call", ConversationTurnTraceItem::ToolCall {
                sequence:0, call_id:"missing-path".into(), tool:"read_file".into(),
                provenance:crate::AgentToolIdentity::Builtin { tool_name:"read_file".into() },
                operation:serde_json::json!({}), approval_status:crate::AgentApprovalStatus::NotRequired,
                truncated:false,
            }),
            (1, "tool_result", ConversationTurnTraceItem::ToolResult {
                sequence:1, call_id:"missing-path".into(), tool:"read_file".into(),
                status:crate::conversation_trace::ConversationTraceToolResultStatus::Failed,
                success:false, observation:serde_json::json!({}),
                approval_status:crate::AgentApprovalStatus::NotRequired,
                error:Some("path is required".into()), truncated:false, archive:Default::default(),
            }),
        ] {
            connection.execute("INSERT INTO conversation_turn_trace_items(assistant_message_id,sequence,item_kind,item_json)
                VALUES ('assistant',?1,?2,?3)", rusqlite::params![sequence,kind,serde_json::to_string(&item).unwrap()]).unwrap();
        }
    }).unwrap();
    let initial = begin(&service);
    let mut failed = activity("readActivities", "missing-path", "failed", 2);
    failed["path"] = "".into();
    failed["fileName"] = "file".into();
    failed["error"] = "path is required".into();
    let first = checkpoint(
        &initial,
        serde_json::json!({"readActivities":[failed.clone()]}),
    );
    assert_eq!(
        service.apply_runtime_checkpoint(&first).unwrap(),
        CoreRuntimeCheckpointOutcome::Persisted
    );
    let next = CoreRuntimeCheckpoint {
        expected_sequence: first.sequence,
        sequence: first.sequence + 1,
        content: "recovered model response".into(),
        projection_json: serde_json::json!({"timeline":[{
            "id":"stream:next", "type":"message", "streamId":"next", "content":"recovered model response"
        }]}).to_string(),
        ..first
    };
    assert_eq!(
        service.apply_runtime_checkpoint(&next).unwrap(),
        CoreRuntimeCheckpointOutcome::Persisted
    );
    let (content, projection) = stored(&service);
    assert_eq!(content, "recovered model response");
    assert_eq!(projection["readActivities"], serde_json::json!([failed]));
    assert_eq!(projection["coreCheckpoint"]["sequence"], 2);
    let loaded = service.load_conversation("conversation").unwrap().unwrap();
    let run: Value =
        serde_json::from_str(loaded.messages[0].agent_run_json.as_deref().unwrap()).unwrap();
    assert_eq!(run["readActivities"][0]["path"], "");
    assert_eq!(loaded.messages[0].content, "recovered model response");
}
