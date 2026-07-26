// Rust core storage integration test for canonical model identity migration.

use mycopilot_core::storage::migrations::run_migrations;
use rusqlite::Connection;

#[test]
fn migrates_legacy_provider_path_into_the_canonical_model_id() {
    let connection = Connection::open_in_memory().unwrap();
    run_migrations(&connection).unwrap();
    connection
        .execute_batch(
            "
            ALTER TABLE models ADD COLUMN short_name TEXT;
            ALTER TABLE models ADD COLUMN provider_path TEXT;
            ALTER TABLE agent_usage_records ADD COLUMN provider_path TEXT;
            ALTER TABLE agent_deleted_usage_daily_rollups
                ADD COLUMN provider_path_key TEXT NOT NULL DEFAULT '';

            INSERT INTO models (
                id, display_name, supports_image, input_price, output_price, enabled,
                position, created_at, updated_at, short_name, provider_path
            ) VALUES (
                'model-a', 'model-a', 0, '0.01', '0.02', 1,
                0, 1, 1, 'A', 'provider/model-a'
            );

            INSERT INTO conversations (id, model_id, title, created_at, updated_at)
            VALUES ('conversation-1', 'model-a', 'Conversation', 1, 1);

            INSERT INTO messages (
                id, conversation_id, role, content, status, agent_run_json,
                ui_state_json, created_at, position
            ) VALUES (
                'message-1', 'conversation-1', 'assistant', 'done', 'sent',
                NULL, NULL, 1, 0
            );

            INSERT INTO composer_drafts (
                scope_id, message, permission_mode, permission_mode_version, model_id,
                project_id, attachments_json, skills_json, updated_at
            ) VALUES ('conversation-1', '', 'default', 1, 'model-a', NULL, '[]', '[]', 1);

            INSERT INTO agent_usage_records (
                id, conversation_id, message_id, run_id, model_id, model_name,
                provider_path, created_at, input_tokens
            ) VALUES (
                'usage-1', 'conversation-1', 'message-1', 'run-1',
                'model-a', 'model-a', 'provider/model-a', 1, 11
            );
            ",
        )
        .unwrap();

    run_migrations(&connection).unwrap();

    let model = connection
        .query_row("SELECT id, display_name FROM models", [], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .unwrap();
    assert_eq!(
        model,
        (
            "provider/model-a".to_string(),
            "provider/model-a".to_string()
        )
    );

    let references = connection
        .query_row(
            "SELECT conversation.model_id, draft.model_id
             FROM conversations AS conversation
             JOIN composer_drafts AS draft ON draft.scope_id = conversation.id
             WHERE conversation.id = 'conversation-1'",
            [],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
        )
        .unwrap();
    assert_eq!(
        references,
        (
            "provider/model-a".to_string(),
            "provider/model-a".to_string()
        )
    );

    let usage = connection
        .query_row(
            "SELECT model_id, input_tokens FROM agent_usage_records",
            [],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)),
        )
        .unwrap();
    assert_eq!(usage, ("provider/model-a".to_string(), 11));

    let model_columns = connection
        .prepare("PRAGMA table_info(models)")
        .unwrap()
        .query_map([], |row| row.get::<_, String>(1))
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();
    assert!(!model_columns.iter().any(|column| column == "short_name"));
    assert!(!model_columns.iter().any(|column| column == "provider_path"));

    let usage_columns = connection
        .prepare("PRAGMA table_info(agent_usage_records)")
        .unwrap()
        .query_map([], |row| row.get::<_, String>(1))
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();
    assert!(!usage_columns.iter().any(|column| column == "provider_path"));

    let rollup_columns = connection
        .prepare("PRAGMA table_info(agent_deleted_usage_daily_rollups)")
        .unwrap()
        .query_map([], |row| row.get::<_, String>(1))
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();
    assert!(!rollup_columns
        .iter()
        .any(|column| column == "provider_path_key"));

    run_migrations(&connection).unwrap();
    let model_count: i64 = connection
        .query_row("SELECT COUNT(*) FROM models", [], |row| row.get(0))
        .unwrap();
    assert_eq!(model_count, 1);
}
