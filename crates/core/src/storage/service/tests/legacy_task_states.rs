use super::*;
use rusqlite::params;

#[test]
fn legacy_task_state_is_exportable_but_never_migrated_into_goal_state() {
    let fixture = StorageFixture::new();
    let service = fixture.service();
    service
        .save_conversation(conversation(
            "conversation-legacy-task",
            None,
            "legacy-source",
        ))
        .unwrap();

    {
        let connection = service.state.connection().unwrap();
        connection
            .execute(
                "INSERT INTO task_control_states (
                    task_id, conversation_id, schema_version, objective, source_message_id,
                    status, revision, current_run_id, stopped_reason, checkpoint_json,
                    created_at, updated_at
                 ) VALUES (?1, ?2, 1, ?3, ?4, 'active', 1, NULL, NULL, '{}', 1, 1)",
                params![
                    "legacy-task-1",
                    "conversation-legacy-task",
                    "Automatically inferred legacy objective",
                    "legacy-source"
                ],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO task_control_state_revisions (
                    task_id, revision, conversation_id, snapshot_json, mutation_kind,
                    mutation_id, result_task_id, created_at
                 ) VALUES (?1, 1, ?2, '{}', 'legacy', NULL, ?1, 1)",
                params!["legacy-task-1", "conversation-legacy-task"],
            )
            .unwrap();
    }

    let export = service
        .export_legacy_task_states("conversation-legacy-task")
        .unwrap();
    assert_eq!(export["controlStates"][0]["taskId"], "legacy-task-1");
    assert_eq!(export["revisions"].as_array().unwrap().len(), 1);
    assert!(service
        .load_conversation_goal("conversation-legacy-task")
        .unwrap()
        .is_none());
}
