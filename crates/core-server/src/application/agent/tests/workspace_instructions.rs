//! Workspace `AGENTS.md` instructions flow through the real World State host as a model-visible
//! section that appears, replaces itself and disappears as the file changes.
use super::*;

const CONVERSATION: &str = "conversation-workspace-instructions";
const RUN: &str = "run-workspace-instructions";
const ASSISTANT: &str = "assistant-workspace-instructions";
const PROJECT: &str = "project-workspace-instructions";

struct InstructionsFixture {
    service: AgentService,
    agent_input: AgentChatInput,
    project_dir: tempfile::TempDir,
    _storage_dir: tempfile::TempDir,
}

impl InstructionsFixture {
    fn new() -> Self {
        let storage_dir = tempdir().unwrap();
        let project_dir = tempdir().unwrap();
        let storage =
            Arc::new(StorageService::open(&storage_dir.path().join("storage.sqlite")).unwrap());
        let service = AgentService::new_authorized_for_test(storage.clone());
        storage.save_model_settings(test_model_settings()).unwrap();
        storage
            .save_project(ProjectRecord::with_primary_folder(
                PROJECT,
                "Instructions",
                project_dir.path().to_string_lossy().into_owned(),
                1,
            ))
            .unwrap();
        let prepared = prepare_conversation_turn(
            &storage,
            &SkillsService::new(),
            AgentConversationTurnInput {
                conversation_id: Some(CONVERSATION.to_string()),
                project_id: Some(PROJECT.to_string()),
                model_id: "model-1".to_string(),
                context_window_indicator_enabled: true,
                content: "遵守项目约定。".to_string(),
                attachments: Vec::new(),
                skills: Vec::new(),
                title: None,
                user_message_id: Some("user-workspace-instructions".to_string()),
                assistant_message_id: Some(ASSISTANT.to_string()),
                max_tokens: None,
                temperature: None,
                prompt_preferences: None,
                permissions: AgentPermissions::default(),
            },
            RUN,
        )
        .unwrap();
        let trace = mycopilot_core::ConversationTraceSnapshot::default().in_progress_trace(
            RUN,
            CONVERSATION,
            ASSISTANT,
        );
        storage
            .append_in_progress_conversation_turn_trace(&trace, 1, 1)
            .unwrap();
        Self {
            service,
            agent_input: prepared.agent_input,
            project_dir,
            _storage_dir: storage_dir,
        }
    }

    /// Drives one real sampling boundary through the Host and returns the active journal.
    fn commit(&self, request_index: u64) -> Vec<mycopilot_core::AnchoredWorldStateRecord> {
        let boundary = mycopilot_core::WorldStateRequestBoundary {
            run_id: RUN.into(),
            assistant_message_id: ASSISTANT.into(),
            request_index,
            after_trace_sequence: None,
        };
        let host = self.service.conversation_world_state_host(
            &self.agent_input,
            RUN,
            CONVERSATION,
            ASSISTANT,
            &AgentCancellationToken::new(),
        );
        let records = host
            .prepare_request(mycopilot_core::AgentConversationWorldStateRequest {
                conversation_id: CONVERSATION.to_string(),
                boundary: boundary.clone(),
                sections: Vec::new(),
            })
            .unwrap();
        host.mark_request_observed(&boundary).unwrap();
        records
    }
}

