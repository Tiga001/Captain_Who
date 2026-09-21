use super::*;

const CONVERSATION: &str = "conversation-compacted-rebuild";
const ASSISTANT: &str = "assistant-compacted-rebuild";
const RUN: &str = "run-compacted-rebuild";
const COVERED: &str = "COVERED_ORIGINAL_NARRATION";
const TAIL: &str = "UNCOVERED_NARRATION";
const APPENDED: &str = "NEWLY_APPENDED_NARRATION";
const SUMMARY: &str = "COMMITTED_SUMMARY_REPLACEMENT";

struct CompactedRebuildFixture {
    _directory: tempfile::TempDir,
    storage: Arc<StorageService>,
    service: AgentService,
    input: AgentChatInput,
    trace: ConversationTurnTrace,
    model_context: Vec<ConversationModelContextItem>,
}

impl CompactedRebuildFixture {
    fn new() -> Self {
        let directory = tempdir().unwrap();
        let storage =
            Arc::new(StorageService::open(&directory.path().join("storage.sqlite")).unwrap());
        storage.save_model_settings(test_model_settings()).unwrap();
        storage
            .save_conversation(ChatConversationRecord {
                id: CONVERSATION.into(),
                project_id: None,
                model_id: Some("model-1".into()),
                title: "Compacted context rebuild".into(),
                messages: vec![
                    ChatMessageRecord {
                        id: "user-compacted-rebuild".into(),
                        role: "user".into(),
                        content: "CURRENT_USER_REQUEST".into(),
                        created_at: 1,
                        status: Some("sent".into()),
                        human_interaction_response: None,
                        attachments: vec![],
                        folder_references_json: None,
                        agent_run_json: None,
                        ui_state_json: None,
                    },
                    ChatMessageRecord {
                        id: ASSISTANT.into(),
                        role: "assistant".into(),
                        content: String::new(),
                        created_at: 2,
                        status: Some("pending".into()),
                        human_interaction_response: None,
                        attachments: vec![],
                        folder_references_json: None,
                        agent_run_json: None,
                        ui_state_json: None,
                    },
                ],
                created_at: 1,
                updated_at: 2,
                pinned_at: None,
                archived_at: None,
                unread_at: None,
            })
            .unwrap();
        // Startup reconciliation must run before this process creates its active trace.
        let service = AgentService::new_authorized_for_test(storage.clone());
        let mut input: AgentChatInput = serde_json::from_value(json!({
            "apiUrl": "https://example.test/v1/chat/completions", "apiToken": "token",
            "model": "model-1", "modelCapabilities": {"imageInput": false},
            "contextWindowTokens": 128000, "contextWindowIndicatorEnabled": true,
            "messages": [],
        }))
        .unwrap();
        input.context = Some(AgentRunContext {
            collaboration_identity: None,
            conversation_id: Some(CONVERSATION.into()),
            project_id: None,
            workspace: None,
            attachment_library: None,
            permissions: AgentPermissions::default(),
        });
        freeze_test_pending_provider_configuration(&storage, &mut input);
        let trace =
            ConversationTraceSnapshot::default().in_progress_trace(RUN, CONVERSATION, ASSISTANT);
        let mut fixture = Self {
            _directory: directory,
            storage,
            service,
            input,
            trace,
            model_context: vec![],
        };
        fixture.append(COVERED);
        fixture.append(TAIL);
        fixture
    }

    fn append(&mut self, marker: &str) {
        self.stage(marker);
        self.storage
            .append_in_progress_conversation_turn_trace_and_apply_guidances(
                &self.trace,
                &self.model_context,
                2,
                2 + self.trace.items.len() as i64,
            )
            .unwrap();
    }

    fn stage(&mut self, marker: &str) {
        let sequence = self.trace.items.len() as u64;
        self.trace
            .items
            .push(ConversationTurnTraceItem::AssistantNarration {
                first_tool_call_id: None,
                provider_turn_id: None,
                sequence,
                content: marker.into(),
                truncated: false,
            });
        self.model_context.push(ConversationModelContextItem {
            sequence,
            ordinal: 0,
            role: "assistant".into(),
            content: marker.into(),
            tool_call_id: None,
            tool_calls: vec![],
            images: vec![],
            is_error: false,
        });
    }

    fn finish(&mut self) {
        self.trace.terminal_status = ConversationTurnTraceTerminalStatus::Completed;
        self.storage
            .finalize_chat_message_with_conversation_trace_model_context_and_usage(
                CONVERSATION,
                ASSISTANT,
                "FINAL_ANSWER",
                Some("sent"),
                "completed",
                &self.trace,
                Some(&self.model_context),
                2,
                10,
                None,
                None,
            )
            .unwrap();
    }

    fn compact(&self, cursor: ContextJournalCursor) {
        let prefix = self
            .storage
            .prepare_context_compaction_prefix(CONVERSATION, &cursor)
            .unwrap();
        self.storage
            .commit_context_compaction_prefix(
                &prefix,
                test_compaction_draft(&prefix, "summary-compacted-rebuild", SUMMARY, 500, 11),
                ASSISTANT,
            )
            .unwrap();
        self.service
            .invalidate_conversation_context_state(CONVERSATION);
    }

    fn projected_history(&self) -> String {
        let persisted = self
            .service
            .persisted_conversation_context_state(&self.input, CONVERSATION)
            .unwrap();
        serde_json::to_string(&json!({
            "summary": persisted.preview_input.context_compaction_summary,
            "messages": persisted.preview_input.messages,
        }))
        .unwrap()
    }

