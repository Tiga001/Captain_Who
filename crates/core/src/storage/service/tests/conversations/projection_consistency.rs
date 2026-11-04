use super::*;

#[test]
fn conversation_meta_projection_filters_child_agents_in_one_statement() {
    let fixture = StorageFixture::new();
    let service = fixture.service();
    for id in ["meta-ordinary", "meta-root"] {
        service
            .save_conversation(conversation(id, None, &format!("{id}-message")))
            .unwrap();
    }
    let mut child = conversation("meta-child", None, "meta-child-message");
    child.messages.clear();
    service.save_conversation(child).unwrap();
    bind_agent_root(&service, "meta-root-agent", "meta-root");
    bind_agent_child(
        &service,
        "meta-child-agent",
        "meta-child",
        "meta-root-agent",
        "meta-root",
        "child",
    );

    let (metas, selects) =
        trace_storage_selects(&service, || service.load_conversation_metas().unwrap());
    let mut ids = metas.into_iter().map(|meta| meta.id).collect::<Vec<_>>();
    ids.sort();
    assert_eq!(ids, ["meta-ordinary".to_string(), "meta-root".to_string()]);
    assert_eq!(selects.len(), 1, "unexpected metadata SQL: {selects:#?}");
    assert!(selects[0].contains("NOT EXISTS"));
    assert!(selects[0].contains("agent_nodes"));
}

#[test]
fn conversation_detail_projection_query_count_is_independent_of_history_length() {
    let fixture = StorageFixture::new();
    let service = fixture.service();
    seed_batched_conversation_projection(&service, "projection-one", 1);
    seed_batched_conversation_projection(&service, "projection-many", 12);

    let (one, one_selects) = trace_storage_selects(&service, || {
        service
            .load_conversation("projection-one")
            .unwrap()
            .unwrap()
    });
    let (many, many_selects) = trace_storage_selects(&service, || {
        service
            .load_conversation("projection-many")
            .unwrap()
            .unwrap()
    });

    assert_eq!(
        one_selects.len(),
        many_selects.len(),
        "one: {one_selects:#?}\nmany: {many_selects:#?}"
    );
    assert_eq!(
        many_selects.len(),
        // The terminal-inclusive Skill decision/audit overlay adds one batched query. Its
        // receipt may exist without a runtime ToolCall, so it cannot use the FileChange gate.
        16,
        "unexpected detail SQL: {many_selects:#?}"
    );
    assert_eq!(
        many_selects
            .iter()
            .filter(|sql| sql.contains("FROM agent_pending_actions AS pending"))
            .count(),
        1,
        "Skill installation recovery must remain one query for the whole conversation"
    );
    assert!(
        !many_selects
            .iter()
            .any(|sql| sql.contains("FROM agent_file_changes AS change")),
        "history without a builtin apply_patch source cannot have a proven FileChange overlay"
    );
    assert_eq!(
        many_selects
            .iter()
            .filter(|sql| sql.contains("FROM conversation_turn_trace_items AS item"))
            .count(),
        1,
        "ordinary detail must not re-read Trace bodies for an impossible FileChange overlay"
    );
    for table in [
        "conversation_turn_trace_items",
        "agent_run_guidances",
        "agent_run_guidance_attachments",
        "agent_pending_actions",
        "attachments",
        "workflow_mail_message_origins",
    ] {
        assert!(
            many_selects.iter().any(|sql| sql.contains(table)),
            "missing batched {table} query: {many_selects:#?}"
        );
    }

    assert_eq!(
        many_selects
            .iter()
            .filter(|sql| sql.contains("FROM workflow_mail_message_origins"))
            .count(),
        1,
        "mail origins must be loaded once for the whole conversation: {many_selects:#?}"
    );

    assert_eq!(one.messages.len(), 2);
    assert_eq!(many.messages.len(), 13);
    for message in many
        .messages
        .iter()
        .filter(|message| message.role == "assistant")
    {
        assert!(message.attachments.is_empty());
        let run: serde_json::Value =
            serde_json::from_str(message.agent_run_json.as_deref().unwrap()).unwrap();
        assert_eq!(run["timeline"].as_array().unwrap().len(), 1);
        assert_eq!(run["timeline"][0]["status"], "queued");
        assert_eq!(
            run["timeline"][0]["attachments"].as_array().unwrap().len(),
            1
        );
    }
}

