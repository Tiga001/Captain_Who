use crate::context::CONTEXT_COMPACTION_SUMMARY_SCHEMA_VERSION;
use crate::CONVERSATION_TURN_TRACE_SCHEMA_VERSION;
use rusqlite::Connection;

const REMOVE_INITIAL_DEMO_PROFILE_TASK: &str = "remove_initial_demo_profile";
const CLEAR_PLACEHOLDER_TAVILY_KEY_TASK: &str = "clear_placeholder_tavily_key";
const CLEAR_INITIAL_API_URL_TASK: &str = "clear_initial_api_url";
const INITIAL_API_URL: &str = "https://zju.smartml.cn/userapi/v1/model/v1/chat/completions";

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

fn table_has_column(connection: &Connection, table: &str, column: &str) -> rusqlite::Result<bool> {
    let mut statement = connection.prepare(&format!("PRAGMA table_info({table})"))?;
    let columns = statement
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(columns.iter().any(|candidate| candidate == column))
}

fn upgrade_conversation_trace_commit_schema(connection: &Connection) -> rusqlite::Result<()> {
    if table_has_column(connection, "conversation_turn_traces", "updated_at")? {
        return Ok(());
    }

    connection.execute_batch("PRAGMA foreign_keys = OFF;")?;
    let migration = connection.execute_batch(
        "
        BEGIN IMMEDIATE;

        ALTER TABLE conversation_turn_trace_items
            RENAME TO conversation_turn_trace_items_legacy;
        ALTER TABLE conversation_turn_traces
            RENAME TO conversation_turn_traces_legacy;

        CREATE TABLE conversation_turn_traces (
            assistant_message_id TEXT PRIMARY KEY,
            conversation_id TEXT NOT NULL,
            run_id TEXT NOT NULL UNIQUE,
            schema_version INTEGER NOT NULL CHECK (schema_version > 0),
            terminal_status TEXT NOT NULL CHECK (terminal_status IN ('in_progress', 'completed', 'failed', 'cancelled')),
            terminal_error TEXT,
            truncated INTEGER NOT NULL CHECK (truncated IN (0, 1)),
            created_at INTEGER NOT NULL,
            updated_at INTEGER NOT NULL CHECK (updated_at >= created_at),
            completed_at INTEGER CHECK (completed_at IS NULL OR completed_at >= created_at),
            CHECK (
                (terminal_status = 'in_progress' AND terminal_error IS NULL AND completed_at IS NULL)
                OR (terminal_status != 'in_progress' AND completed_at IS NOT NULL)
            ),
            FOREIGN KEY (assistant_message_id) REFERENCES messages(id) ON DELETE CASCADE,
            FOREIGN KEY (conversation_id) REFERENCES conversations(id) ON DELETE CASCADE
        );

        CREATE TABLE conversation_turn_trace_items (
            assistant_message_id TEXT NOT NULL,
            sequence INTEGER NOT NULL CHECK (sequence >= 0),
            item_kind TEXT NOT NULL CHECK (item_kind IN ('assistant_narration', 'tool_call', 'tool_result')),
            item_json TEXT NOT NULL,
            PRIMARY KEY (assistant_message_id, sequence),
            FOREIGN KEY (assistant_message_id) REFERENCES conversation_turn_traces(assistant_message_id) ON DELETE CASCADE
        );

        INSERT INTO conversation_turn_traces (
            assistant_message_id, conversation_id, run_id, schema_version,
            terminal_status, terminal_error, truncated, created_at, updated_at, completed_at
        )
        SELECT
            assistant_message_id, conversation_id, run_id, schema_version,
            terminal_status, terminal_error, truncated, created_at, completed_at, completed_at
        FROM conversation_turn_traces_legacy;

        INSERT INTO conversation_turn_trace_items (
            assistant_message_id, sequence, item_kind, item_json
        )
        SELECT assistant_message_id, sequence, item_kind, item_json
        FROM conversation_turn_trace_items_legacy;

        DROP TABLE conversation_turn_trace_items_legacy;
        DROP TABLE conversation_turn_traces_legacy;
        COMMIT;
        ",
    );
    if let Err(error) = migration {
        let _ = connection.execute_batch("ROLLBACK;");
        let _ = connection.execute_batch("PRAGMA foreign_keys = ON;");
        return Err(error);
    }
    connection.execute_batch("PRAGMA foreign_keys = ON;")
}

fn reset_incompatible_context_compaction_schema(connection: &Connection) -> rusqlite::Result<()> {
    if !table_has_column(connection, "context_compaction_summaries", "id")? {
        return Ok(());
    }
    let current_shape = table_has_column(
        connection,
        "context_compaction_summaries",
        "covered_through_kind",
    )? && table_has_column(
        connection,
        "context_compaction_summaries",
        "continuity_schema_version",
    )? && table_has_column(
        connection,
        "context_compaction_summaries",
        "continuity_json",
    )? && table_has_column(
        connection,
        "context_compaction_summaries",
        "continuity_input_tokens",
    )? && table_has_column(
        connection,
        "context_compaction_summaries",
        "replacement_input_tokens",
    )?;
    if current_shape {
        return Ok(());
    }

    // Development summaries are derived data. Dropping the old message-prefix projection keeps
    // the raw conversation intact and avoids carrying two incompatible coverage models forward.
    connection.execute_batch(
        "
        DROP TRIGGER IF EXISTS invalidate_context_compaction_before_message_delete;
        DROP TRIGGER IF EXISTS invalidate_context_compaction_before_message_update;
        DROP TRIGGER IF EXISTS invalidate_context_compaction_before_trace_insert;
        DROP TRIGGER IF EXISTS invalidate_context_compaction_before_trace_item_update;
        DROP TRIGGER IF EXISTS invalidate_context_compaction_before_trace_item_delete;
        DROP TRIGGER IF EXISTS invalidate_context_compaction_before_trace_update;
        DROP TRIGGER IF EXISTS invalidate_context_compaction_before_trace_delete;
        DROP TABLE IF EXISTS conversation_context_compaction_heads;
        DROP TABLE IF EXISTS context_compaction_summary_sources;
        DROP TABLE IF EXISTS context_compaction_summaries;
        ",
    )
}

