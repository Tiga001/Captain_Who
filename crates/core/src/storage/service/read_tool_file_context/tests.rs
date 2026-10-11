use super::*;
use serde_json::json;

fn fixture() -> (tempfile::TempDir, StorageService) {
    let directory = tempfile::tempdir().unwrap();
    let service = StorageService::open_with_model_credentials(
        &directory.path().join("read-tool.sqlite"),
        Arc::new(crate::image_generation::InMemoryCredentialStore::default()),
    )
    .unwrap();
    for id in ["before-project", "after-project"] {
        service
            .save_project(ProjectRecord::without_folders(id, id, 1))
            .unwrap();
    }
    service
        .save_conversation(ChatConversationRecord {
            id: "conversation".into(),
            project_id: Some("before-project".into()),
            model_id: None,
            title: "synthetic read proof".into(),
            messages: vec![ChatMessageRecord {
                human_interaction_response: None,
                id: "assistant".into(),
                role: "assistant".into(),
                content: "irrelevant answer body".repeat(1000),
                created_at: 1,
                status: Some("sent".into()),
                attachments: Vec::new(),
                folder_references_json: None,
                agent_run_json: Some(legacy_run("before.txt").to_string()),
                ui_state_json: None,
            }],
            created_at: 1,
            updated_at: 1,
            pinned_at: None,
            archived_at: None,
            unread_at: None,
        })
        .unwrap();
    (directory, service)
}

fn legacy_run(path: &str) -> Value {
    json!({
        "runId": "run",
        "toolCalls": [{"id":"read", "tool":"read_file", "args":{"path":path}}],
        "toolResults": [{"callId":"read", "tool":"read_file", "result":{"path":path, "content":"unrelated tool body".repeat(1000)}}]
    })
}

#[test]
fn narrow_context_preserves_legacy_proof_without_history_bodies() {
    let (_directory, service) = fixture();
    let context = service
        .load_read_tool_file_context("conversation", "assistant", "read")
        .unwrap()
        .unwrap();
    assert_eq!(context.conversation_id, "conversation");
    assert_eq!(context.project_id.as_deref(), Some("before-project"));
    assert!(context.message_found);
    assert_eq!(context.run_id.as_deref(), Some("run"));
    assert_eq!(context.call.unwrap().path.as_deref(), Some("before.txt"));
    assert!(context.call_identity.is_none());
    assert_eq!(context.result_path.as_deref(), Some("before.txt"));
    let encoded = serde_json::to_string(
        &service
            .load_read_tool_file_context("conversation", "assistant", "read")
            .unwrap(),
    )
    .unwrap();
    assert!(encoded.len() < 512);
    assert!(!encoded.contains("body"));
    assert!(!encoded.contains("toolResults"));
    let missing = service
        .load_read_tool_file_context("conversation", "other", "read")
        .unwrap()
        .unwrap();
    assert!(!missing.message_found);
    assert!(missing.call.is_none());
    assert!(service
        .load_read_tool_file_context("other", "assistant", "read")
        .unwrap()
        .is_none());
    assert!(service
        .load_read_tool_file_context("conversation", "assistant", "other")
        .unwrap()
        .unwrap()
        .call
        .is_none());
}