#[test]
fn conversation_file_change_overlay_query_requires_a_builtin_patch_in_loaded_trace() {
    for source in ["builtin_patch", "other_builtin", "extension_patch"] {
        let fixture = StorageFixture::new();
        let service = fixture.service();
        let conversation_id = "projection-patch";
        let message_id = "projection-patch-assistant";
        let run_id = "projection-patch-run";
        save_projection_conversation(
            &service,
            conversation_id,
            message_id,
            current_projection_with_suffix(Some(run_id), "retained partial text"),
        );
        let tool = if source == "other_builtin" {
            "read_file"
        } else {
            "apply_patch"
        };
        let provenance = if source == "extension_patch" {
            crate::AgentToolIdentity::RuntimeExtension {
                extension_id: "synthetic-extension".into(),
                tool_name: tool.into(),
            }
        } else {
            crate::AgentToolIdentity::Builtin {
                tool_name: tool.into(),
            }
        };
        let mut trace = empty_projection_trace(conversation_id, message_id, run_id);
        trace.items.push(crate::ConversationTurnTraceItem::ToolCall {
            sequence: 0, call_id: "source-call".into(), tool: tool.into(), provenance,
            operation: serde_json::json!({"request":{"action":"begin","operation":"create","filePath":"synthetic.txt"}}),
            approval_status: crate::AgentApprovalStatus::NotRequired, truncated: false,
        });
        service
            .replace_conversation_turn_trace(&trace, 2, 2)
            .unwrap();
        let change = crate::storage::models::AgentFileChangeRecord {
            schema_version: 1,
            id: "synthetic-change".into(),
            conversation_id: conversation_id.into(),
            project_id: None,
            run_id: run_id.into(),
            source_tool_name: "apply_patch".into(),
            source_tool_call_id: "source-call".into(),
            source_tool_arguments_digest: "digest".into(),
            permission_revision: "permission".into(),
            tool_set_revision: "tools".into(),
            provider_wire_revision: "wire".into(),
            observation_id: "observation".into(),
            observation_json: "{}".into(),
            file_path: "synthetic.txt".into(),
            operation: "create".into(),
            strategy: None,
            status: "applied".into(),
            base_revision: None,
            base_content: "private base".into(),
            content: "private draft".into(),
            draft_revision: 1,
            next_mutation_index: 0,
            additions: 7,
            deletions: 0,
            line_count: 7,
            byte_count: 20,
            mutation_count: 0,
            stats_final: true,
            summary: None,
            final_action_id: Some("action".into()),
            final_action_arguments_digest: Some("action-digest".into()),
            final_permission_revision: Some("permission".into()),
            final_tool_set_revision: Some("tools".into()),
            final_provider_wire_revision: Some("wire".into()),
            created_at: 2,
            updated_at: 3,
            expires_at: 100,
        };
        {
            let connection = service.state.connection().unwrap();
            crate::storage::file_change_repository::insert_file_change(&connection, &change)
                .unwrap();
        }
        let (loaded, selects) = trace_storage_selects(&service, || {
            service.load_conversation(conversation_id).unwrap().unwrap()
        });
        let run: serde_json::Value = serde_json::from_str(
            loaded
                .messages
                .last()
                .unwrap()
                .agent_run_json
                .as_deref()
                .unwrap(),
        )
        .unwrap();
        let file_change_queries = selects
            .iter()
            .filter(|sql| sql.contains("FROM agent_file_changes AS change"))
            .count();
        assert_eq!(
            file_change_queries,
            usize::from(source == "builtin_patch"),
            "{source}: {selects:#?}"
        );
        if source == "builtin_patch" {
            assert_eq!(run["fileChanges"][0]["transactionId"], "synthetic-change");
            assert_eq!(run["fileChanges"][0]["status"], "applied");
            assert_eq!(run["fileChanges"][0]["additions"], 7);
            assert_eq!(run["fileChanges"].as_array().unwrap().len(), 1);
            assert!(!run.to_string().contains("private"));
        } else {
            assert_eq!(run["fileChanges"], serde_json::json!([]));
        }
    }
}

