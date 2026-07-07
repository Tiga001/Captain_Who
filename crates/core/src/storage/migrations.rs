// Rust core storage.
use rusqlite::Connection;

fn add_column_if_missing(
    connection: &Connection,
    table: &str,
    column: &str,
    definition: &str,
) -> rusqlite::Result<()> {
    connection
        .execute_batch(&format!(
            "ALTER TABLE {table} ADD COLUMN {column} {definition};"
        ))
        .or_else(|error| {
            if error.to_string().contains("duplicate column name") {
                Ok(())
            } else {
                Err(error)
            }
        })
}

pub fn run_migrations(connection: &Connection) -> rusqlite::Result<()> {
    connection.execute_batch(
        "
        PRAGMA foreign_keys = ON;

        CREATE TABLE IF NOT EXISTS model_provider_settings (
            id TEXT PRIMARY KEY CHECK (id = 'default'),
            api_url TEXT NOT NULL,
            api_token TEXT NOT NULL,
            search_mode TEXT NOT NULL,
            tavily_api_key TEXT NOT NULL,
            updated_at INTEGER NOT NULL
        );

        CREATE TABLE IF NOT EXISTS models (
            id TEXT PRIMARY KEY,
            display_name TEXT NOT NULL,
            short_name TEXT,
            provider_path TEXT,
            supports_image INTEGER NOT NULL,
            input_price TEXT NOT NULL,
            output_price TEXT NOT NULL,
            enabled INTEGER NOT NULL,
            position INTEGER NOT NULL,
            created_at INTEGER NOT NULL,
            updated_at INTEGER NOT NULL
        );

        CREATE TABLE IF NOT EXISTS projects (
            id TEXT PRIMARY KEY,
            name TEXT NOT NULL,
            path TEXT,
            created_at INTEGER NOT NULL,
            pinned_at INTEGER,
            updated_at INTEGER NOT NULL
        );

        CREATE TABLE IF NOT EXISTS conversations (
            id TEXT PRIMARY KEY,
            project_id TEXT,
            model_id TEXT,
            title TEXT NOT NULL,
            created_at INTEGER NOT NULL,
            updated_at INTEGER NOT NULL,
            pinned_at INTEGER,
            archived_at INTEGER,
            unread_at INTEGER
        );

        CREATE TABLE IF NOT EXISTS attachments (
            id TEXT PRIMARY KEY,
            conversation_id TEXT NOT NULL,
            message_id TEXT NOT NULL,
            project_id TEXT,
            kind TEXT NOT NULL,
            original_name TEXT NOT NULL,
            mime_type TEXT,
            size_bytes INTEGER NOT NULL,
            storage_rel_path TEXT NOT NULL,
            created_at INTEGER NOT NULL,
            FOREIGN KEY (conversation_id) REFERENCES conversations(id) ON DELETE CASCADE
        );

        CREATE TABLE IF NOT EXISTS composer_drafts (
            scope_id TEXT PRIMARY KEY,
            message TEXT NOT NULL,
            permission_mode TEXT NOT NULL,
            model_id TEXT,
            project_id TEXT,
            attachments_json TEXT NOT NULL,
            updated_at INTEGER NOT NULL
        );

        CREATE TABLE IF NOT EXISTS ui_preferences (
            id TEXT PRIMARY KEY CHECK (id = 'default'),
            sidebar_conversation_sort TEXT NOT NULL,
            sidebar_project_sort TEXT NOT NULL,
            sidebar_section_order TEXT NOT NULL,
            updated_at INTEGER NOT NULL
        );

        CREATE TABLE IF NOT EXISTS agent_prompt_preferences (
            id TEXT PRIMARY KEY CHECK (id = 'default'),
            work_mode TEXT NOT NULL,
            tone TEXT NOT NULL,
            detail_level TEXT NOT NULL,
            custom_instructions TEXT NOT NULL,
            updated_at INTEGER NOT NULL
        );

        CREATE TABLE IF NOT EXISTS agent_usage_records (
            id TEXT PRIMARY KEY,
            conversation_id TEXT NOT NULL,
            message_id TEXT NOT NULL,
            run_id TEXT NOT NULL,
            project_id TEXT,
            model_id TEXT NOT NULL,
            model_name TEXT NOT NULL,
            provider_path TEXT,
            started_at INTEGER,
            completed_at INTEGER,
            status TEXT,
            error TEXT,
            created_at INTEGER NOT NULL,
            input_tokens INTEGER,
            output_tokens INTEGER,
            total_tokens INTEGER,
            cached_input_tokens INTEGER,
            cache_creation_input_tokens INTEGER,
            billable_request_count INTEGER NOT NULL DEFAULT 1,
            input_price TEXT,
            output_price TEXT,
            estimated_cost REAL,
            UNIQUE(conversation_id, message_id),
            FOREIGN KEY (conversation_id) REFERENCES conversations(id) ON DELETE CASCADE
        );

        CREATE TABLE IF NOT EXISTS agent_action_audit (
            action_id TEXT PRIMARY KEY,
            run_id TEXT NOT NULL,
            conversation_id TEXT,
            assistant_message_id TEXT,
            action_type TEXT NOT NULL,
            tool_name TEXT NOT NULL,
            decision TEXT,
            status TEXT NOT NULL,
            action_json TEXT NOT NULL,
            patch_result_json TEXT,
            command_result_json TEXT,
            tool_result_json TEXT,
            error TEXT,
            created_at INTEGER NOT NULL,
            decided_at INTEGER,
            completed_at INTEGER
        );

        CREATE TABLE IF NOT EXISTS agent_pending_actions (
            action_id TEXT PRIMARY KEY,
            run_id TEXT NOT NULL,
            conversation_id TEXT,
            assistant_message_id TEXT,
            action_type TEXT NOT NULL,
            tool_name TEXT NOT NULL,
            tool_call_id TEXT,
            status TEXT NOT NULL,
            action_json TEXT NOT NULL,
            agent_input_json TEXT NOT NULL,
            created_at INTEGER NOT NULL,
            updated_at INTEGER NOT NULL
        );

        CREATE TABLE IF NOT EXISTS maintenance_tasks (
            id TEXT PRIMARY KEY,
            completed_at INTEGER NOT NULL
        );
        ",
    )?;

    add_column_if_missing(connection, "projects", "pinned_at", "INTEGER")?;
    add_column_if_missing(connection, "conversations", "pinned_at", "INTEGER")?;
    add_column_if_missing(connection, "conversations", "archived_at", "INTEGER")?;
    add_column_if_missing(connection, "conversations", "unread_at", "INTEGER")?;
    add_column_if_missing(
        connection,
        "ui_preferences",
        "sidebar_project_order_json",
        "TEXT NOT NULL DEFAULT '[]'",
    )?;
    add_column_if_missing(
        connection,
        "ui_preferences",
        "translucent_sidebar",
        "INTEGER NOT NULL DEFAULT 0",
    )?;
    add_column_if_missing(
        connection,
        "ui_preferences",
        "native_font_smoothing",
        "INTEGER NOT NULL DEFAULT 0",
    )?;
    add_column_if_missing(
        connection,
        "ui_preferences",
        "show_token_usage_details",
        "INTEGER NOT NULL DEFAULT 1",
    )?;
    add_column_if_missing(
        connection,
        "ui_preferences",
        "profile_display_name",
        "TEXT NOT NULL DEFAULT ''",
    )?;
    add_column_if_missing(
        connection,
        "ui_preferences",
        "profile_handle",
        "TEXT NOT NULL DEFAULT 'USER'",
    )?;
    add_column_if_missing(
        connection,
        "ui_preferences",
        "profile_avatar_data_url",
        "TEXT",
    )?;
    add_column_if_missing(
        connection,
        "ui_preferences",
        "custom_read_permission",
        "TEXT NOT NULL DEFAULT 'workspace_only'",
    )?;
    add_column_if_missing(
        connection,
        "ui_preferences",
        "custom_write_permission",
        "TEXT NOT NULL DEFAULT 'workspace_only'",
    )?;
    add_column_if_missing(
        connection,
        "ui_preferences",
        "custom_command_permission",
        "TEXT NOT NULL DEFAULT 'require_approval'",
    )?;
    add_column_if_missing(
        connection,
        "ui_preferences",
        "custom_patch_permission",
        "TEXT NOT NULL DEFAULT 'require_approval'",
    )?;
    add_column_if_missing(
        connection,
        "ui_preferences",
        "full_permission_enabled",
        "INTEGER NOT NULL DEFAULT 1",
    )?;
    add_column_if_missing(
        connection,
        "ui_preferences",
        "custom_permission_enabled",
        "INTEGER NOT NULL DEFAULT 1",
    )?;

    connection.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS messages (
            id TEXT PRIMARY KEY,
            conversation_id TEXT NOT NULL,
            role TEXT NOT NULL,
            content TEXT NOT NULL,
            status TEXT,
            agent_run_json TEXT,
            ui_state_json TEXT,
            created_at INTEGER NOT NULL,
            position INTEGER NOT NULL,
            FOREIGN KEY (conversation_id) REFERENCES conversations(id) ON DELETE CASCADE
        );

        CREATE INDEX IF NOT EXISTS idx_models_position ON models(position);
        CREATE INDEX IF NOT EXISTS idx_projects_updated_at ON projects(updated_at);
        CREATE INDEX IF NOT EXISTS idx_conversations_project_id ON conversations(project_id);
        CREATE INDEX IF NOT EXISTS idx_conversations_pinned_at ON conversations(pinned_at);
        CREATE INDEX IF NOT EXISTS idx_conversations_archived_at ON conversations(archived_at);
        CREATE INDEX IF NOT EXISTS idx_conversations_unread_at ON conversations(unread_at);
        CREATE INDEX IF NOT EXISTS idx_conversations_updated_at ON conversations(updated_at);
        CREATE INDEX IF NOT EXISTS idx_messages_conversation_id ON messages(conversation_id, position);
        CREATE INDEX IF NOT EXISTS idx_attachments_conversation_id ON attachments(conversation_id, created_at);
        CREATE INDEX IF NOT EXISTS idx_attachments_project_id ON attachments(project_id, created_at);
        CREATE INDEX IF NOT EXISTS idx_attachments_message_id ON attachments(message_id);
        CREATE INDEX IF NOT EXISTS idx_agent_usage_records_created_at ON agent_usage_records(created_at);
        CREATE INDEX IF NOT EXISTS idx_agent_usage_records_model_id ON agent_usage_records(model_id);
        CREATE INDEX IF NOT EXISTS idx_agent_usage_records_project_id ON agent_usage_records(project_id);
        CREATE INDEX IF NOT EXISTS idx_agent_action_audit_run_id ON agent_action_audit(run_id);
        CREATE INDEX IF NOT EXISTS idx_agent_action_audit_conversation_id ON agent_action_audit(conversation_id);
        CREATE INDEX IF NOT EXISTS idx_agent_action_audit_created_at ON agent_action_audit(created_at);
        CREATE INDEX IF NOT EXISTS idx_agent_action_audit_status ON agent_action_audit(status);
        CREATE INDEX IF NOT EXISTS idx_agent_pending_actions_status ON agent_pending_actions(status);
        CREATE INDEX IF NOT EXISTS idx_agent_pending_actions_run_id ON agent_pending_actions(run_id);
        CREATE INDEX IF NOT EXISTS idx_agent_pending_actions_conversation_id ON agent_pending_actions(conversation_id);
        CREATE INDEX IF NOT EXISTS idx_composer_drafts_updated_at ON composer_drafts(updated_at);
        ",
    )?;

    add_column_if_missing(connection, "messages", "agent_run_json", "TEXT")?;
    add_column_if_missing(connection, "messages", "ui_state_json", "TEXT")?;
    add_column_if_missing(connection, "agent_usage_records", "started_at", "INTEGER")?;
    add_column_if_missing(connection, "agent_usage_records", "completed_at", "INTEGER")?;
    add_column_if_missing(connection, "agent_usage_records", "status", "TEXT")?;
    add_column_if_missing(connection, "agent_usage_records", "error", "TEXT")?;
    add_column_if_missing(
        connection,
        "agent_action_audit",
        "effective_permissions_json",
        "TEXT",
    )?;
    add_column_if_missing(connection, "agent_action_audit", "path_scope", "TEXT")?;
    add_column_if_missing(
        connection,
        "agent_action_audit",
        "command_cwd_scope",
        "TEXT",
    )?;
    add_column_if_missing(connection, "agent_action_audit", "blocked_reason", "TEXT")?;
    add_column_if_missing(connection, "agent_action_audit", "decision_source", "TEXT")?;

    Ok(())
}
