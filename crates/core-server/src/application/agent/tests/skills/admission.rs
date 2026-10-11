use super::*;
use crate::application::agent_support::prepare_reserved_human_turn;
use mycopilot_core::image_generation::InMemoryCredentialStore;
use mycopilot_protocol_rs::SkillSelectionDto;
use std::sync::atomic::{AtomicUsize, Ordering};

const CONVERSATION: &str = "conversation-skill-admission";
const RUN: &str = "run-skill-admission";
const INSTRUCTIONS: &str = "SKILL_ADMISSION_PRIVATE_INSTRUCTIONS";

struct Fixture {
    directory: tempfile::TempDir,
    storage: StorageService,
    credentials: Arc<InMemoryCredentialStore>,
    skills: SkillsService,
    selection: SkillSelectionDto,
}

impl Fixture {
    fn new() -> Self {
        let directory = tempdir().unwrap();
        let workspace = directory.path().join("workspace");
        write_test_skill(&workspace, INSTRUCTIONS);
        let credentials = Arc::new(InMemoryCredentialStore::default());
        let storage = StorageService::open_with_model_credentials(
            &directory.path().join("storage.sqlite"),
            credentials.clone(),
        )
        .unwrap();
        storage.save_model_settings(test_model_settings()).unwrap();
        storage
            .save_project(ProjectRecord::with_primary_folder(
                "project-skill-admission",
                "Skill admission fixture",
                workspace.to_string_lossy().into_owned(),
                1,
            ))
            .unwrap();
        let skills = SkillsService::new();
        let catalog = skills
            .list_workspace("project-skill-admission", &workspace)
            .unwrap();
        let descriptor = catalog.skills().first().unwrap();
        let selection = SkillSelectionDto {
            id: descriptor.id().as_str().to_string(),
            revision: descriptor.revision().as_str().to_string(),
        };
        Self {
            directory,
            storage,
            credentials,
            skills,
            selection,
        }
    }

    fn input(&self) -> AgentConversationTurnInput {
        skill_turn_input(
            "project-skill-admission",
            self.selection.clone(),
            CONVERSATION,
        )
    }
}

#[test]
fn selected_skills_are_durable_before_runtime_and_survive_reopen_and_canonicalization() {
    let fixture = Fixture::new();
    // Use real durable admission but never launch a Runtime or construct AgentService, whose
    // startup reconciliation would otherwise retire this deliberately unstarted Turn.
    let prepared = prepare_reserved_human_turn(
        &fixture.storage,
        &fixture.skills,
        fixture.input(),
        RUN,
        None,
        None,
        None,
        None,
        &|| Ok(()),
    )
    .unwrap();
    let assistant_id = prepared.output.assistant_message_id.clone();
    let expected_selections = json!([fixture.selection]);
    let expected_summaries = json!(prepared.output.activated_skills);
    let expected_revision = json!(prepared.output.skill_activation_revision);
    let assert_skills = |run: &Value| {
        assert_eq!(run["runId"], RUN);
        assert_eq!(run["explicitSkillSelections"], expected_selections);
        assert_eq!(run["activatedSkills"], expected_summaries);
        assert_eq!(run["skillActivationRevision"], expected_revision);
        let encoded = run.to_string();
        assert!(!encoded.contains(INSTRUCTIONS));
        assert!(!encoded.contains("DESCRIPTION_DISCOVERY_ONLY"));
    };
    let trace = fixture
        .storage
        .get_conversation_turn_trace(&assistant_id)
        .unwrap()
        .unwrap();
    assert_eq!(trace.run_id, RUN);
    assert_eq!(
        trace.terminal_status,
        ConversationTurnTraceTerminalStatus::InProgress
    );
    assert!(trace.items.is_empty());
    assert_skills(
        &serde_json::from_str::<Value>(
            prepared
                .output
                .assistant_message
                .agent_run_json
                .as_deref()
                .unwrap(),
        )
        .unwrap(),
    );
    drop(prepared);
    let Fixture {
        directory,
        storage,
        credentials,
        ..
    } = fixture;
    drop(storage);

    let database = directory.path().join("storage.sqlite");
    // Inspect the raw durable row before any load overlay or checkpoint can fill in fields.
    let raw: String = rusqlite::Connection::open(&database)
        .unwrap()
        .query_row(
            "SELECT agent_run_json FROM messages WHERE id=?1",
            [&assistant_id],
            |row| row.get(0),
        )
        .unwrap();
    let raw: Value = serde_json::from_str(&raw).unwrap();
    assert_skills(&raw);
    assert!(raw.get("coreCheckpoint").is_none());

    let reopened = StorageService::open_with_model_credentials(&database, credentials).unwrap();
    let conversation = reopened.load_conversation(CONVERSATION).unwrap().unwrap();
    assert_eq!(conversation.messages.len(), 2);
    assert_eq!(conversation.messages[0].content, "Review this repository.");
    assert_skills(
        &serde_json::from_str::<Value>(conversation.messages[1].agent_run_json.as_deref().unwrap())
            .unwrap(),
    );

    // The strict Core begin path and later lifecycle canonicalization must retain the snapshot.
    let checkpoint = reopened
        .begin_runtime_checkpoint(CONVERSATION, &assistant_id, RUN)
        .unwrap()
        .unwrap();
    assert_eq!(checkpoint.sequence, 0);
    assert_skills(&serde_json::from_str::<Value>(&checkpoint.projection_json).unwrap());
    assert_eq!(
        reopened
            .reconcile_orphaned_in_progress_conversation_turn_traces(&HashSet::new(), now_ms())
            .unwrap(),
        1
    );
    let recovered = reopened.load_conversation(CONVERSATION).unwrap().unwrap();
    let recovered: Value =
        serde_json::from_str(recovered.messages[1].agent_run_json.as_deref().unwrap()).unwrap();
    assert_eq!(recovered["status"], "failed");
    assert_skills(&recovered);
}

#[test]
fn failed_final_admission_fence_rolls_back_selected_skills_with_messages_and_trace() {
    let fixture = Fixture::new();
    let checks = AtomicUsize::new(0);
    let result = prepare_reserved_human_turn(
        &fixture.storage,
        &fixture.skills,
        fixture.input(),
        RUN,
        None,
        None,
        None,
        None,
        &|| {
            if checks.fetch_add(1, Ordering::SeqCst) == 0 {
                Ok(())
            } else {
                Err("injected final admission fence".to_string().into())
            }
        },
    );
    assert!(result.is_err());
    assert_eq!(checks.load(Ordering::SeqCst), 2);
    assert!(fixture
        .storage
        .load_conversation(CONVERSATION)
        .unwrap()
        .is_none());
    assert!(fixture
        .storage
        .list_conversation_turn_traces(CONVERSATION)
        .unwrap()
        .is_empty());
    let connection =
        rusqlite::Connection::open(fixture.directory.path().join("storage.sqlite")).unwrap();
    let count: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM messages WHERE conversation_id=?1",
            [CONVERSATION],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(count, 0);
}