#[test]
fn conversation_view_preserves_a_strict_current_projection_suffix() {
    let fixture = StorageFixture::new();
    let service = fixture.service();
    let conversation_id = "conversation-current-projection";
    let assistant_message_id = "assistant-current-projection";
    let run_id = "run-current-projection";
    save_projection_conversation(
        &service,
        conversation_id,
        assistant_message_id,
        current_projection_with_suffix(Some(run_id), "preserve current suffix"),
    );
    service
        .append_in_progress_conversation_turn_trace(
            &empty_projection_trace(conversation_id, assistant_message_id, run_id),
            2,
            2,
        )
        .unwrap();

    let run = load_projected_run(&service, conversation_id);
    assert_eq!(run["runId"], run_id);
    assert_eq!(run["timeline"][0]["content"], "preserve current suffix");
    assert_eq!(
        run["messageStreamCheckpoints"]["stream-current"]["previousContent"],
        "partial response"
    );
    assert_eq!(run["interruption"]["reason"], "service_connection_failed");
    assert_eq!(run["fileChangeProposals"], serde_json::json!([]));
    assert_eq!(run["fileChanges"], serde_json::json!([]));
}

#[test]
fn conversation_view_rebuilds_a_projection_owned_by_another_run() {
    let fixture = StorageFixture::new();
    let service = fixture.service();
    let conversation_id = "conversation-mismatched-projection";
    let assistant_message_id = "assistant-mismatched-projection";
    let authoritative_run_id = "run-authoritative-projection";
    save_projection_conversation(
        &service,
        conversation_id,
        assistant_message_id,
        current_projection_with_suffix(Some("run-wrong-owner"), "must not survive"),
    );
    service
        .append_in_progress_conversation_turn_trace(
            &empty_projection_trace(conversation_id, assistant_message_id, authoritative_run_id),
            2,
            2,
        )
        .unwrap();

    let run = load_projected_run(&service, conversation_id);
    assert_eq!(run["runId"], authoritative_run_id);
    assert_eq!(run["timeline"], serde_json::json!([]));
    assert!(run.get("interruption").is_none());
    assert_eq!(run["messageStreamCheckpoints"], serde_json::json!({}));
}

#[test]
fn conversation_view_binds_a_current_null_run_id_to_the_durable_trace() {
    let fixture = StorageFixture::new();
    let service = fixture.service();
    let conversation_id = "conversation-null-run-projection";
    let assistant_message_id = "assistant-null-run-projection";
    let run_id = "run-bound-projection";
    save_projection_conversation(
        &service,
        conversation_id,
        assistant_message_id,
        current_projection_with_suffix(None, "preserve null-owner suffix"),
    );
    service
        .append_in_progress_conversation_turn_trace(
            &empty_projection_trace(conversation_id, assistant_message_id, run_id),
            2,
            2,
        )
        .unwrap();

    let run = load_projected_run(&service, conversation_id);
    assert_eq!(run["runId"], run_id);
    assert_eq!(run["timeline"][0]["content"], "preserve null-owner suffix");
}

#[test]
fn conversation_view_rejects_guidance_rows_with_conflicting_run_owners() {
    let fixture = StorageFixture::new();
    let service = fixture.service();
    let conversation_id = "conversation-conflicting-guidance";
    let assistant_message_id = "assistant-conflicting-guidance";
    save_projection_conversation(
        &service,
        conversation_id,
        assistant_message_id,
        current_projection_with_suffix(Some("run-guidance-a"), "must not load"),
    );
    for (index, run_id) in ["run-guidance-a", "run-guidance-b"].into_iter().enumerate() {
        service
            .store_agent_run_guidance(AgentRunGuidanceRecord {
                guidance_id: format!("guidance-conflict-{index}"),
                client_message_id: format!("client-guidance-conflict-{index}"),
                run_id: run_id.to_string(),
                conversation_id: conversation_id.to_string(),
                assistant_message_id: assistant_message_id.to_string(),
                content: format!("guidance {index}"),
                status: crate::AgentGuidanceStatus::Queued,
                attachment_ids: Vec::new(),
                folder_references_json: "[]".to_string(),
                applied_trace_sequence: None,
                terminal_reason: None,
                created_at: 3 + index as i64,
                updated_at: 3 + index as i64,
            })
            .unwrap();
    }

    assert_eq!(
        service.load_conversation_view(conversation_id).unwrap_err(),
        "guidance projection has inconsistent run identity"
    );
}