    fn committed_count(&self) -> usize {
        self.service
            .conversation_context_states
            .lock()
            .unwrap()
            .get(CONVERSATION)
            .unwrap()
            .committed_activity_items
    }
}

#[test]
fn partially_compacted_terminal_trace_rebuilds_without_resurrecting_covered_history() {
    let mut fixture = CompactedRebuildFixture::new();
    fixture.finish();
    fixture.compact(ContextJournalCursor::trace_item(ASSISTANT, 0));

    // Main and child terminal runners share this exact terminal-refresh entry point.
    let snapshot = fixture
        .service
        .finalize_conversation_context_state(
            &fixture.input,
            RUN,
            CONVERSATION,
            ASSISTANT,
            "FINAL_ANSWER",
        )
        .unwrap()
        .unwrap();
    assert!(snapshot.input_tokens > 0);
    let baseline = fixture.projected_history();
    assert!(baseline.contains(SUMMARY));
    assert!(!baseline.contains(COVERED));
    assert!(baseline.contains(TAIL));
    assert!(baseline.contains("FINAL_ANSWER"));
    assert_eq!(fixture.committed_count(), 2);
    assert_eq!(
        fixture
            .storage
            .get_conversation_model_context_log(ASSISTANT)
            .unwrap()
            .unwrap()
            .items,
        fixture.model_context
    );
}

#[test]
fn fully_compacted_latest_terminal_trace_keeps_full_journal_cursor_and_summary_only() {
    let mut fixture = CompactedRebuildFixture::new();
    fixture.finish();
    fixture.compact(ContextJournalCursor::message(ASSISTANT));

    let update = fixture
        .service
        .rebuild_conversation_context_state(&fixture.input, CONVERSATION, None, None, None)
        .unwrap();
    assert!(update.snapshot.is_some());
    let baseline = fixture.projected_history();
    assert!(baseline.contains(SUMMARY));
    assert!(!baseline.contains(COVERED));
    assert!(!baseline.contains(TAIL));
    assert!(!baseline.contains("FINAL_ANSWER"));
    assert_eq!(fixture.committed_count(), 2);
}

#[test]
fn compacted_running_trace_cache_rebuild_continues_from_full_cursor_without_duplicates() {
    let mut fixture = CompactedRebuildFixture::new();
    fixture.compact(ContextJournalCursor::trace_item(ASSISTANT, 0));
    fixture
        .service
        .rebuild_conversation_context_state(&fixture.input, CONVERSATION, Some(RUN), None, None)
        .unwrap();
    assert_eq!(fixture.committed_count(), 2);
    let before_baseline = fixture.projected_history();
    assert!(!before_baseline.contains(COVERED));
    assert!(before_baseline.contains(TAIL));
    fixture.stage(APPENDED);
    let configuration_revision =
        conversation_context_configuration_revision(&fixture.input).unwrap();
    let (notifications, _receiver) = tokio::sync::mpsc::unbounded_channel();
    fixture
        .service
        .persist_in_progress_trace_snapshot(
            RUN,
            CONVERSATION,
            ASSISTANT,
            2,
            &fixture.input,
            &notifications,
            &ConversationTraceSnapshot {
                items: fixture.trace.items.clone(),
                model_context_items: fixture.model_context.clone(),
                next_sequence: fixture.trace.items.len() as u64,
                truncated: false,
            },
            &configuration_revision,
            None,
        )
        .unwrap()
        .unwrap();
    let baseline = fixture.projected_history();
    assert!(baseline.contains(SUMMARY));
    assert!(!baseline.contains(COVERED));
    assert_eq!(
        baseline.matches(TAIL).count(),
        before_baseline.matches(TAIL).count()
    );
    assert_eq!(
        baseline.matches(APPENDED).count(),
        before_baseline.matches(TAIL).count()
    );
    assert_eq!(fixture.committed_count(), 3);
    let incremental_tokens = fixture
        .service
        .conversation_context_states
        .lock()
        .unwrap()
        .get_mut(CONVERSATION)
        .unwrap()
        .state
        .snapshot()
        .input_tokens;
    fixture
        .service
        .invalidate_conversation_context_state(CONVERSATION);
    let cold = fixture
        .service
        .rebuild_conversation_context_state(&fixture.input, CONVERSATION, Some(RUN), None, None)
        .unwrap();
    assert_eq!(incremental_tokens, cold.snapshot.unwrap().input_tokens);
}

#[test]
fn compacted_context_rebuild_still_rejects_a_genuinely_incomplete_full_journal() {
    let fixture = CompactedRebuildFixture::new();
    fixture.compact(ContextJournalCursor::trace_item(ASSISTANT, 0));
    // Deliberately corrupt only this temporary database: the surviving suffix is valid as
    // compacted history but must never disguise a missing committed model-context prefix.
    rusqlite::Connection::open(fixture._directory.path().join("storage.sqlite"))
        .unwrap()
        .execute(
            "DELETE FROM conversation_model_context_items WHERE assistant_message_id = ?1 AND sequence = 0",
            [ASSISTANT],
        ).unwrap();
    let result = fixture.service.rebuild_conversation_context_state(
        &fixture.input,
        CONVERSATION,
        Some(RUN),
        None,
        None,
    );
    let error = match result {
        Ok(_) => panic!("a corrupt full model-context journal must fail validation"),
        Err(error) => error,
    };
    assert!(
        error.contains("complete contiguous trace prefix"),
        "{error}"
    );
    assert!(!fixture
        .service
        .conversation_context_states
        .lock()
        .unwrap()
        .contains_key(CONVERSATION));
}