#[test]
fn snapshot_never_combines_new_project_with_old_call_and_observes_later_deletion() {
    let (_directory, service) = fixture();
    let mut reader = service.state.read_connection().unwrap();
    let transaction = reader.transaction().unwrap();
    transaction
        .query_row(
            "SELECT revision FROM conversations WHERE id='conversation'",
            [],
            |row| row.get::<_, i64>(0),
        )
        .unwrap();
    // The storage owner intentionally rejects nested read/write acquisition on one thread.
    // Model a real independent request while this reader keeps its original WAL snapshot.
    std::thread::scope(|scope| {
        scope.spawn(|| {
        service.state.write_sync(1024, crate::storage::StorageWriteClass::Normal, |connection| {
            let update = connection.transaction().map_err(storage_error)?;
            update.execute("UPDATE conversations SET project_id='after-project' WHERE id='conversation'", []).map_err(storage_error)?;
            update.execute("UPDATE messages SET agent_run_json=?1 WHERE id='assistant'", [legacy_run("after.txt").to_string()]).map_err(storage_error)?;
            update.commit().map_err(storage_error)
        }).unwrap().unwrap();
    }).join().unwrap()
    });
    let before = read_context(&transaction, "conversation", "assistant", "read")
        .unwrap()
        .unwrap();
    assert_eq!(before.project_id.as_deref(), Some("before-project"));
    assert_eq!(before.call.unwrap().path.as_deref(), Some("before.txt"));
    transaction.commit().unwrap();
    drop(reader);
    let after = service
        .load_read_tool_file_context("conversation", "assistant", "read")
        .unwrap()
        .unwrap();
    assert_eq!(after.project_id.as_deref(), Some("after-project"));
    assert_eq!(after.call.unwrap().path.as_deref(), Some("after.txt"));
    let mut deletion_reader = service.state.read_connection().unwrap();
    let deletion_snapshot = deletion_reader.transaction().unwrap();
    let before_deletion = read_context(&deletion_snapshot, "conversation", "assistant", "read")
        .unwrap()
        .unwrap();
    std::thread::scope(|scope| {
        scope
            .spawn(|| service.delete_conversation("conversation").unwrap())
            .join()
            .unwrap()
    });
    assert_eq!(
        read_context(&deletion_snapshot, "conversation", "assistant", "read")
            .unwrap()
            .unwrap(),
        before_deletion
    );
    deletion_snapshot.commit().unwrap();
    drop(deletion_reader);
    assert!(service
        .load_read_tool_file_context("conversation", "assistant", "read")
        .unwrap()
        .is_none());
}

#[test]
fn durable_trace_supplies_proof_without_renderer_run_json() {
    let (_directory, service) = fixture();
    let trace = ConversationTurnTrace {
        schema_version: crate::CONVERSATION_TURN_TRACE_SCHEMA_VERSION,
        run_id: "trace-run".into(),
        conversation_id: "conversation".into(),
        assistant_message_id: "assistant".into(),
        terminal_status: crate::ConversationTurnTraceTerminalStatus::InProgress,
        terminal_error: None,
        truncated: false,
        items: vec![ConversationTurnTraceItem::ToolCall {
            sequence: 0,
            call_id: "trace-call".into(),
            tool: "read_file".into(),
            provenance: crate::AgentToolIdentity::Builtin {
                tool_name: "read_file".into(),
            },
            operation: json!({"path":"durable.txt"}),
            approval_status: crate::AgentApprovalStatus::NotRequired,
            truncated: false,
        }],
    };
    {
        let mut writer = service.state.connection().unwrap();
        writer
            .execute(
                "UPDATE messages SET agent_run_json=NULL WHERE id='assistant'",
                [],
            )
            .unwrap();
        conversation_trace_repository::replace_trace(&mut writer, &trace, 1, 2).unwrap();
    }
    let narrow = service
        .load_read_tool_file_context("conversation", "assistant", "trace-call")
        .unwrap()
        .unwrap();
    assert_eq!(narrow.run_id.as_deref(), Some("trace-run"));
    assert_eq!(narrow.call.unwrap().path.as_deref(), Some("durable.txt"));
    assert_eq!(
        narrow.call_identity.unwrap(),
        ReadToolFileIdentity {
            r#type: Some("builtin".into()),
            tool_name: Some("read_file".into())
        }
    );
    assert!(service
        .load_read_tool_file_context("conversation", "assistant", "read")
        .unwrap()
        .unwrap()
        .call
        .is_none());
    let full = service.load_conversation("conversation").unwrap().unwrap();
    let run: Value =
        serde_json::from_str(full.messages[0].agent_run_json.as_deref().unwrap()).unwrap();
    assert_eq!(run["toolCalls"][0]["args"]["path"], "durable.txt");
}

#[test]
fn malformed_explicit_identity_stays_distinct_from_legacy_missing_identity() {
    let (_directory, service) = fixture();
    for identity in [
        Value::Null,
        json!({"type":"runtime_extension", "toolName":"read_file"}),
    ] {
        let mut run = legacy_run("before.txt");
        run["timeline"] = json!([{ "type":"tool_call", "callId":"read", "identity":identity }]);
        service
            .state
            .connection()
            .unwrap()
            .execute(
                "UPDATE messages SET agent_run_json=?1 WHERE id='assistant'",
                [run.to_string()],
            )
            .unwrap();
        let narrow = service
            .load_read_tool_file_context("conversation", "assistant", "read")
            .unwrap()
            .unwrap();
        assert!(narrow.call_identity.is_some());
        assert_ne!(
            narrow.call_identity.unwrap().r#type.as_deref(),
            Some("builtin")
        );
    }
}