fn run_one_time_maintenance(connection: &Connection) -> rusqlite::Result<()> {
    let transaction = connection.unchecked_transaction()?;
    let already_completed = transaction.query_row(
        "SELECT EXISTS(SELECT 1 FROM maintenance_tasks WHERE id = ?1)",
        [REMOVE_INITIAL_DEMO_PROFILE_TASK],
        |row| row.get::<_, bool>(0),
    )?;

    if !already_completed {
        transaction.execute(
            "UPDATE ui_preferences
             SET profile_display_name = '', profile_handle = 'USER'
             WHERE lower(trim(profile_display_name)) = 'hx z'
               AND lower(ltrim(trim(profile_handle), '@')) = 'hxz9393'",
            [],
        )?;
        transaction.execute(
            "INSERT INTO maintenance_tasks (id, completed_at) VALUES (?1, CAST(strftime('%s', 'now') AS INTEGER) * 1000)",
            [REMOVE_INITIAL_DEMO_PROFILE_TASK],
        )?;
    }

    let placeholder_key_cleared = transaction.query_row(
        "SELECT EXISTS(SELECT 1 FROM maintenance_tasks WHERE id = ?1)",
        [CLEAR_PLACEHOLDER_TAVILY_KEY_TASK],
        |row| row.get::<_, bool>(0),
    )?;
    if !placeholder_key_cleared {
        transaction.execute(
            "UPDATE model_provider_settings SET tavily_api_key = '' WHERE trim(tavily_api_key) = 'tvly-my-copilot-search-key'",
            [],
        )?;
        transaction.execute(
            "INSERT INTO maintenance_tasks (id, completed_at) VALUES (?1, CAST(strftime('%s', 'now') AS INTEGER) * 1000)",
            [CLEAR_PLACEHOLDER_TAVILY_KEY_TASK],
        )?;
    }

    let initial_api_url_cleared = transaction.query_row(
        "SELECT EXISTS(SELECT 1 FROM maintenance_tasks WHERE id = ?1)",
        [CLEAR_INITIAL_API_URL_TASK],
        |row| row.get::<_, bool>(0),
    )?;
    if !initial_api_url_cleared {
        transaction.execute(
            "UPDATE model_provider_settings
             SET api_url = ''
             WHERE trim(api_url) = ?1 AND trim(api_token) = ''",
            [INITIAL_API_URL],
        )?;
        transaction.execute(
            "INSERT INTO maintenance_tasks (id, completed_at) VALUES (?1, CAST(strftime('%s', 'now') AS INTEGER) * 1000)",
            [CLEAR_INITIAL_API_URL_TASK],
        )?;
    }

    transaction.commit()
}

