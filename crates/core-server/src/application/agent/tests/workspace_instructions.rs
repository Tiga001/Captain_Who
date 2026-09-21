//! Workspace `AGENTS.md` instructions flow through the real World State host as a model-visible
//! section that appears, replaces itself and disappears as the file changes.
use super::*;

const CONVERSATION: &str = "conversation-workspace-instructions";
const RUN: &str = "run-workspace-instructions";
const ASSISTANT: &str = "assistant-workspace-instructions";
const PROJECT: &str = "project-workspace-instructions";

struct InstructionsFixture {
    service: AgentService,
    storage: Arc<StorageService>,
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
        let (existing, expected_revision) =
            storage.load_conversation_for_turn(CONVERSATION).unwrap();
        // The durable reservation is the production entry that also freezes the run workspace,
        // so both Host commits and context-window previews run against the same frozen root.
        let prepared = crate::application::agent_support::prepare_reserved_human_turn(
            &storage,
            &SkillsService::new(),
            AgentConversationTurnInput {
                conversation_id: Some(CONVERSATION.to_string()),
                project_id: Some(PROJECT.to_string()),
                model_id: "model-1".to_string(),
                context_window_indicator_enabled: true,
                content: "遵守项目约定。".to_string(),
                attachments: Vec::new(),
                folder_references: Vec::new(),
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
            existing,
            expected_revision,
            None,
            None,
            &|| Ok(()),
        )
        .unwrap();
        Self {
            service,
            storage,
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

    /// Reads the real public context-window preview (the same API the context ring uses).
    fn preview(&self) -> mycopilot_core::AgentContextWindowSnapshot {
        self.service
            .get_context_window_snapshot(AgentContextWindowSnapshotInput {
                conversation_id: Some(CONVERSATION.to_string()),
                project_id: Some(PROJECT.to_string()),
                model_id: "model-1".to_string(),
                max_tokens: None,
                prompt_preferences: None,
                permissions: AgentPermissions::default(),
                skills: Vec::new(),
            })
            .unwrap()
            .snapshot
            .unwrap()
    }
}

fn fold_records(
    records: &[mycopilot_core::AnchoredWorldStateRecord],
) -> mycopilot_core::WorldStateSnapshot {
    let mut iter = records.iter();
    let first = iter.next().expect("world state records");
    let mycopilot_core::WorldStateRecord::Full(full) = &first.record else {
        panic!("the first world state record must be full");
    };
    let diffs = iter
        .map(|entry| match &entry.record {
            mycopilot_core::WorldStateRecord::Diff(diff) => diff.clone(),
            mycopilot_core::WorldStateRecord::Full(_) => {
                panic!("later world state records must be diffs")
            }
        })
        .collect::<Vec<_>>();
    mycopilot_core::WorldStateReducer::fold(full.clone(), &diffs).unwrap()
}

fn section_text(snapshot: &mycopilot_core::WorldStateSnapshot) -> String {
    snapshot
        .sections
        .iter()
        .flat_map(|section| {
            std::iter::once(section.state.to_string()).chain(
                section
                    .model_projection
                    .iter()
                    .map(|projection| projection.to_string()),
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
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

#[test]
fn context_window_preview_rediscovers_agents_md_changes_like_the_next_request() {
    let fixture = InstructionsFixture::new();
    let baseline = fixture.preview();

    // A large new file must move the preview before any request commits it. The preview used to
    // keep counting the old state because it never re-read the file.
    let long_v1 = format!("# 约定\n\n{}", "第一版约定说明。\n".repeat(800));
    let long_v2 = format!("{long_v1}{}", "第二版约定说明。\n".repeat(800));
    fs::write(fixture.project_dir.path().join("AGENTS.md"), &long_v1).unwrap();
    let added = fixture.preview();
    assert!(
        added.input_tokens > baseline.input_tokens,
        "adding AGENTS.md must change the context preview"
    );

    // The next real request commits exactly the content the preview already showed.
    let records = fixture.commit(1);
    let committed = fold_records(&records);
    let instructions = committed
        .section(&mycopilot_core::WorldStateSectionId::WorkspaceInstructions)
        .expect("the request must publish workspace.instructions");
    assert_eq!(
        instructions.model_projection.as_ref().unwrap()["sources"][0]["content"],
        json!(long_v1)
    );
    let durable_after_commit =
        load_conversation_world_state(&fixture.storage, CONVERSATION).unwrap();
    let steady_v1 = fixture.preview();

    // An edit on disk must move the preview even while the old content stays persisted.
    fs::write(fixture.project_dir.path().join("AGENTS.md"), &long_v2).unwrap();
    let modified = fixture.preview();
    assert!(
        modified.input_tokens > steady_v1.input_tokens,
        "editing AGENTS.md must change the context preview"
    );

    // Deleting the file must move the preview too, and committing the deletion removes the
    // persisted section instead of leaving stale conventions behind.
    fs::remove_file(fixture.project_dir.path().join("AGENTS.md")).unwrap();
    let removed = fixture.preview();
    assert!(
        removed.input_tokens > steady_v1.input_tokens,
        "removing AGENTS.md must change the context preview"
    );
    assert_eq!(
        load_conversation_world_state(&fixture.storage, CONVERSATION).unwrap(),
        durable_after_commit,
        "previews must not publish file changes as durable state"
    );
    let records = fixture.commit(2);
    let committed = fold_records(&records);
    assert!(
        committed
            .section(&mycopilot_core::WorldStateSectionId::WorkspaceInstructions)
            .is_none(),
        "the next request must remove the deleted conventions"
    );
    let steady_after = fixture.preview();
    assert_eq!(
        steady_after,
        fixture.preview(),
        "an idle preview must be stable"
    );
}

#[cfg(unix)]
#[test]
fn symlinked_agents_md_and_replaced_root_never_redirect_discovery() {
    let fixture = InstructionsFixture::new();
    let outside = tempdir().unwrap();
    fs::write(outside.path().join("AGENTS.md"), "outside secret").unwrap();
    let clean = fixture.preview();

    // A workspace AGENTS.md symlinked to a file outside the workspace contributes nothing: the
    // preview does not move and the committed section stays absent.
    std::os::unix::fs::symlink(
        outside.path().join("AGENTS.md"),
        fixture.project_dir.path().join("AGENTS.md"),
    )
    .unwrap();
    assert_eq!(
        fixture.preview(),
        clean,
        "a symlinked AGENTS.md must not leak outside content into the preview"
    );
    let first = fold_records(&fixture.commit(1));
    assert!(first
        .section(&mycopilot_core::WorldStateSectionId::WorkspaceInstructions)
        .is_none());
    assert!(!section_text(&first).contains("outside secret"));

    // A real file is still discovered after the symlink is removed.
    fs::remove_file(fixture.project_dir.path().join("AGENTS.md")).unwrap();
    let long = format!("# 约定\n\n{}", "项目约定行。\n".repeat(400));
    fs::write(fixture.project_dir.path().join("AGENTS.md"), &long).unwrap();
    let populated = fixture.preview();
    assert!(
        populated.input_tokens > clean.input_tokens,
        "the preview must discover a real AGENTS.md"
    );
    let second = fold_records(&fixture.commit(2));
    assert_eq!(
        second
            .section(&mycopilot_core::WorldStateSectionId::WorkspaceInstructions)
            .unwrap()
            .model_projection
            .as_ref()
            .unwrap()["sources"][0]["content"],
        json!(long)
    );

    // Replacing the frozen root with a symlink to an outside directory must not redirect the
    // read: the section is removed and the outside content never reaches the model context.
    fs::remove_file(fixture.project_dir.path().join("AGENTS.md")).unwrap();
    fs::remove_dir(fixture.project_dir.path()).unwrap();
    std::os::unix::fs::symlink(outside.path(), fixture.project_dir.path()).unwrap();
    let third = fold_records(&fixture.commit(3));
    assert!(third
        .section(&mycopilot_core::WorldStateSectionId::WorkspaceInstructions)
        .is_none());
    assert!(!section_text(&third).contains("outside secret"));
    // Clean up the replacement so the fixture directory drops cleanly.
    fs::remove_file(fixture.project_dir.path()).unwrap();
}