#[test]
fn workspace_instructions_publish_replace_and_stay_quiet_when_unchanged() {
    let fixture = InstructionsFixture::new();
    fs::write(
        fixture.project_dir.path().join("AGENTS.md"),
        "# 项目约定\n\n- 使用 pnpm 安装依赖。\n",
    )
    .unwrap();

    let first_records = fixture.commit(1);
    assert_eq!(first_records.len(), 1);
    let mycopilot_core::WorldStateRecord::Full(first_snapshot) = &first_records[0].record else {
        panic!("the first committed record must be full");
    };
    let instructions = first_snapshot
        .section(&mycopilot_core::WorldStateSectionId::WorkspaceInstructions)
        .expect("AGENTS.md must be published as workspace.instructions");
    let projection = instructions.model_projection.as_ref().unwrap();
    assert_eq!(projection["sources"][0]["path"], json!("AGENTS.md"));
    assert_eq!(
        projection["sources"][0]["content"],
        json!("# 项目约定\n\n- 使用 pnpm 安装依赖。\n")
    );
    let scope = projection["sources"][0]["scope"].as_str().unwrap();
    assert!(scope.starts_with("@workspace/"));
    let canonical_project = fixture.project_dir.path().canonicalize().unwrap();
    assert!(!projection
        .to_string()
        .contains(canonical_project.to_string_lossy().as_ref()));
    assert!(instructions.state.to_string().contains("sha256:"));
    assert!(!projection.to_string().contains("sha256:"));
    let first_revision = instructions.revision.clone();

    // An unchanged file keeps the revision and appends no record at the next boundary.
    let second_records = fixture.commit(2);
    assert_eq!(second_records.len(), 1);

    // An edit publishes a Replace that only touches this section.
    fs::write(
        fixture.project_dir.path().join("AGENTS.md"),
        "# 项目约定 v2\n\n- 使用 cargo test --locked。\n",
    )
    .unwrap();
    let third_records = fixture.commit(3);
    assert_eq!(third_records.len(), 2);
    let mycopilot_core::WorldStateRecord::Diff(diff) = &third_records[1].record else {
        panic!("a changed AGENTS.md must append a diff");
    };
    let updated_snapshot =
        mycopilot_core::WorldStateReducer::fold(first_snapshot.clone(), std::slice::from_ref(diff))
            .unwrap();
    let updated = updated_snapshot
        .section(&mycopilot_core::WorldStateSectionId::WorkspaceInstructions)
        .unwrap();
    assert_ne!(updated.revision, first_revision);
    assert_eq!(
        updated.model_projection.as_ref().unwrap()["sources"][0]["content"],
        json!("# 项目约定 v2\n\n- 使用 cargo test --locked。\n")
    );
    let rendered = diff
        .model_projection_against(
            first_snapshot,
            mycopilot_core::WorldStateLifetime::Conversation,
        )
        .unwrap()
        .unwrap()
        .render_sanitized_text();
    assert!(rendered.contains("workspace.instructions"));
    assert!(rendered.contains("cargo test --locked"));
}

#[test]
fn workspace_instructions_override_wins_and_removal_is_explicit() {
    let fixture = InstructionsFixture::new();
    fs::write(
        fixture.project_dir.path().join("AGENTS.md"),
        "base instructions",
    )
    .unwrap();
    fs::write(
        fixture.project_dir.path().join("AGENTS.override.md"),
        "override instructions",
    )
    .unwrap();

    let first_records = fixture.commit(1);
    let mycopilot_core::WorldStateRecord::Full(first_snapshot) = &first_records[0].record else {
        panic!("the first committed record must be full");
    };
    let instructions = first_snapshot
        .section(&mycopilot_core::WorldStateSectionId::WorkspaceInstructions)
        .unwrap();
    assert_eq!(
        instructions.model_projection.as_ref().unwrap()["sources"][0]["path"],
        json!("AGENTS.override.md")
    );
    assert_eq!(
        instructions.model_projection.as_ref().unwrap()["sources"][0]["content"],
        json!("override instructions")
    );

    // Deleting the instruction files removes the section from the model-visible state.
    fs::remove_file(fixture.project_dir.path().join("AGENTS.md")).unwrap();
    fs::remove_file(fixture.project_dir.path().join("AGENTS.override.md")).unwrap();
    let second_records = fixture.commit(2);
    assert_eq!(second_records.len(), 2);
    let mycopilot_core::WorldStateRecord::Diff(diff) = &second_records[1].record else {
        panic!("a removed AGENTS.md must append a diff");
    };
    let removal = diff
        .model_projection_against(
            first_snapshot,
            mycopilot_core::WorldStateLifetime::Conversation,
        )
        .unwrap()
        .unwrap();
    match &removal {
        mycopilot_core::WorldStateModelRecord::Diff { changes, .. } => {
            assert_eq!(changes.len(), 1);
            assert!(matches!(
                &changes[0],
                mycopilot_core::WorldStateModelChange::Remove { section_id }
                    if section_id.as_str() == "workspace.instructions"
            ));
        }
        other => panic!("expected a diff, got {other:?}"),
    }
    let removed_snapshot =
        mycopilot_core::WorldStateReducer::fold(first_snapshot.clone(), std::slice::from_ref(diff))
            .unwrap();
    assert!(removed_snapshot
        .section(&mycopilot_core::WorldStateSectionId::WorkspaceInstructions)
        .is_none());
}