pub fn run_migrations(connection: &Connection) -> rusqlite::Result<()> {
    reset_incompatible_context_compaction_schema(connection)?;
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
            context_window_tokens INTEGER,
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
            output_thinking_tokens INTEGER,
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

        CREATE TABLE IF NOT EXISTS agent_deleted_usage_daily_rollups (
            usage_day INTEGER NOT NULL,
            model_id TEXT NOT NULL,
            model_name TEXT NOT NULL,
            provider_path_key TEXT NOT NULL,
            request_count INTEGER NOT NULL DEFAULT 0,
            message_count INTEGER NOT NULL DEFAULT 0,
            unpriced_message_count INTEGER NOT NULL DEFAULT 0,
            input_tokens INTEGER,
            output_tokens INTEGER,
            output_thinking_tokens INTEGER,
            total_tokens INTEGER,
            cached_input_tokens INTEGER,
            cache_creation_input_tokens INTEGER,
            estimated_cost REAL,
            created_at INTEGER NOT NULL,
            updated_at INTEGER NOT NULL,
            PRIMARY KEY (usage_day, model_id, model_name, provider_path_key)
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

        CREATE TABLE IF NOT EXISTS agent_file_drafts (
            id TEXT PRIMARY KEY,
            conversation_id TEXT NOT NULL,
            project_id TEXT,
            run_id TEXT NOT NULL,
            file_path TEXT NOT NULL,
            mode TEXT NOT NULL,
            status TEXT NOT NULL,
            base_revision TEXT,
            base_content TEXT NOT NULL,
            content TEXT NOT NULL,
            additions INTEGER NOT NULL DEFAULT 0,
            deletions INTEGER NOT NULL DEFAULT 0,
            line_count INTEGER NOT NULL DEFAULT 0,
            byte_count INTEGER NOT NULL DEFAULT 0,
            chunk_count INTEGER NOT NULL DEFAULT 0,
            next_chunk_index INTEGER NOT NULL DEFAULT 0,
            stats_final INTEGER NOT NULL DEFAULT 0,
            summary TEXT,
            final_action_id TEXT,
            created_at INTEGER NOT NULL,
            updated_at INTEGER NOT NULL,
            expires_at INTEGER NOT NULL,
            FOREIGN KEY (conversation_id) REFERENCES conversations(id) ON DELETE CASCADE
        );

        CREATE TABLE IF NOT EXISTS agent_file_draft_chunks (
            draft_id TEXT NOT NULL,
            chunk_index INTEGER NOT NULL,
            content_hash TEXT NOT NULL,
            byte_count INTEGER NOT NULL,
            created_at INTEGER NOT NULL,
            PRIMARY KEY (draft_id, chunk_index),
            FOREIGN KEY (draft_id) REFERENCES agent_file_drafts(id) ON DELETE CASCADE
        );

        CREATE TABLE IF NOT EXISTS agent_file_draft_operations (
            draft_id TEXT NOT NULL,
            sequence INTEGER NOT NULL,
            operation TEXT NOT NULL,
            payload_hash TEXT NOT NULL,
            created_at INTEGER NOT NULL,
            PRIMARY KEY (draft_id, sequence),
            FOREIGN KEY (draft_id) REFERENCES agent_file_drafts(id) ON DELETE CASCADE
        );

        CREATE TABLE IF NOT EXISTS maintenance_tasks (
            id TEXT PRIMARY KEY,
            completed_at INTEGER NOT NULL
        );
        ",
    )?;

    add_column_if_missing(connection, "projects", "pinned_at", "INTEGER")?;
    add_column_if_missing(connection, "models", "context_window_tokens", "INTEGER")?;
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
        "translucent_sidebar_transparency",
        "INTEGER NOT NULL DEFAULT 54",
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
        "show_context_window_usage",
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
    run_one_time_maintenance(connection)?;

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

        CREATE TABLE IF NOT EXISTS conversation_turn_traces (
            assistant_message_id TEXT PRIMARY KEY,
            conversation_id TEXT NOT NULL,
            run_id TEXT NOT NULL UNIQUE,
            schema_version INTEGER NOT NULL CHECK (schema_version > 0),
            terminal_status TEXT NOT NULL CHECK (terminal_status IN ('in_progress', 'completed', 'failed', 'cancelled')),
            terminal_error TEXT,
            truncated INTEGER NOT NULL CHECK (truncated IN (0, 1)),
            created_at INTEGER NOT NULL,
            updated_at INTEGER NOT NULL CHECK (updated_at >= created_at),
            completed_at INTEGER CHECK (completed_at IS NULL OR completed_at >= created_at),
            CHECK (
                (terminal_status = 'in_progress' AND terminal_error IS NULL AND completed_at IS NULL)
                OR (terminal_status != 'in_progress' AND completed_at IS NOT NULL)
            ),
            FOREIGN KEY (assistant_message_id) REFERENCES messages(id) ON DELETE CASCADE,
            FOREIGN KEY (conversation_id) REFERENCES conversations(id) ON DELETE CASCADE
        );

        CREATE TABLE IF NOT EXISTS conversation_turn_trace_items (
            assistant_message_id TEXT NOT NULL,
            sequence INTEGER NOT NULL CHECK (sequence >= 0),
            item_kind TEXT NOT NULL CHECK (item_kind IN ('assistant_narration', 'tool_call', 'tool_result')),
            item_json TEXT NOT NULL,
            PRIMARY KEY (assistant_message_id, sequence),
            FOREIGN KEY (assistant_message_id) REFERENCES conversation_turn_traces(assistant_message_id) ON DELETE CASCADE
        );

        CREATE TABLE IF NOT EXISTS context_compaction_summaries (
            id TEXT PRIMARY KEY,
            conversation_id TEXT NOT NULL,
            schema_version INTEGER NOT NULL CHECK (schema_version > 0),
            source_revision TEXT NOT NULL,
            previous_summary_id TEXT,
            covered_through_kind TEXT NOT NULL CHECK (covered_through_kind IN ('message', 'trace_item')),
            covered_through_message_id TEXT NOT NULL,
            covered_through_trace_sequence INTEGER CHECK (covered_through_trace_sequence >= 0),
            content TEXT NOT NULL CHECK (length(trim(content)) > 0),
            continuity_schema_version INTEGER NOT NULL CHECK (continuity_schema_version > 0),
            continuity_json TEXT NOT NULL CHECK (length(trim(continuity_json)) > 0),
            generation_kind TEXT NOT NULL CHECK (generation_kind IN ('test', 'model')),
            generation_model TEXT,
            source_input_tokens INTEGER NOT NULL CHECK (source_input_tokens > 0),
            summary_input_tokens INTEGER NOT NULL CHECK (
                summary_input_tokens > 0
            ),
            continuity_input_tokens INTEGER NOT NULL CHECK (continuity_input_tokens > 0),
            replacement_input_tokens INTEGER NOT NULL CHECK (
                replacement_input_tokens > 0
                AND replacement_input_tokens < source_input_tokens
                AND summary_input_tokens <= replacement_input_tokens
                AND continuity_input_tokens <= replacement_input_tokens
            ),
            created_at INTEGER NOT NULL CHECK (created_at >= 0),
            FOREIGN KEY (conversation_id) REFERENCES conversations(id) ON DELETE CASCADE,
            FOREIGN KEY (previous_summary_id) REFERENCES context_compaction_summaries(id) ON DELETE SET NULL,
            FOREIGN KEY (covered_through_message_id) REFERENCES messages(id) ON DELETE CASCADE,
            CHECK (
                (covered_through_kind = 'message' AND covered_through_trace_sequence IS NULL)
                OR (covered_through_kind = 'trace_item' AND covered_through_trace_sequence IS NOT NULL)
            ),
            CHECK (
                (generation_kind = 'test' AND generation_model IS NULL)
                OR (generation_kind = 'model' AND length(trim(generation_model)) > 0)
            )
        );

        CREATE TABLE IF NOT EXISTS model_request_observations (
            id TEXT PRIMARY KEY,
            schema_version INTEGER NOT NULL CHECK (schema_version > 0),
            run_id TEXT NOT NULL,
            conversation_id TEXT,
            assistant_message_id TEXT,
            operation_id TEXT,
            request_index INTEGER NOT NULL CHECK (request_index > 0),
            purpose TEXT NOT NULL CHECK (purpose IN ('agent_loop', 'context_compaction')),
            model TEXT NOT NULL CHECK (length(trim(model)) > 0),
            api_style TEXT NOT NULL CHECK (api_style IN ('open_ai_compatible', 'anthropic_compatible')),
            status TEXT NOT NULL CHECK (status IN ('completed', 'failed', 'cancelled')),
            estimated_input_tokens INTEGER,
            normalized_actual_input_tokens INTEGER,
            observation_json TEXT NOT NULL CHECK (length(trim(observation_json)) > 0),
            started_at INTEGER NOT NULL CHECK (started_at >= 0),
            completed_at INTEGER NOT NULL CHECK (completed_at >= started_at),
            CHECK (
                (conversation_id IS NULL AND assistant_message_id IS NULL)
                OR (conversation_id IS NOT NULL AND assistant_message_id IS NOT NULL)
            ),
            FOREIGN KEY (conversation_id) REFERENCES conversations(id) ON DELETE CASCADE,
            FOREIGN KEY (assistant_message_id) REFERENCES messages(id) ON DELETE CASCADE
        );

        CREATE TABLE IF NOT EXISTS context_compaction_receipts (
            operation_id TEXT PRIMARY KEY,
            schema_version INTEGER NOT NULL CHECK (schema_version > 0),
            run_id TEXT NOT NULL,
            conversation_id TEXT NOT NULL,
            assistant_message_id TEXT NOT NULL,
            request_index INTEGER NOT NULL CHECK (request_index > 0),
            attempt_index INTEGER NOT NULL CHECK (attempt_index > 0),
            model TEXT NOT NULL CHECK (length(trim(model)) > 0),
            api_style TEXT NOT NULL CHECK (api_style IN ('open_ai_compatible', 'anthropic_compatible')),
            status TEXT NOT NULL CHECK (status IN (
                'in_progress', 'applied', 'refreshed', 'failed', 'cancelled', 'interrupted'
            )),
            stage TEXT NOT NULL CHECK (stage IN (
                'planned', 'preparing', 'generating', 'committing', 'completed'
            )),
            generation_observation_id TEXT,
            summary_id TEXT UNIQUE,
            receipt_json TEXT NOT NULL CHECK (length(trim(receipt_json)) > 0),
            started_at INTEGER NOT NULL CHECK (started_at >= 0),
            updated_at INTEGER NOT NULL CHECK (updated_at >= started_at),
            completed_at INTEGER CHECK (completed_at IS NULL OR completed_at >= started_at),
            UNIQUE(run_id, request_index, attempt_index),
            CHECK (
                (status = 'in_progress' AND completed_at IS NULL)
                OR (status != 'in_progress' AND completed_at IS NOT NULL)
            ),
            FOREIGN KEY (conversation_id) REFERENCES conversations(id) ON DELETE CASCADE,
            FOREIGN KEY (assistant_message_id) REFERENCES messages(id) ON DELETE CASCADE,
            FOREIGN KEY (generation_observation_id) REFERENCES model_request_observations(id)
        );

        CREATE TABLE IF NOT EXISTS conversation_context_compaction_heads (
            conversation_id TEXT PRIMARY KEY,
            summary_id TEXT NOT NULL UNIQUE,
            revision INTEGER NOT NULL CHECK (revision > 0),
            updated_at INTEGER NOT NULL CHECK (updated_at >= 0),
            FOREIGN KEY (conversation_id) REFERENCES conversations(id) ON DELETE CASCADE,
            FOREIGN KEY (summary_id) REFERENCES context_compaction_summaries(id) ON DELETE CASCADE
        );

        CREATE INDEX IF NOT EXISTS idx_models_position ON models(position);
        CREATE INDEX IF NOT EXISTS idx_projects_updated_at ON projects(updated_at);
        CREATE INDEX IF NOT EXISTS idx_conversations_project_id ON conversations(project_id);
        CREATE INDEX IF NOT EXISTS idx_conversations_pinned_at ON conversations(pinned_at);
        CREATE INDEX IF NOT EXISTS idx_conversations_archived_at ON conversations(archived_at);
        CREATE INDEX IF NOT EXISTS idx_conversations_unread_at ON conversations(unread_at);
        CREATE INDEX IF NOT EXISTS idx_conversations_updated_at ON conversations(updated_at);
        CREATE INDEX IF NOT EXISTS idx_messages_conversation_id ON messages(conversation_id, position);
        CREATE INDEX IF NOT EXISTS idx_conversation_turn_traces_conversation_id ON conversation_turn_traces(conversation_id, completed_at);
        CREATE INDEX IF NOT EXISTS idx_context_compaction_summaries_conversation_id ON context_compaction_summaries(conversation_id, created_at);
        CREATE INDEX IF NOT EXISTS idx_model_request_observations_conversation_id ON model_request_observations(conversation_id, completed_at);
        CREATE INDEX IF NOT EXISTS idx_model_request_observations_operation_id ON model_request_observations(operation_id);
        CREATE INDEX IF NOT EXISTS idx_model_request_observations_profile ON model_request_observations(model, api_style, purpose, completed_at);
        CREATE INDEX IF NOT EXISTS idx_context_compaction_receipts_conversation_id ON context_compaction_receipts(conversation_id, started_at);
        CREATE INDEX IF NOT EXISTS idx_context_compaction_receipts_run_id ON context_compaction_receipts(run_id, request_index, attempt_index);
        CREATE INDEX IF NOT EXISTS idx_context_compaction_receipts_status ON context_compaction_receipts(status, updated_at);
        CREATE INDEX IF NOT EXISTS idx_attachments_conversation_id ON attachments(conversation_id, created_at);
        CREATE INDEX IF NOT EXISTS idx_attachments_project_id ON attachments(project_id, created_at);
        CREATE INDEX IF NOT EXISTS idx_attachments_message_id ON attachments(message_id);
        CREATE INDEX IF NOT EXISTS idx_agent_usage_records_created_at ON agent_usage_records(created_at);
        CREATE INDEX IF NOT EXISTS idx_agent_usage_records_model_id ON agent_usage_records(model_id);
        CREATE INDEX IF NOT EXISTS idx_agent_usage_records_project_id ON agent_usage_records(project_id);
        CREATE INDEX IF NOT EXISTS idx_agent_deleted_usage_daily_rollups_usage_day ON agent_deleted_usage_daily_rollups(usage_day);
        CREATE INDEX IF NOT EXISTS idx_agent_deleted_usage_daily_rollups_model_id ON agent_deleted_usage_daily_rollups(model_id);
        CREATE INDEX IF NOT EXISTS idx_agent_action_audit_run_id ON agent_action_audit(run_id);
        CREATE INDEX IF NOT EXISTS idx_agent_action_audit_conversation_id ON agent_action_audit(conversation_id);
        CREATE INDEX IF NOT EXISTS idx_agent_action_audit_created_at ON agent_action_audit(created_at);
        CREATE INDEX IF NOT EXISTS idx_agent_action_audit_status ON agent_action_audit(status);
        CREATE INDEX IF NOT EXISTS idx_agent_pending_actions_status ON agent_pending_actions(status);
        CREATE INDEX IF NOT EXISTS idx_agent_pending_actions_run_id ON agent_pending_actions(run_id);
        CREATE INDEX IF NOT EXISTS idx_agent_pending_actions_conversation_id ON agent_pending_actions(conversation_id);
        CREATE INDEX IF NOT EXISTS idx_agent_file_drafts_conversation_status ON agent_file_drafts(conversation_id, status);
        CREATE INDEX IF NOT EXISTS idx_agent_file_drafts_project_id ON agent_file_drafts(project_id);
        CREATE INDEX IF NOT EXISTS idx_agent_file_drafts_expires_at ON agent_file_drafts(expires_at);
        CREATE INDEX IF NOT EXISTS idx_composer_drafts_updated_at ON composer_drafts(updated_at);

        ",
    )?;

    connection.execute(
        "DELETE FROM context_compaction_summaries WHERE schema_version != ?1",
        [CONTEXT_COMPACTION_SUMMARY_SCHEMA_VERSION],
    )?;

    upgrade_conversation_trace_commit_schema(connection)?;
    connection.execute(
        "DELETE FROM conversation_turn_traces WHERE schema_version != ?1",
        [CONVERSATION_TURN_TRACE_SCHEMA_VERSION],
    )?;
    connection.execute_batch(
        "
        DROP INDEX IF EXISTS idx_conversation_turn_traces_conversation_id;
        CREATE INDEX idx_conversation_turn_traces_conversation_id
            ON conversation_turn_traces(conversation_id, updated_at);
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
        "agent_usage_records",
        "output_thinking_tokens",
        "INTEGER",
    )?;
    add_column_if_missing(
        connection,
        "agent_deleted_usage_daily_rollups",
        "unpriced_message_count",
        "INTEGER NOT NULL DEFAULT 0",
    )?;
    connection.execute(
        "
        UPDATE agent_deleted_usage_daily_rollups
        SET unpriced_message_count = message_count
        WHERE unpriced_message_count = 0
          AND estimated_cost IS NULL
          AND (input_tokens IS NOT NULL OR output_tokens IS NOT NULL)
        ",
        [],
    )?;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adds_nullable_context_window_to_existing_model_tables() {
        let connection = Connection::open_in_memory().unwrap();
        connection
            .execute_batch(
                "
                CREATE TABLE models (
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
                INSERT INTO models VALUES (
                    'model-a', 'Model A', NULL, NULL, 0, '0', '0', 1, 0, 1, 1
                );
                ",
            )
            .unwrap();

        run_migrations(&connection).unwrap();

        let context_window = connection
            .query_row(
                "SELECT context_window_tokens FROM models WHERE id = 'model-a'",
                [],
                |row| row.get::<_, Option<u32>>(0),
            )
            .unwrap();
        assert_eq!(context_window, None);
    }

    #[test]
    fn replaces_development_trace_schema_with_the_current_append_only_table() {
        let connection = Connection::open_in_memory().unwrap();
        connection
            .execute_batch(
                "
                PRAGMA foreign_keys = ON;
                CREATE TABLE conversations (
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
                CREATE TABLE messages (
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
                CREATE TABLE conversation_turn_traces (
                    assistant_message_id TEXT PRIMARY KEY,
                    conversation_id TEXT NOT NULL,
                    run_id TEXT NOT NULL UNIQUE,
                    schema_version INTEGER NOT NULL CHECK (schema_version > 0),
                    terminal_status TEXT NOT NULL CHECK (terminal_status IN ('completed', 'failed', 'cancelled')),
                    terminal_error TEXT,
                    truncated INTEGER NOT NULL CHECK (truncated IN (0, 1)),
                    created_at INTEGER NOT NULL,
                    completed_at INTEGER NOT NULL CHECK (completed_at >= created_at),
                    FOREIGN KEY (assistant_message_id) REFERENCES messages(id) ON DELETE CASCADE,
                    FOREIGN KEY (conversation_id) REFERENCES conversations(id) ON DELETE CASCADE
                );
                CREATE TABLE conversation_turn_trace_items (
                    assistant_message_id TEXT NOT NULL,
                    sequence INTEGER NOT NULL CHECK (sequence >= 0),
                    item_kind TEXT NOT NULL CHECK (item_kind IN ('assistant_narration', 'tool_call', 'tool_result')),
                    item_json TEXT NOT NULL,
                    PRIMARY KEY (assistant_message_id, sequence),
                    FOREIGN KEY (assistant_message_id) REFERENCES conversation_turn_traces(assistant_message_id) ON DELETE CASCADE
                );
                INSERT INTO conversations VALUES ('conversation-1', NULL, NULL, 'title', 1, 1, NULL, NULL, NULL);
                INSERT INTO messages VALUES ('assistant-1', 'conversation-1', 'assistant', 'done', 'sent', NULL, NULL, 1, 0);
                INSERT INTO conversation_turn_traces VALUES (
                    'assistant-1', 'conversation-1', 'run-1', 1, 'completed', NULL, 0, 1, 2
                );
                INSERT INTO conversation_turn_trace_items VALUES (
                    'assistant-1', 0, 'assistant_narration',
                    '{\"type\":\"assistant_narration\",\"sequence\":0,\"content\":\"hello\",\"truncated\":false}'
                );
                ",
            )
            .unwrap();

        run_migrations(&connection).unwrap();

        assert!(table_has_column(&connection, "conversation_turn_traces", "updated_at").unwrap());
        let old_trace_count = connection
            .query_row(
                "SELECT COUNT(*) FROM conversation_turn_trace_items WHERE assistant_message_id = 'assistant-1'",
                [],
                |row| row.get::<_, i64>(0),
            )
            .unwrap();
        assert_eq!(old_trace_count, 0);
        connection
            .execute(
                "INSERT INTO messages VALUES ('assistant-running', 'conversation-1', 'assistant', '', 'pending', NULL, NULL, 3, 1)",
                [],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO conversation_turn_traces (
                    assistant_message_id, conversation_id, run_id, schema_version,
                    terminal_status, terminal_error, truncated, created_at, updated_at, completed_at
                 ) VALUES ('assistant-running', 'conversation-1', 'run-running', ?1,
                    'in_progress', NULL, 0, 3, 3, NULL)",
                [CONVERSATION_TURN_TRACE_SCHEMA_VERSION],
            )
            .unwrap();
        let sql = connection
            .query_row(
                "SELECT sql FROM sqlite_master WHERE type = 'table' AND name = 'conversation_turn_traces'",
                [],
                |row| row.get::<_, String>(0),
            )
            .unwrap();
        assert!(sql.contains("in_progress"));
    }

    #[test]
    fn removes_initial_demo_profile_only_once() {
        let connection = Connection::open_in_memory().unwrap();
        run_migrations(&connection).unwrap();
        connection
            .execute(
                "DELETE FROM maintenance_tasks WHERE id = ?1",
                [REMOVE_INITIAL_DEMO_PROFILE_TASK],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO ui_preferences (
                    id,
                    sidebar_conversation_sort,
                    sidebar_project_sort,
                    sidebar_section_order,
                    profile_display_name,
                    profile_handle,
                    updated_at
                ) VALUES ('default', 'updated', 'created', 'projects_first', ' hx z ', '@hxz9393', 0)",
                [],
            )
            .unwrap();

        run_migrations(&connection).unwrap();
        let migrated = connection
            .query_row(
                "SELECT profile_display_name, profile_handle FROM ui_preferences WHERE id = 'default'",
                [],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
            )
            .unwrap();
        assert_eq!(migrated, (String::new(), "USER".to_string()));

        connection
            .execute(
                "UPDATE ui_preferences SET profile_display_name = 'hx z', profile_handle = 'hxz9393' WHERE id = 'default'",
                [],
            )
            .unwrap();
        run_migrations(&connection).unwrap();
        let preserved = connection
            .query_row(
                "SELECT profile_display_name, profile_handle FROM ui_preferences WHERE id = 'default'",
                [],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
            )
            .unwrap();
        assert_eq!(preserved, ("hx z".to_string(), "hxz9393".to_string()));
    }

    #[test]
    fn preserves_profiles_that_only_partially_match_the_initial_demo() {
        for (display_name, handle) in [("hx z", "real-user"), ("Real User", "hxz9393")] {
            let connection = Connection::open_in_memory().unwrap();
            run_migrations(&connection).unwrap();
            connection
                .execute(
                    "DELETE FROM maintenance_tasks WHERE id = ?1",
                    [REMOVE_INITIAL_DEMO_PROFILE_TASK],
                )
                .unwrap();
            connection
                .execute(
                    "INSERT INTO ui_preferences (
                        id,
                        sidebar_conversation_sort,
                        sidebar_project_sort,
                        sidebar_section_order,
                        profile_display_name,
                        profile_handle,
                        updated_at
                    ) VALUES ('default', 'updated', 'created', 'projects_first', ?1, ?2, 0)",
                    [display_name, handle],
                )
                .unwrap();

            run_migrations(&connection).unwrap();
            let preserved = connection
                .query_row(
                    "SELECT profile_display_name, profile_handle FROM ui_preferences WHERE id = 'default'",
                    [],
                    |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
                )
                .unwrap();
            assert_eq!(preserved, (display_name.to_string(), handle.to_string()));
        }
    }

    #[test]
    fn clears_placeholder_tavily_key() {
        let connection = Connection::open_in_memory().unwrap();
        run_migrations(&connection).unwrap();
        connection
            .execute(
                "DELETE FROM maintenance_tasks WHERE id = ?1",
                [CLEAR_PLACEHOLDER_TAVILY_KEY_TASK],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO model_provider_settings (
                    id, api_url, api_token, search_mode, tavily_api_key, updated_at
                ) VALUES ('default', '', '', 'auto', ' tvly-my-copilot-search-key ', 0)",
                [],
            )
            .unwrap();

        run_migrations(&connection).unwrap();
        let key = connection
            .query_row(
                "SELECT tavily_api_key FROM model_provider_settings WHERE id = 'default'",
                [],
                |row| row.get::<_, String>(0),
            )
            .unwrap();
        assert!(key.is_empty());
    }

    #[test]
    fn clears_initial_api_url_only_when_no_token_was_configured() {
        for (api_token, expected_url) in [("", ""), ("configured-token", INITIAL_API_URL)] {
            let connection = Connection::open_in_memory().unwrap();
            run_migrations(&connection).unwrap();
            connection
                .execute(
                    "DELETE FROM maintenance_tasks WHERE id = ?1",
                    [CLEAR_INITIAL_API_URL_TASK],
                )
                .unwrap();
            connection
                .execute(
                    "INSERT INTO model_provider_settings (
                        id, api_url, api_token, search_mode, tavily_api_key, updated_at
                    ) VALUES ('default', ?1, ?2, 'auto', '', 0)",
                    [INITIAL_API_URL, api_token],
                )
                .unwrap();

            run_migrations(&connection).unwrap();
            let api_url = connection
                .query_row(
                    "SELECT api_url FROM model_provider_settings WHERE id = 'default'",
                    [],
                    |row| row.get::<_, String>(0),
                )
                .unwrap();
            assert_eq!(api_url, expected_url);
        }
    }

    #[test]
    fn removes_only_incompatible_context_compaction_summaries() {
        let connection = Connection::open_in_memory().unwrap();
        run_migrations(&connection).unwrap();
        connection
            .execute_batch(
                "
                INSERT INTO conversations (
                    id, project_id, model_id, title, created_at, updated_at,
                    pinned_at, archived_at, unread_at
                ) VALUES ('conversation-1', NULL, NULL, 'title', 1, 1, NULL, NULL, NULL);
                INSERT INTO messages (
                    id, conversation_id, role, content, status, agent_run_json,
                    ui_state_json, created_at, position
                ) VALUES
                    ('user-1', 'conversation-1', 'user', 'question', 'sent', NULL, NULL, 1, 0),
                    ('assistant-1', 'conversation-1', 'assistant', 'answer', 'sent', NULL, NULL, 2, 1);
                ",
            )
            .unwrap();
        let incompatible_version = CONTEXT_COMPACTION_SUMMARY_SCHEMA_VERSION - 1;
        connection
            .execute(
                "INSERT INTO context_compaction_summaries (
                    id, conversation_id, schema_version, source_revision,
                    previous_summary_id, covered_through_kind,
                    covered_through_message_id, covered_through_trace_sequence, content,
                    continuity_schema_version, continuity_json,
                    generation_kind, generation_model, source_input_tokens,
                    summary_input_tokens, continuity_input_tokens,
                    replacement_input_tokens, created_at
                ) VALUES (
                    'old-summary', 'conversation-1', ?1, 'old-revision', NULL,
                    'message', 'assistant-1', NULL, 'old', 1, '{}',
                    'test', NULL, 10, 1, 1, 2, 3
                )",
                [incompatible_version],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO context_compaction_summaries (
                    id, conversation_id, schema_version, source_revision,
                    previous_summary_id, covered_through_kind,
                    covered_through_message_id, covered_through_trace_sequence, content,
                    continuity_schema_version, continuity_json,
                    generation_kind, generation_model, source_input_tokens,
                    summary_input_tokens, continuity_input_tokens,
                    replacement_input_tokens, created_at
                ) VALUES (
                    'current-summary', 'conversation-1', ?1, 'current-revision', NULL,
                    'message', 'assistant-1', NULL, 'current', 1, '{}',
                    'test', NULL, 10, 1, 1, 2, 4
                )",
                [CONTEXT_COMPACTION_SUMMARY_SCHEMA_VERSION],
            )
            .unwrap();
        connection
            .execute_batch(
                "
                INSERT INTO conversation_context_compaction_heads (
                    conversation_id, summary_id, revision, updated_at
                ) VALUES ('conversation-1', 'old-summary', 1, 3);
                ",
            )
            .unwrap();

        run_migrations(&connection).unwrap();

        let remaining = connection
            .query_row(
                "SELECT group_concat(id, ',') FROM context_compaction_summaries",
                [],
                |row| row.get::<_, Option<String>>(0),
            )
            .unwrap();
        assert_eq!(remaining.as_deref(), Some("current-summary"));
        let active_heads = connection
            .query_row(
                "SELECT COUNT(*) FROM conversation_context_compaction_heads",
                [],
                |row| row.get::<_, i64>(0),
            )
            .unwrap();
        assert_eq!(active_heads, 0);
    }

    #[test]
    fn replaces_legacy_compaction_tables_without_touching_raw_history() {
        let connection = Connection::open_in_memory().unwrap();
        run_migrations(&connection).unwrap();
        connection
            .execute_batch(
                "INSERT INTO conversations (
                    id, project_id, model_id, title, created_at, updated_at,
                    pinned_at, archived_at, unread_at
                 ) VALUES ('conversation-legacy', NULL, NULL, 'title', 1, 2, NULL, NULL, NULL);
                 INSERT INTO messages (
                    id, conversation_id, role, content, status, agent_run_json,
                    ui_state_json, created_at, position
                 ) VALUES
                    ('legacy-user', 'conversation-legacy', 'user', 'keep this question', 'sent', NULL, NULL, 1, 0),
                    ('legacy-assistant', 'conversation-legacy', 'assistant', 'keep this answer', 'sent', NULL, NULL, 2, 1);

                 DROP TRIGGER IF EXISTS invalidate_context_compaction_before_message_delete;
                 DROP TRIGGER IF EXISTS invalidate_context_compaction_before_message_update;
                 DROP TRIGGER IF EXISTS invalidate_context_compaction_before_trace_insert;
                 DROP TRIGGER IF EXISTS invalidate_context_compaction_before_trace_item_update;
                 DROP TRIGGER IF EXISTS invalidate_context_compaction_before_trace_item_delete;
                 DROP TRIGGER IF EXISTS invalidate_context_compaction_before_trace_update;
                 DROP TRIGGER IF EXISTS invalidate_context_compaction_before_trace_delete;
                 DROP TABLE conversation_context_compaction_heads;
                 DROP TABLE context_compaction_summaries;

                 CREATE TABLE context_compaction_summaries (
                    id TEXT PRIMARY KEY,
                    conversation_id TEXT NOT NULL,
                    schema_version INTEGER NOT NULL,
                    source_revision TEXT NOT NULL,
                    previous_summary_id TEXT,
                    covered_through_kind TEXT NOT NULL,
                    covered_through_message_id TEXT NOT NULL,
                    covered_through_trace_sequence INTEGER,
                    content TEXT NOT NULL,
                    generation_kind TEXT NOT NULL,
                    generation_model TEXT,
                    source_input_tokens INTEGER NOT NULL,
                    summary_input_tokens INTEGER NOT NULL,
                    created_at INTEGER NOT NULL
                 );
                 CREATE TABLE conversation_context_compaction_heads (
                    conversation_id TEXT PRIMARY KEY,
                    summary_id TEXT NOT NULL,
                    revision INTEGER NOT NULL,
                    updated_at INTEGER NOT NULL
                 );
                 INSERT INTO context_compaction_summaries VALUES (
                    'legacy-summary', 'conversation-legacy', 3, 'revision', NULL,
                    'message', 'legacy-assistant', NULL, 'legacy summary',
                    'test', NULL, 100, 10, 3
                 );
                 INSERT INTO conversation_context_compaction_heads VALUES (
                    'conversation-legacy', 'legacy-summary', 1, 3
                 );",
            )
            .unwrap();

        run_migrations(&connection).unwrap();

        let messages = connection
            .query_row(
                "SELECT group_concat(content, '|') FROM messages
                 WHERE conversation_id = 'conversation-legacy'
                 ORDER BY position",
                [],
                |row| row.get::<_, String>(0),
            )
            .unwrap();
        assert_eq!(messages, "keep this question|keep this answer");
        assert_eq!(
            connection
                .query_row(
                    "SELECT COUNT(*) FROM context_compaction_summaries",
                    [],
                    |row| { row.get::<_, i64>(0) }
                )
                .unwrap(),
            0
        );
        assert!(table_has_column(
            &connection,
            "context_compaction_summaries",
            "continuity_json"
        )
        .unwrap());
    }
}
