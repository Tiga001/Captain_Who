use super::*;
use mycopilot_core::skills::{
    derive_skill_activation_ref, AgentDiscoverableSkill, AgentSkillDiscoverySnapshot,
    AGENT_SKILL_DISCOVERY_SCHEMA_VERSION,
};

const CONVERSATION: &str = "conversation-window-consistency";

struct WindowFixture {
    _directory: tempfile::TempDir,
    storage: Arc<StorageService>,
    service: AgentService,
    input: AgentChatInput,
    turn: usize,
}

fn message(id: &str, role: &str, content: &str, created_at: i64) -> ChatMessageRecord {
    ChatMessageRecord {
        id: id.into(),
        role: role.into(),
        content: content.into(),
        created_at,
        status: Some(
            if role == "assistant" && content.is_empty() {
                "pending"
            } else {
                "sent"
            }
            .into(),
        ),
        human_interaction_response: None,
        attachments: vec![],
        agent_run_json: None,
        ui_state_json: None,
    }
}

impl WindowFixture {
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
                title: "Context window consistency".into(),
                messages: vec![
                    message("old-user", "user", &"OLD_COVERED_REQUEST ".repeat(300), 1),
                    message(
                        "old-assistant",
                        "assistant",
                        &"OLD_COVERED_ANSWER ".repeat(300),
                        2,
                    ),
                ],
                created_at: 1,
                updated_at: 2,
                pinned_at: None,
                archived_at: None,
                unread_at: None,
            })
            .unwrap();
        storage
            .finalize_chat_message_with_conversation_trace(
                CONVERSATION,
                "old-assistant",
                &"OLD_COVERED_ANSWER ".repeat(300),
                Some("sent"),
                "completed",
                &completed_conversation_trace_without_items(
                    "run-window-old",
                    CONVERSATION,
                    "old-assistant",
                ),
                2,
                3,
            )
            .unwrap();
        let service = AgentService::new(storage.clone());
        let mut input: AgentChatInput = serde_json::from_value(json!({
            "apiUrl": "https://example.test/v1/chat/completions",
            "apiToken": "token", "model": "model-1",
            "modelCapabilities": {"imageInput": false},
            "contextWindowTokens": 128000,
            "contextWindowIndicatorEnabled": true,
            "maxTokens": 30000,
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
        let catalog = "context-consistency-catalog";
        let skill_id = "bundled:application:context-check";
        let revision = "context-check-revision";
        input.skill_discovery = Some(AgentSkillDiscoverySnapshot {
            schema_version: AGENT_SKILL_DISCOVERY_SCHEMA_VERSION,
            catalog_revision: catalog.into(),
            prompt_token_budget: 2000,
            skills: vec![AgentDiscoverableSkill {
                activation_ref: derive_skill_activation_ref(catalog, skill_id, revision),
                id: skill_id.into(),
                revision: revision.into(),
                name: "context-check".into(),
                description: "Check persistent context and explain the measured result. "
                    .repeat(25),
                source_kind: "bundled".into(),
            }],
            max_activated_skills: 8,
            max_total_source_bytes: 512 * 1024,
        });
        input.skill_discovery.as_ref().unwrap().validate().unwrap();
        freeze_test_pending_provider_configuration(&storage, &mut input);
        Self {
            _directory: directory,
            storage,
            service,
            input,
            turn: 0,
        }
    }

    fn run_id(&self) -> String {
        format!("run-window-{}", self.turn)
    }
    fn assistant_id(&self) -> String {
        format!("assistant-window-{}", self.turn)
    }

    fn start_turn(&mut self, content: &str) {
        self.turn += 1;
        let mut conversation = self
            .storage
            .load_conversation(CONVERSATION)
            .unwrap()
            .unwrap();
        let created_at = 10 * self.turn as i64;
        conversation.messages.push(message(
            &format!("user-window-{}", self.turn),
            "user",
            content,
            created_at,
        ));
        conversation.messages.push(message(
            &self.assistant_id(),
            "assistant",
            "",
            created_at + 1,
        ));
        conversation.updated_at = created_at + 1;
        self.storage.save_conversation(conversation).unwrap();
        let trace = ConversationTraceSnapshot::default().in_progress_trace(
            &self.run_id(),
            CONVERSATION,
            &self.assistant_id(),
        );
        self.storage
            .append_in_progress_conversation_turn_trace(&trace, created_at + 1, created_at + 1)
            .unwrap();
        self.input.assistant_message_id = Some(self.assistant_id());
        self.service
            .invalidate_conversation_context_state(CONVERSATION);
        self.service
            .rebuild_conversation_context_state(
                &self.input,
                CONVERSATION,
                Some(&self.run_id()),
                None,
                None,
            )
            .unwrap();
    }

    fn finish_turn(&self, content: &str) -> AgentContextWindowSnapshot {
        let trace = completed_conversation_trace_without_items(
            &self.run_id(),
            CONVERSATION,
            &self.assistant_id(),
        );
        self.storage
            .finalize_chat_message_with_conversation_trace(
                CONVERSATION,
                &self.assistant_id(),
                content,
                Some("sent"),
                "completed",
                &trace,
                10 * self.turn as i64 + 1,
                10 * self.turn as i64 + 2,
            )
            .unwrap();
        self.terminal(content)
    }

    fn terminal(&self, content: &str) -> AgentContextWindowSnapshot {
        // Root and child terminal workers both use this shared entry point.
        self.service
            .finalize_conversation_context_state(
                &self.input,
                &self.run_id(),
                CONVERSATION,
                &self.assistant_id(),
                content,
            )
            .unwrap()
            .unwrap()
    }

    fn preview_input(&self) -> AgentChatInput {
        let mut input = self
            .service
            .persisted_conversation_context_state(&self.input, CONVERSATION)
            .unwrap()
            .preview_input;
        input.skill_activation = None;
        input.assistant_message_id = None;
        if let Some(preferences) = input.prompt_preferences.as_mut() {
            preferences.automation_execution_context = None;
        }
        input
    }

    fn preview(&self, input: &AgentChatInput) -> AgentContextWindowSnapshot {
        let projection = self
            .service
            .context_window_tool_projection(input, None)
            .unwrap();
        self.service
            .context_window_snapshot_with_projection_cache(input, CONVERSATION, &projection)
            .unwrap()
            .unwrap()
    }

    fn assert_all_read_paths(&self, expected: &AgentContextWindowSnapshot, final_content: &str) {
        let input = self.preview_input();
        let projection = self
            .service
            .context_window_tool_projection(&input, None)
            .unwrap();
        assert!(
            !projection.dynamic_definitions().is_empty(),
            "fixture must exercise dynamic schemas"
        );
        let direct = inspect_context_window_with_tool_projection(input.clone(), &projection)
            .unwrap()
            .unwrap();
        assert_eq!(
            &direct, expected,
            "terminal must measure the complete next request preview"
        );
        for _ in 0..3 {
            assert_eq!(
                self.preview(&input),
                direct,
                "repeated reads must not add overlays again"
            );
        }
        self.service
            .invalidate_conversation_context_state(CONVERSATION);
        assert_eq!(
            self.preview(&input),
            direct,
            "cache eviction must not change accounting"
        );
        self.service
            .invalidate_conversation_context_state(CONVERSATION);
        assert_eq!(
            self.terminal(final_content),
            direct,
            "cold terminal refresh must use the same projection"
        );
    }
}

#[test]
fn completed_turn_preview_is_identical_across_hot_cold_and_direct_read_paths() {
    let mut fixture = WindowFixture::new();
    fixture.start_turn("Check the current state.");
    let snapshot = fixture.finish_turn("The current state is checked.");
    fixture.assert_all_read_paths(&snapshot, "The current state is checked.");
    assert!(snapshot.cost_breakdown.world_state_tokens > 0);
    assert!(snapshot.cost_breakdown.tool_schema_tokens > 0);
}

#[test]
fn terminal_preview_retires_automation_execution_but_preserves_user_preferences() {
    let mut fixture = WindowFixture::new();
    let mut preferences: mycopilot_core::AgentPromptPreferences = serde_json::from_value(json!({
        "customInstructions": "PERSISTENT_USER_PREFERENCE ".repeat(80),
        "updatedAt": 42,
    }))
    .unwrap();
    preferences.automation_execution_context = Some(
        mycopilot_core::AgentAutomationExecutionContext::new(
            "automation-window-consistency",
            "automation-run-window-consistency",
            1_725_000_000_000,
            Some(1_724_000_000_000),
            "scheduled",
        )
        .unwrap(),
    );
    fixture.input.prompt_preferences = Some(preferences);
    fixture.start_turn("Inspect the scheduled task.");
    let snapshot = fixture.finish_turn("Scheduled inspection complete.");
    fixture.assert_all_read_paths(&snapshot, "Scheduled inspection complete.");

    let mut without_preferences = fixture.preview_input();
    without_preferences.prompt_preferences = None;
    assert!(
        snapshot.input_tokens > fixture.preview(&without_preferences).input_tokens,
        "retiring automation metadata must preserve the user's ordinary preferences"
    );
    assert_eq!(fixture.preview(&fixture.preview_input()), snapshot);
}

#[test]
fn terminal_preview_counts_discovery_once_and_retires_completed_skill_activation() {
    let mut fixture = WindowFixture::new();
    fixture.input.skill_activation = Some(AgentSkillActivation {
        activation_revision: "skill-activation-sha256-v1:context-consistency".into(),
        skills: vec![AgentActivatedSkill {
            id: "bundled:application:context-check".into(),
            name: "context-check".into(),
            revision: "context-check-revision".into(),
            source: "bundled:application".into(),
            instructions: "COMPLETED_RUN_INSTRUCTIONS ".repeat(100),
            source_bytes: 2700,
            resources: None,
        }],
    });
    fixture.start_turn("Use the selected skill for this turn.");
    let snapshot = fixture.finish_turn("Skill review complete.");
    fixture.assert_all_read_paths(&snapshot, "Skill review complete.");

    let mut without_discovery = fixture.preview_input();
    without_discovery.skill_discovery = None;
    let without = fixture.preview(&without_discovery);
    assert!(
        snapshot.input_tokens > without.input_tokens,
        "retiring activation must not also drop available-skill discovery"
    );
    assert_eq!(fixture.preview(&fixture.preview_input()), snapshot);
}

#[test]
fn text_append_grows_the_same_measurement_and_compaction_can_reduce_it() {
    let mut fixture = WindowFixture::new();
    fixture.start_turn("123");
    let before = fixture.finish_turn("Received the first message.");
    fixture.assert_all_read_paths(&before, "Received the first message.");

    fixture.start_turn("1");
    let after = fixture.finish_turn("Received the next message.");
    fixture.assert_all_read_paths(&after, "Received the next message.");
    assert!(
        after.input_tokens > before.input_tokens,
        "a plain text turn must not drop request layers when the UI changes entry point"
    );
    assert_eq!(
        after.cost_breakdown.tool_schema_tokens,
        before.cost_breakdown.tool_schema_tokens
    );

    let prefix = fixture
        .storage
        .prepare_context_compaction_prefix(
            CONVERSATION,
            &ContextJournalCursor::message("old-assistant"),
        )
        .unwrap();
    fixture
        .storage
        .commit_context_compaction_prefix(
            &prefix,
            test_compaction_draft(
                &prefix,
                "summary-window-consistency",
                "Earlier work is complete.",
                5000,
                100,
            ),
            &fixture.assistant_id(),
        )
        .unwrap();
    fixture
        .service
        .invalidate_conversation_context_state(CONVERSATION);
    let compacted = fixture.terminal("Received the next message.");
    fixture.assert_all_read_paths(&compacted, "Received the next message.");
    assert!(
        compacted.input_tokens < after.input_tokens,
        "real compaction may legitimately lower occupancy"
    );
    assert!(compacted.cost_breakdown.summary_tokens > 0);
    assert_eq!(
        compacted.cost_breakdown.tool_schema_tokens,
        after.cost_breakdown.tool_schema_tokens
    );
}

fn save_context_profile(storage: &StorageService, profile: mycopilot_core::AgentContextProfile) {
    let mut preferences = storage.load_agent_prompt_preferences().unwrap();
    preferences.context_profile = profile;
    storage.save_agent_prompt_preferences(preferences).unwrap();
}

fn saved_profile_snapshot(service: &AgentService) -> AgentContextWindowSnapshot {
    service
        .get_context_window_snapshot(AgentContextWindowSnapshotInput {
            conversation_id: Some(CONVERSATION.into()),
            project_id: None,
            model_id: "model-1".into(),
            max_tokens: Some(30_000),
            prompt_preferences: None,
            permissions: AgentPermissions::default(),
            skills: vec![],
        })
        .unwrap()
        .snapshot
        .unwrap()
}

#[test]
fn context_profile_idle_toggle_refreshes_measurement_without_rewriting_history() {
    use mycopilot_core::AgentContextProfile::{Full, Minimal};

    let fixture = WindowFixture::new();
    let before = fixture
        .storage
        .load_conversation_for_turn(CONVERSATION)
        .unwrap();
    let traces_before = fixture
        .storage
        .list_conversation_turn_traces(CONVERSATION)
        .unwrap();
    let full = saved_profile_snapshot(&fixture.service);
    save_context_profile(&fixture.storage, Minimal);
    let minimal = saved_profile_snapshot(&fixture.service);
    assert!(minimal.input_tokens < full.input_tokens);
    assert!(minimal.cost_breakdown.tool_schema_tokens < full.cost_breakdown.tool_schema_tokens);
    assert_eq!(minimal.input_capacity_tokens, full.input_capacity_tokens);

    let reopened =
        Arc::new(StorageService::open(&fixture._directory.path().join("storage.sqlite")).unwrap());
    assert_eq!(
        reopened
            .load_agent_prompt_preferences()
            .unwrap()
            .context_profile,
        Minimal
    );
    save_context_profile(&fixture.storage, Full);
    assert_eq!(saved_profile_snapshot(&fixture.service), full);
    assert_eq!(
        serde_json::to_value(
            fixture
                .storage
                .load_conversation_for_turn(CONVERSATION)
                .unwrap()
        )
        .unwrap(),
        serde_json::to_value(before).unwrap(),
    );
    assert_eq!(
        fixture
            .storage
            .list_conversation_turn_traces(CONVERSATION)
            .unwrap(),
        traces_before
    );
}

#[test]
fn context_profile_active_run_stays_frozen_and_terminal_previews_next_mode() {
    use mycopilot_core::AgentContextProfile::{Full, Minimal};

    let fixture = WindowFixture::new();
    save_context_profile(&fixture.storage, Minimal);
    let (existing, revision) = fixture
        .storage
        .load_conversation_for_turn(CONVERSATION)
        .unwrap();
    let run_id = "run-minimal-frozen-window";
    let prepared = crate::application::agent_support::prepare_reserved_human_turn(
        &fixture.storage,
        &fixture.service.skills,
        serde_json::from_value(json!({
            "conversationId": CONVERSATION,
            "modelId": "model-1",
            "content": "Keep the current task in its original mode.",
            "userMessageId": "user-minimal-window",
            "assistantMessageId": "assistant-minimal-window",
            "maxTokens": 30000
        }))
        .unwrap(),
        run_id,
        existing,
        revision,
        None,
        None,
    )
    .unwrap();
    assert_eq!(
        prepared
            .agent_input
            .prompt_preferences
            .as_ref()
            .unwrap()
            .context_profile,
        Minimal
    );
    assert_eq!(
        fixture
            .storage
            .load_agent_context_profile_for_run(run_id)
            .unwrap(),
        Some(Minimal)
    );
    let active = saved_profile_snapshot(&fixture.service);
    save_context_profile(&fixture.storage, Full);
    assert_eq!(saved_profile_snapshot(&fixture.service), active);
    let reopened =
        Arc::new(StorageService::open(&fixture._directory.path().join("storage.sqlite")).unwrap());
    assert_eq!(
        reopened.load_agent_context_profile_for_run(run_id).unwrap(),
        Some(Minimal)
    );

    let assistant_id = &prepared.output.assistant_message_id;
    let trace = completed_conversation_trace_without_items(run_id, CONVERSATION, assistant_id);
    fixture
        .storage
        .finalize_chat_message_with_conversation_trace(
            CONVERSATION,
            assistant_id,
            "Done.",
            Some("sent"),
            "completed",
            &trace,
            prepared.output.assistant_message.created_at,
            prepared.output.assistant_message.created_at + 1,
        )
        .unwrap();
    let terminal = fixture
        .service
        .finalize_conversation_context_state(
            &prepared.agent_input,
            run_id,
            CONVERSATION,
            assistant_id,
            "Done.",
        )
        .unwrap()
        .unwrap();
    assert!(terminal.cost_breakdown.tool_schema_tokens > active.cost_breakdown.tool_schema_tokens);
    assert_eq!(saved_profile_snapshot(&fixture.service), terminal);
    assert_eq!(
        fixture
            .storage
            .load_agent_context_profile_for_run(run_id)
            .unwrap(),
        Some(Minimal)
    );
}
