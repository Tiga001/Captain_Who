use crate::context::CONTEXT_COMPACTION_SUMMARY_SCHEMA_VERSION;
use crate::storage::{conversation_history_archive_repository, usage_repository};
use crate::CONVERSATION_TURN_TRACE_SCHEMA_VERSION;
use rusqlite::Connection;

const REMOVE_INITIAL_DEMO_PROFILE_TASK: &str = "remove_initial_demo_profile";
const CLEAR_PLACEHOLDER_TAVILY_KEY_TASK: &str = "clear_placeholder_tavily_key";
const CLEAR_INITIAL_API_URL_TASK: &str = "clear_initial_api_url";
const REMOVE_RETIRED_BUNDLED_SKILL_TASK: &str = "remove_retired_bundled_skill_v1";
const RETIRED_BUNDLED_SKILL_ID: &str = "bundled:application:repository-evidence-auditor";
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

fn ensure_legacy_task_state_schema(connection: &Connection) -> rusqlite::Result<()> {
    connection.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS task_control_states (
            task_id TEXT PRIMARY KEY,
            conversation_id TEXT NOT NULL,
            schema_version INTEGER NOT NULL CHECK (schema_version = 1),
            objective TEXT NOT NULL,
            source_message_id TEXT NOT NULL,
            status TEXT NOT NULL CHECK (
                status IN (
                    'active', 'waiting_user', 'blocked', 'completed',
                    'superseded', 'cancelled'
                )
            ),
            revision INTEGER NOT NULL CHECK (revision > 0),
            current_run_id TEXT,
            stopped_reason TEXT,
            checkpoint_json TEXT NOT NULL,
            created_at INTEGER NOT NULL,
            updated_at INTEGER NOT NULL,
            FOREIGN KEY (conversation_id) REFERENCES conversations(id) ON DELETE CASCADE
        );

        CREATE UNIQUE INDEX IF NOT EXISTS task_control_states_one_open_task
        ON task_control_states(conversation_id)
        WHERE status IN ('active', 'waiting_user', 'blocked');

        CREATE INDEX IF NOT EXISTS task_control_states_conversation_updated
        ON task_control_states(conversation_id, updated_at DESC);

        CREATE TRIGGER IF NOT EXISTS validate_task_control_state_source_insert
        BEFORE INSERT ON task_control_states
        WHEN NOT EXISTS (
            SELECT 1
            FROM messages
            WHERE id = NEW.source_message_id
              AND conversation_id = NEW.conversation_id
              AND role = 'user'
        )
        BEGIN
            SELECT RAISE(
                ABORT,
                'task state source must be a user message in the same conversation'
            );
        END;

        CREATE TRIGGER IF NOT EXISTS validate_task_control_state_source_update
        BEFORE UPDATE OF conversation_id, source_message_id
        ON task_control_states
        WHEN NOT EXISTS (
            SELECT 1
            FROM messages
            WHERE id = NEW.source_message_id
              AND conversation_id = NEW.conversation_id
              AND role = 'user'
        )
        BEGIN
            SELECT RAISE(
                ABORT,
                'task state source must be a user message in the same conversation'
            );
        END;

        CREATE TABLE IF NOT EXISTS task_control_state_revisions (
            task_id TEXT NOT NULL,
            revision INTEGER NOT NULL CHECK (revision > 0),
            conversation_id TEXT NOT NULL,
            snapshot_json TEXT NOT NULL,
            mutation_kind TEXT NOT NULL,
            mutation_id TEXT,
            result_task_id TEXT NOT NULL,
            created_at INTEGER NOT NULL,
            PRIMARY KEY (task_id, revision),
            FOREIGN KEY (task_id) REFERENCES task_control_states(task_id) ON DELETE CASCADE,
            FOREIGN KEY (conversation_id) REFERENCES conversations(id) ON DELETE CASCADE
        );

        CREATE UNIQUE INDEX IF NOT EXISTS task_control_state_revisions_mutation
        ON task_control_state_revisions(task_id, mutation_id)
        WHERE mutation_id IS NOT NULL;
        ",
    )
}

fn ensure_conversation_goal_schema(connection: &Connection) -> rusqlite::Result<()> {
    connection.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS conversation_goals (
            conversation_id TEXT PRIMARY KEY,
            goal_id TEXT NOT NULL UNIQUE,
            objective TEXT NOT NULL,
            source_message_id TEXT NOT NULL,
            status TEXT NOT NULL CHECK (
                status IN ('active', 'blocked', 'completed', 'cancelled')
            ),
            stopped_reason TEXT,
            created_at INTEGER NOT NULL,
            updated_at INTEGER NOT NULL CHECK (updated_at >= created_at),
            FOREIGN KEY (conversation_id) REFERENCES conversations(id) ON DELETE CASCADE,
            FOREIGN KEY (source_message_id) REFERENCES messages(id) ON DELETE CASCADE
        );

        CREATE TRIGGER IF NOT EXISTS validate_conversation_goal_source_insert
        BEFORE INSERT ON conversation_goals
        WHEN NOT EXISTS (
            SELECT 1
            FROM messages
            WHERE id = NEW.source_message_id
              AND conversation_id = NEW.conversation_id
              AND role = 'user'
        )
        BEGIN
            SELECT RAISE(
                ABORT,
                'goal source must be a user message in the same conversation'
            );
        END;

        CREATE TRIGGER IF NOT EXISTS validate_conversation_goal_source_update
        BEFORE UPDATE OF conversation_id, source_message_id
        ON conversation_goals
        WHEN NOT EXISTS (
            SELECT 1
            FROM messages
            WHERE id = NEW.source_message_id
              AND conversation_id = NEW.conversation_id
              AND role = 'user'
        )
        BEGIN
            SELECT RAISE(
                ABORT,
                'goal source must be a user message in the same conversation'
            );
        END;

        CREATE TABLE IF NOT EXISTS conversation_goal_revisions (
            conversation_id TEXT NOT NULL,
            goal_id TEXT NOT NULL,
            sequence INTEGER NOT NULL CHECK (sequence > 0),
            schema_version INTEGER NOT NULL CHECK (schema_version = 1),
            actor TEXT CHECK (actor IS NULL OR actor IN ('model', 'user')),
            event_kind TEXT NOT NULL CHECK (
                event_kind IN ('initial', 'objective_changed', 'status_changed')
            ),
            event_json TEXT NOT NULL CHECK (json_valid(event_json)),
            created_at INTEGER NOT NULL CHECK (created_at >= 0),
            PRIMARY KEY (goal_id, sequence),
            CHECK (
                (sequence = 1 AND event_kind = 'initial')
                OR (sequence > 1 AND event_kind != 'initial' AND actor IS NOT NULL)
            ),
            FOREIGN KEY (conversation_id) REFERENCES conversations(id) ON DELETE CASCADE
        );

        CREATE INDEX IF NOT EXISTS conversation_goal_revisions_conversation
        ON conversation_goal_revisions(conversation_id, created_at, goal_id, sequence);

        CREATE TRIGGER IF NOT EXISTS prevent_conversation_goal_revision_update
        BEFORE UPDATE ON conversation_goal_revisions
        BEGIN
            SELECT RAISE(ABORT, 'goal revisions are append-only');
        END;

        CREATE TRIGGER IF NOT EXISTS prevent_conversation_goal_revision_delete
        BEFORE DELETE ON conversation_goal_revisions
        WHEN EXISTS (
            SELECT 1 FROM conversations WHERE id = OLD.conversation_id
        )
        BEGIN
            SELECT RAISE(ABORT, 'goal revisions are append-only');
        END;

        INSERT OR IGNORE INTO conversation_goal_revisions (
            conversation_id, goal_id, sequence, schema_version, actor,
            event_kind, event_json, created_at
        )
        SELECT
            goal.conversation_id,
            goal.goal_id,
            1,
            1,
            NULL,
            'initial',
            json_object(
                'type', 'initial',
                'goal', json_object(
                    'goalId', goal.goal_id,
                    'conversationId', goal.conversation_id,
                    'objective', goal.objective,
                    'sourceMessageId', goal.source_message_id,
                    'status', goal.status,
                    'stoppedReason', goal.stopped_reason,
                    'createdAt', goal.created_at,
                    'updatedAt', goal.updated_at
                )
            ),
            goal.created_at
        FROM conversation_goals AS goal;
        ",
    )
}

fn ensure_conversation_history_fts_schema(connection: &Connection) -> rusqlite::Result<()> {
    connection.execute_batch(
        "
        CREATE VIRTUAL TABLE IF NOT EXISTS conversation_history_fts USING fts5(
            ref_key UNINDEXED,
            conversation_id UNINDEXED,
            record_type UNINDEXED,
            item_kind UNINDEXED,
            message_id UNINDEXED,
            assistant_message_id UNINDEXED,
            sequence UNINDEXED,
            archive_ref UNINDEXED,
            call_id UNINDEXED,
            tool UNINDEXED,
            status UNINDEXED,
            run_id UNINDEXED,
            created_at UNINDEXED,
            position UNINDEXED,
            within_message_order UNINDEXED,
            content,
            tokenize = 'trigram'
        );

        CREATE TRIGGER IF NOT EXISTS conversation_history_fts_message_insert
        AFTER INSERT ON messages
        BEGIN
            INSERT INTO conversation_history_fts (
                ref_key, conversation_id, record_type, item_kind, message_id,
                assistant_message_id, sequence, archive_ref, call_id, tool,
                status, run_id, created_at, position, within_message_order, content
            ) VALUES (
                'message:' || NEW.id, NEW.conversation_id, 'message', NULL, NEW.id,
                NULL, NULL, NULL, NULL, NULL, NEW.status, NULL, NEW.created_at,
                NEW.position, CASE WHEN NEW.role = 'assistant' THEN 9223372036854775807 ELSE 0 END,
                NEW.content
            );
        END;

        CREATE TRIGGER IF NOT EXISTS conversation_history_fts_message_update
        AFTER UPDATE OF conversation_id, role, content, status, created_at, position ON messages
        BEGIN
            DELETE FROM conversation_history_fts WHERE ref_key = 'message:' || OLD.id;
            INSERT INTO conversation_history_fts (
                ref_key, conversation_id, record_type, item_kind, message_id,
                assistant_message_id, sequence, archive_ref, call_id, tool,
                status, run_id, created_at, position, within_message_order, content
            ) VALUES (
                'message:' || NEW.id, NEW.conversation_id, 'message', NULL, NEW.id,
                NULL, NULL, NULL, NULL, NULL, NEW.status, NULL, NEW.created_at,
                NEW.position, CASE WHEN NEW.role = 'assistant' THEN 9223372036854775807 ELSE 0 END,
                NEW.content
            );
        END;

        CREATE TRIGGER IF NOT EXISTS conversation_history_fts_message_delete
        AFTER DELETE ON messages
        BEGIN
            DELETE FROM conversation_history_fts WHERE ref_key = 'message:' || OLD.id;
        END;

        CREATE TRIGGER IF NOT EXISTS conversation_history_fts_trace_insert
        AFTER INSERT ON conversation_turn_trace_items
        BEGIN
            INSERT INTO conversation_history_fts (
                ref_key, conversation_id, record_type, item_kind, message_id,
                assistant_message_id, sequence, archive_ref, call_id, tool,
                status, run_id, created_at, position, within_message_order, content
            )
            SELECT
                'trace:' || NEW.assistant_message_id || ':' || NEW.sequence,
                trace.conversation_id,
                'trace_item',
                NEW.item_kind,
                NULL,
                NEW.assistant_message_id,
                NEW.sequence,
                json_extract(NEW.item_json, '$.archiveRef'),
                json_extract(NEW.item_json, '$.callId'),
                json_extract(NEW.item_json, '$.tool'),
                COALESCE(
                    json_extract(NEW.item_json, '$.status'),
                    json_extract(NEW.item_json, '$.approvalStatus')
                ),
                trace.run_id,
                COALESCE(json_extract(NEW.item_json, '$.createdAt'), trace.created_at),
                message.position,
                NEW.sequence + 1,
                NEW.item_json
            FROM conversation_turn_traces AS trace
            INNER JOIN messages AS message
                ON message.id = trace.assistant_message_id
            WHERE trace.assistant_message_id = NEW.assistant_message_id;
            UPDATE conversation_history_fts
            SET
                status = COALESCE(
                    json_extract(NEW.item_json, '$.status'),
                    json_extract(NEW.item_json, '$.approvalStatus')
                ),
                run_id = (
                    SELECT run_id FROM conversation_turn_traces
                    WHERE assistant_message_id = NEW.assistant_message_id
                )
            WHERE archive_ref = json_extract(NEW.item_json, '$.archiveRef');
        END;

        CREATE TRIGGER IF NOT EXISTS conversation_history_fts_trace_update
        AFTER UPDATE OF item_kind, item_json ON conversation_turn_trace_items
        BEGIN
            DELETE FROM conversation_history_fts
            WHERE ref_key = 'trace:' || OLD.assistant_message_id || ':' || OLD.sequence;
            INSERT INTO conversation_history_fts (
                ref_key, conversation_id, record_type, item_kind, message_id,
                assistant_message_id, sequence, archive_ref, call_id, tool,
                status, run_id, created_at, position, within_message_order, content
            )
            SELECT
                'trace:' || NEW.assistant_message_id || ':' || NEW.sequence,
                trace.conversation_id,
                'trace_item',
                NEW.item_kind,
                NULL,
                NEW.assistant_message_id,
                NEW.sequence,
                json_extract(NEW.item_json, '$.archiveRef'),
                json_extract(NEW.item_json, '$.callId'),
                json_extract(NEW.item_json, '$.tool'),
                COALESCE(
                    json_extract(NEW.item_json, '$.status'),
                    json_extract(NEW.item_json, '$.approvalStatus')
                ),
                trace.run_id,
                COALESCE(json_extract(NEW.item_json, '$.createdAt'), trace.created_at),
                message.position,
                NEW.sequence + 1,
                NEW.item_json
            FROM conversation_turn_traces AS trace
            INNER JOIN messages AS message
                ON message.id = trace.assistant_message_id
            WHERE trace.assistant_message_id = NEW.assistant_message_id;
            UPDATE conversation_history_fts
            SET
                status = COALESCE(
                    json_extract(NEW.item_json, '$.status'),
                    json_extract(NEW.item_json, '$.approvalStatus')
                ),
                run_id = (
                    SELECT run_id FROM conversation_turn_traces
                    WHERE assistant_message_id = NEW.assistant_message_id
                )
            WHERE archive_ref = json_extract(NEW.item_json, '$.archiveRef');
        END;

        CREATE TRIGGER IF NOT EXISTS conversation_history_fts_trace_delete
        AFTER DELETE ON conversation_turn_trace_items
        BEGIN
            DELETE FROM conversation_history_fts
            WHERE ref_key = 'trace:' || OLD.assistant_message_id || ':' || OLD.sequence;
        END;

        CREATE TRIGGER IF NOT EXISTS conversation_history_fts_archive_delete
        AFTER DELETE ON conversation_history_blobs
        BEGIN
            DELETE FROM conversation_history_fts WHERE ref_key = 'archive:' || OLD.archive_ref;
        END;

        INSERT INTO conversation_history_fts (
            ref_key, conversation_id, record_type, item_kind, message_id,
            assistant_message_id, sequence, archive_ref, call_id, tool,
            status, run_id, created_at, position, within_message_order, content
        )
        SELECT
            'message:' || message.id,
            message.conversation_id,
            'message',
            NULL,
            message.id,
            NULL,
            NULL,
            NULL,
            NULL,
            NULL,
            message.status,
            NULL,
            message.created_at,
            message.position,
            CASE WHEN message.role = 'assistant' THEN 9223372036854775807 ELSE 0 END,
            message.content
        FROM messages AS message
        WHERE NOT EXISTS (
            SELECT 1 FROM conversation_history_fts
            WHERE ref_key = 'message:' || message.id
        );

        INSERT INTO conversation_history_fts (
            ref_key, conversation_id, record_type, item_kind, message_id,
            assistant_message_id, sequence, archive_ref, call_id, tool,
            status, run_id, created_at, position, within_message_order, content
        )
        SELECT
            'trace:' || item.assistant_message_id || ':' || item.sequence,
            trace.conversation_id,
            'trace_item',
            item.item_kind,
            NULL,
            item.assistant_message_id,
            item.sequence,
            json_extract(item.item_json, '$.archiveRef'),
            json_extract(item.item_json, '$.callId'),
            json_extract(item.item_json, '$.tool'),
            COALESCE(
                json_extract(item.item_json, '$.status'),
                json_extract(item.item_json, '$.approvalStatus')
            ),
            trace.run_id,
            COALESCE(json_extract(item.item_json, '$.createdAt'), trace.created_at),
            message.position,
            item.sequence + 1,
            item.item_json
        FROM conversation_turn_trace_items AS item
        INNER JOIN conversation_turn_traces AS trace
            ON trace.assistant_message_id = item.assistant_message_id
        INNER JOIN messages AS message
            ON message.id = trace.assistant_message_id
        WHERE NOT EXISTS (
            SELECT 1 FROM conversation_history_fts
            WHERE ref_key = 'trace:' || item.assistant_message_id || ':' || item.sequence
        );
        ",
    )?;
    conversation_history_archive_repository::backfill_history_search_index(connection)
}

fn usage_billable_default_is_zero(connection: &Connection) -> rusqlite::Result<bool> {
    let mut statement = connection.prepare("PRAGMA table_info(agent_usage_records)")?;
    let columns = statement
        .query_map([], |row| {
            Ok((row.get::<_, String>(1)?, row.get::<_, Option<String>>(4)?))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(columns.iter().any(|(name, default)| {
        name == "billable_request_count"
            && default
                .as_deref()
                .map(|value| value.trim_matches(['(', ')', '\'', '"']).trim() == "0")
                .unwrap_or(false)
    }))
}

fn usage_has_message_cascade(connection: &Connection) -> rusqlite::Result<bool> {
    let mut statement = connection.prepare("PRAGMA foreign_key_list(agent_usage_records)")?;
    let foreign_keys = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, String>(6)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(foreign_keys.iter().any(|(table, from, to, on_delete)| {
        table == "messages"
            && from == "message_id"
            && to == "id"
            && on_delete.eq_ignore_ascii_case("CASCADE")
    }))
}

fn upgrade_usage_consistency_schema(connection: &Connection) -> rusqlite::Result<()> {
    if !usage_billable_default_is_zero(connection)? || !usage_has_message_cascade(connection)? {
        let transaction = connection.unchecked_transaction()?;
        transaction.execute(
            "UPDATE agent_usage_records
             SET billable_request_count = 0
             WHERE billable_request_count < 0",
            [],
        )?;
        usage_repository::roll_up_orphaned_usage(&transaction)?;
        transaction.execute_batch(
            "
            CREATE TABLE agent_usage_records_consistent (
                id TEXT PRIMARY KEY,
                conversation_id TEXT NOT NULL,
                message_id TEXT NOT NULL,
                run_id TEXT NOT NULL,
                project_id TEXT,
                model_id TEXT NOT NULL,
                model_name TEXT NOT NULL,
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
                billable_request_count INTEGER NOT NULL DEFAULT 0
                    CHECK (billable_request_count >= 0),
                input_price TEXT,
                output_price TEXT,
                estimated_cost REAL,
                UNIQUE(conversation_id, message_id),
                FOREIGN KEY (conversation_id) REFERENCES conversations(id) ON DELETE CASCADE,
                FOREIGN KEY (message_id) REFERENCES messages(id) ON DELETE CASCADE
            );

            INSERT INTO agent_usage_records_consistent (
                id,
                conversation_id,
                message_id,
                run_id,
                project_id,
                model_id,
                model_name,
                started_at,
                completed_at,
                status,
                error,
                created_at,
                input_tokens,
                output_tokens,
                output_thinking_tokens,
                total_tokens,
                cached_input_tokens,
                cache_creation_input_tokens,
                billable_request_count,
                input_price,
                output_price,
                estimated_cost
            )
            SELECT
                usage.id,
                usage.conversation_id,
                usage.message_id,
                usage.run_id,
                usage.project_id,
                usage.model_id,
                usage.model_name,
                usage.started_at,
                usage.completed_at,
                usage.status,
                usage.error,
                usage.created_at,
                usage.input_tokens,
                usage.output_tokens,
                usage.output_thinking_tokens,
                usage.total_tokens,
                usage.cached_input_tokens,
                usage.cache_creation_input_tokens,
                MAX(usage.billable_request_count, 0),
                usage.input_price,
                usage.output_price,
                usage.estimated_cost
            FROM agent_usage_records AS usage
            JOIN messages AS message
              ON message.id = usage.message_id
             AND message.conversation_id = usage.conversation_id;

            DROP TABLE agent_usage_records;
            ALTER TABLE agent_usage_records_consistent RENAME TO agent_usage_records;

            CREATE INDEX idx_agent_usage_records_created_at
                ON agent_usage_records(created_at);
            CREATE INDEX idx_agent_usage_records_model_id
                ON agent_usage_records(model_id);
            CREATE INDEX idx_agent_usage_records_project_id
                ON agent_usage_records(project_id);
            ",
        )?;
        transaction.commit()?;
    }

    connection.execute_batch(
        "
        CREATE TRIGGER IF NOT EXISTS validate_agent_usage_message_insert
        BEFORE INSERT ON agent_usage_records
        WHEN NOT EXISTS (
            SELECT 1
            FROM messages
            WHERE id = NEW.message_id
              AND conversation_id = NEW.conversation_id
        )
        BEGIN
            SELECT RAISE(
                ABORT,
                'agent usage message must belong to the same conversation'
            );
        END;

        CREATE TRIGGER IF NOT EXISTS validate_agent_usage_message_update
        BEFORE UPDATE OF conversation_id, message_id ON agent_usage_records
        WHEN NOT EXISTS (
            SELECT 1
            FROM messages
            WHERE id = NEW.message_id
              AND conversation_id = NEW.conversation_id
        )
        BEGIN
            SELECT RAISE(
                ABORT,
                'agent usage message must belong to the same conversation'
            );
        END;
        ",
    )?;
    Ok(())
}

fn upgrade_canonical_model_identity_schema(connection: &Connection) -> rusqlite::Result<()> {
    let has_short_name = table_has_column(connection, "models", "short_name")?;
    let has_model_provider_path = table_has_column(connection, "models", "provider_path")?;
    let has_usage_provider_path =
        table_has_column(connection, "agent_usage_records", "provider_path")?;
    let has_rollup_provider_path = table_has_column(
        connection,
        "agent_deleted_usage_daily_rollups",
        "provider_path_key",
    )?;
    if !has_short_name
        && !has_model_provider_path
        && !has_usage_provider_path
        && !has_rollup_provider_path
    {
        return Ok(());
    }

    // Historical builds split one model identity across `id` (UI/storage) and
    // `provider_path` (the actual API `model` value). Collapse that split atomically so every
    // reference observes the same opaque model id and no usage totals are lost or duplicated.
    let transaction = connection.unchecked_transaction()?;
    if !has_model_provider_path {
        transaction.execute("ALTER TABLE models ADD COLUMN provider_path TEXT", [])?;
    }
    if !has_usage_provider_path {
        transaction.execute(
            "ALTER TABLE agent_usage_records ADD COLUMN provider_path TEXT",
            [],
        )?;
    }
    if !has_rollup_provider_path {
        transaction.execute(
            "ALTER TABLE agent_deleted_usage_daily_rollups
             ADD COLUMN provider_path_key TEXT NOT NULL DEFAULT ''",
            [],
        )?;
    }
    transaction.execute_batch(
        "
        CREATE TEMP TABLE canonical_model_identity_map (
            old_id TEXT PRIMARY KEY,
            canonical_id TEXT NOT NULL
        );

        INSERT INTO canonical_model_identity_map (old_id, canonical_id)
        SELECT
            id,
            CASE
                WHEN length(trim(COALESCE(provider_path, ''))) > 0 THEN trim(provider_path)
                ELSE id
            END
        FROM models;

        CREATE TABLE models_canonical_identity (
            id TEXT PRIMARY KEY,
            display_name TEXT NOT NULL,
            api_url_override TEXT,
            api_token_override TEXT,
            supports_image INTEGER NOT NULL,
            context_window_tokens INTEGER,
            input_price TEXT NOT NULL,
            output_price TEXT NOT NULL,
            enabled INTEGER NOT NULL,
            position INTEGER NOT NULL,
            created_at INTEGER NOT NULL,
            updated_at INTEGER NOT NULL
        );

        INSERT INTO models_canonical_identity (
            id,
            display_name,
            api_url_override,
            api_token_override,
            supports_image,
            context_window_tokens,
            input_price,
            output_price,
            enabled,
            position,
            created_at,
            updated_at
        )
        SELECT
            identity.canonical_id,
            CASE
                WHEN trim(model.display_name) = model.id THEN identity.canonical_id
                ELSE model.display_name
            END,
            model.api_url_override,
            model.api_token_override,
            model.supports_image,
            model.context_window_tokens,
            model.input_price,
            model.output_price,
            model.enabled,
            model.position,
            model.created_at,
            model.updated_at
        FROM models AS model
        JOIN canonical_model_identity_map AS identity ON identity.old_id = model.id
        WHERE model.rowid = (
            SELECT candidate.rowid
            FROM models AS candidate
            JOIN canonical_model_identity_map AS candidate_identity
                ON candidate_identity.old_id = candidate.id
            WHERE candidate_identity.canonical_id = identity.canonical_id
            ORDER BY
                CASE WHEN candidate.id = candidate_identity.canonical_id THEN 0 ELSE 1 END,
                candidate.position ASC,
                candidate.created_at ASC,
                candidate.id ASC
            LIMIT 1
        )
        ORDER BY model.position ASC, model.created_at ASC;

        UPDATE conversations
        SET model_id = (
            SELECT identity.canonical_id
            FROM canonical_model_identity_map AS identity
            WHERE identity.old_id = conversations.model_id
        )
        WHERE model_id IN (SELECT old_id FROM canonical_model_identity_map);

        UPDATE composer_drafts
        SET model_id = (
            SELECT identity.canonical_id
            FROM canonical_model_identity_map AS identity
            WHERE identity.old_id = composer_drafts.model_id
        )
        WHERE model_id IN (SELECT old_id FROM canonical_model_identity_map);

        CREATE TABLE agent_usage_records_canonical_identity (
            id TEXT PRIMARY KEY,
            conversation_id TEXT NOT NULL,
            message_id TEXT NOT NULL,
            run_id TEXT NOT NULL,
            project_id TEXT,
            model_id TEXT NOT NULL,
            model_name TEXT NOT NULL,
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
            billable_request_count INTEGER NOT NULL DEFAULT 0
                CHECK (billable_request_count >= 0),
            input_price TEXT,
            output_price TEXT,
            estimated_cost REAL,
            UNIQUE(conversation_id, message_id),
            FOREIGN KEY (conversation_id) REFERENCES conversations(id) ON DELETE CASCADE,
            FOREIGN KEY (message_id) REFERENCES messages(id) ON DELETE CASCADE
        );

        INSERT INTO agent_usage_records_canonical_identity (
            id,
            conversation_id,
            message_id,
            run_id,
            project_id,
            model_id,
            model_name,
            started_at,
            completed_at,
            status,
            error,
            created_at,
            input_tokens,
            output_tokens,
            output_thinking_tokens,
            total_tokens,
            cached_input_tokens,
            cache_creation_input_tokens,
            billable_request_count,
            input_price,
            output_price,
            estimated_cost
        )
        SELECT
            usage.id,
            usage.conversation_id,
            usage.message_id,
            usage.run_id,
            usage.project_id,
            CASE
                WHEN length(trim(COALESCE(usage.provider_path, ''))) > 0
                    THEN trim(usage.provider_path)
                ELSE COALESCE(identity.canonical_id, usage.model_id)
            END,
            CASE
                WHEN length(trim(COALESCE(usage.provider_path, ''))) > 0
                     AND trim(usage.model_name) = usage.model_id
                    THEN trim(usage.provider_path)
                ELSE usage.model_name
            END,
            usage.started_at,
            usage.completed_at,
            usage.status,
            usage.error,
            usage.created_at,
            usage.input_tokens,
            usage.output_tokens,
            usage.output_thinking_tokens,
            usage.total_tokens,
            usage.cached_input_tokens,
            usage.cache_creation_input_tokens,
            usage.billable_request_count,
            usage.input_price,
            usage.output_price,
            usage.estimated_cost
        FROM agent_usage_records AS usage
        LEFT JOIN canonical_model_identity_map AS identity ON identity.old_id = usage.model_id;

        ALTER TABLE agent_deleted_usage_daily_rollups
            RENAME TO agent_deleted_usage_daily_rollups_legacy_model_identity;

        CREATE TABLE agent_deleted_usage_daily_rollups (
            usage_day INTEGER NOT NULL,
            model_id TEXT NOT NULL,
            model_name TEXT NOT NULL,
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
            PRIMARY KEY (usage_day, model_id, model_name)
        );

        INSERT INTO agent_deleted_usage_daily_rollups (
            usage_day,
            model_id,
            model_name,
            request_count,
            message_count,
            unpriced_message_count,
            input_tokens,
            output_tokens,
            output_thinking_tokens,
            total_tokens,
            cached_input_tokens,
            cache_creation_input_tokens,
            estimated_cost,
            created_at,
            updated_at
        )
        WITH normalized AS (
            SELECT
                legacy.usage_day,
                CASE
                    WHEN length(trim(legacy.provider_path_key)) > 0
                        THEN trim(legacy.provider_path_key)
                    ELSE COALESCE(identity.canonical_id, legacy.model_id)
                END AS canonical_model_id,
                CASE
                    WHEN length(trim(legacy.provider_path_key)) > 0
                         AND trim(legacy.model_name) = legacy.model_id
                        THEN trim(legacy.provider_path_key)
                    ELSE legacy.model_name
                END AS canonical_model_name,
                legacy.request_count,
                legacy.message_count,
                legacy.unpriced_message_count,
                legacy.input_tokens,
                legacy.output_tokens,
                legacy.output_thinking_tokens,
                legacy.total_tokens,
                legacy.cached_input_tokens,
                legacy.cache_creation_input_tokens,
                legacy.estimated_cost,
                legacy.created_at,
                legacy.updated_at
            FROM agent_deleted_usage_daily_rollups_legacy_model_identity AS legacy
            LEFT JOIN canonical_model_identity_map AS identity ON identity.old_id = legacy.model_id
        )
        SELECT
            usage_day,
            canonical_model_id,
            canonical_model_name,
            SUM(request_count),
            SUM(message_count),
            SUM(unpriced_message_count),
            SUM(input_tokens),
            SUM(output_tokens),
            SUM(output_thinking_tokens),
            SUM(total_tokens),
            SUM(cached_input_tokens),
            SUM(cache_creation_input_tokens),
            SUM(estimated_cost),
            MIN(created_at),
            MAX(updated_at)
        FROM normalized
        GROUP BY usage_day, canonical_model_id, canonical_model_name;

        DROP TABLE agent_deleted_usage_daily_rollups_legacy_model_identity;
        DROP TABLE agent_usage_records;
        ALTER TABLE agent_usage_records_canonical_identity RENAME TO agent_usage_records;
        DROP TABLE models;
        ALTER TABLE models_canonical_identity RENAME TO models;

        CREATE INDEX idx_models_position ON models(position);
        CREATE INDEX idx_agent_usage_records_created_at ON agent_usage_records(created_at);
        CREATE INDEX idx_agent_usage_records_model_id ON agent_usage_records(model_id);
        CREATE INDEX idx_agent_usage_records_project_id ON agent_usage_records(project_id);
        CREATE INDEX idx_agent_deleted_usage_daily_rollups_usage_day
            ON agent_deleted_usage_daily_rollups(usage_day);
        CREATE INDEX idx_agent_deleted_usage_daily_rollups_model_id
            ON agent_deleted_usage_daily_rollups(model_id);

        DROP TABLE canonical_model_identity_map;
        ",
    )?;
    transaction.commit()
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
            item_kind TEXT NOT NULL CHECK (item_kind IN ('assistant_narration', 'user_guidance', 'tool_call', 'tool_result')),
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

fn upgrade_conversation_trace_v3_schema(connection: &Connection) -> rusqlite::Result<()> {
    let item_table_sql = connection.query_row(
        "SELECT sql FROM sqlite_master
         WHERE type = 'table' AND name = 'conversation_turn_trace_items'",
        [],
        |row| row.get::<_, String>(0),
    )?;
    if !item_table_sql.contains("'user_guidance'") {
        connection.execute_batch("PRAGMA foreign_keys = OFF;")?;
        let migration = connection.execute_batch(
            "
            BEGIN IMMEDIATE;
            ALTER TABLE conversation_turn_trace_items
                RENAME TO conversation_turn_trace_items_pre_guidance;
            CREATE TABLE conversation_turn_trace_items (
                assistant_message_id TEXT NOT NULL,
                sequence INTEGER NOT NULL CHECK (sequence >= 0),
                item_kind TEXT NOT NULL CHECK (item_kind IN (
                    'assistant_narration', 'user_guidance', 'tool_call', 'tool_result'
                )),
                item_json TEXT NOT NULL,
                PRIMARY KEY (assistant_message_id, sequence),
                FOREIGN KEY (assistant_message_id)
                    REFERENCES conversation_turn_traces(assistant_message_id) ON DELETE CASCADE
            );
            INSERT INTO conversation_turn_trace_items (
                assistant_message_id, sequence, item_kind, item_json
            )
            SELECT assistant_message_id, sequence, item_kind, item_json
            FROM conversation_turn_trace_items_pre_guidance;
            DROP TABLE conversation_turn_trace_items_pre_guidance;
            COMMIT;
            ",
        );
        if let Err(error) = migration {
            let _ = connection.execute_batch("ROLLBACK;");
            let _ = connection.execute_batch("PRAGMA foreign_keys = ON;");
            return Err(error);
        }
        connection.execute_batch("PRAGMA foreign_keys = ON;")?;
    }

    // Version 3 only adds a trace item kind. Existing v2 JSON remains canonical and must be
    // upgraded before the incompatible-version cleanup below runs.
    connection.execute(
        "UPDATE conversation_turn_traces
         SET schema_version = ?1
         WHERE schema_version = 2",
        [CONVERSATION_TURN_TRACE_SCHEMA_VERSION],
    )?;
    Ok(())
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

    let retired_skill_removed = transaction.query_row(
        "SELECT EXISTS(SELECT 1 FROM maintenance_tasks WHERE id = ?1)",
        [REMOVE_RETIRED_BUNDLED_SKILL_TASK],
        |row| row.get::<_, bool>(0),
    )?;
    if !retired_skill_removed {
        remove_retired_bundled_skill_from_drafts(&transaction)?;
        transaction.execute(
            "DELETE FROM skill_enablement_overrides WHERE skill_id = ?1",
            [RETIRED_BUNDLED_SKILL_ID],
        )?;
        transaction.execute(
            "INSERT INTO maintenance_tasks (id, completed_at) VALUES (?1, CAST(strftime('%s', 'now') AS INTEGER) * 1000)",
            [REMOVE_RETIRED_BUNDLED_SKILL_TASK],
        )?;
    }

    transaction.commit()
}

fn remove_retired_bundled_skill_from_drafts(
    transaction: &rusqlite::Transaction<'_>,
) -> rusqlite::Result<()> {
    let drafts = {
        let mut statement =
            transaction.prepare("SELECT scope_id, skills_json FROM composer_drafts")?;
        let rows = statement
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        rows
    };

    for (scope_id, skills_json) in drafts {
        let Ok(mut skills) = serde_json::from_str::<Vec<serde_json::Value>>(&skills_json) else {
            continue;
        };
        let previous_len = skills.len();
        skills.retain(|skill| {
            skill.get("id").and_then(serde_json::Value::as_str) != Some(RETIRED_BUNDLED_SKILL_ID)
        });
        if skills.len() == previous_len {
            continue;
        }
        let updated = serde_json::to_string(&skills)
            .map_err(|error| rusqlite::Error::ToSqlConversionFailure(Box::new(error)))?;
        transaction.execute(
            "UPDATE composer_drafts SET skills_json = ?1 WHERE scope_id = ?2",
            rusqlite::params![updated, scope_id],
        )?;
    }

    Ok(())
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

        CREATE TABLE IF NOT EXISTS image_generation_profiles (
            id TEXT PRIMARY KEY CHECK (
                typeof(id) = 'text'
                AND length(CAST(id AS BLOB)) BETWEEN 1 AND 128
            ),
            schema_version INTEGER NOT NULL CHECK (schema_version = 1),
            adapter_id TEXT NOT NULL CHECK (adapter_id = 'smartmlSeedream'),
            endpoint_url TEXT NOT NULL CHECK (
                length(CAST(endpoint_url AS BLOB)) <= 4096
            ),
            model_id TEXT NOT NULL CHECK (
                length(CAST(model_id AS BLOB)) <= 512
            ),
            credential_ref TEXT CHECK (
                credential_ref IS NULL
                OR (
                    typeof(credential_ref) = 'text'
                    AND length(CAST(credential_ref AS BLOB)) BETWEEN 1 AND 1024
                )
            ),
            enabled INTEGER NOT NULL CHECK (enabled IN (0, 1)),
            text_to_image INTEGER NOT NULL DEFAULT 1 CHECK (text_to_image = 1),
            image_to_image INTEGER NOT NULL DEFAULT 0 CHECK (image_to_image IN (0, 1)),
            default_size_preset TEXT NOT NULL DEFAULT '2K' CHECK (
                default_size_preset = '2K'
            ),
            default_watermark INTEGER NOT NULL DEFAULT 1 CHECK (
                default_watermark IN (0, 1)
            ),
            generation INTEGER NOT NULL DEFAULT 0 CHECK (generation >= 0),
            created_at INTEGER NOT NULL CHECK (created_at >= 0),
            updated_at INTEGER NOT NULL CHECK (updated_at >= created_at)
        );

        CREATE TABLE IF NOT EXISTS image_generation_credential_staging (
            credential_ref TEXT PRIMARY KEY CHECK (
                typeof(credential_ref) = 'text'
                AND length(CAST(credential_ref AS BLOB)) BETWEEN 1 AND 1024
            ),
            profile_id TEXT NOT NULL CHECK (
                typeof(profile_id) = 'text'
                AND length(CAST(profile_id AS BLOB)) BETWEEN 1 AND 128
            ),
            expected_generation INTEGER NOT NULL CHECK (expected_generation >= 0),
            created_at INTEGER NOT NULL CHECK (created_at >= 0)
        );

        CREATE TABLE IF NOT EXISTS image_generation_credential_cleanup (
            credential_ref TEXT PRIMARY KEY CHECK (
                typeof(credential_ref) = 'text'
                AND length(CAST(credential_ref AS BLOB)) BETWEEN 1 AND 1024
            ),
            created_at INTEGER NOT NULL CHECK (created_at >= 0)
        );

        CREATE TABLE IF NOT EXISTS image_generation_executions (
            execution_id TEXT PRIMARY KEY CHECK (
                typeof(execution_id) = 'text'
                AND length(CAST(execution_id AS BLOB)) BETWEEN 1 AND 256
            ),
            schema_version INTEGER NOT NULL CHECK (schema_version = 1),
            request_fingerprint TEXT NOT NULL CHECK (
                length(request_fingerprint) = 71
                AND substr(request_fingerprint, 1, 7) = 'sha256:'
                AND substr(request_fingerprint, 8) NOT GLOB '*[^0-9a-f]*'
            ),
            safe_request_json TEXT NOT NULL CHECK (
                length(CAST(safe_request_json AS BLOB)) BETWEEN 2 AND 65536
            ),
            profile_id TEXT NOT NULL CHECK (
                length(CAST(profile_id AS BLOB)) BETWEEN 1 AND 256
            ),
            adapter_id TEXT NOT NULL CHECK (
                length(CAST(adapter_id AS BLOB)) BETWEEN 1 AND 128
            ),
            profile_revision INTEGER NOT NULL CHECK (profile_revision > 0),
            model_id TEXT NOT NULL CHECK (
                length(CAST(model_id AS BLOB)) BETWEEN 1 AND 512
            ),
            operation TEXT NOT NULL CHECK (operation IN ('generate', 'edit')),
            status TEXT NOT NULL CHECK (status IN (
                'executing',
                'publishing',
                'succeeded',
                'failed',
                'cancelled',
                'outcome_indeterminate',
                'commit_indeterminate'
            )),
            remote_outcome_unknown INTEGER NOT NULL DEFAULT 0 CHECK (
                remote_outcome_unknown IN (0, 1)
            ),
            provider_succeeded INTEGER NOT NULL DEFAULT 0 CHECK (
                provider_succeeded IN (0, 1)
            ),
            commit_may_have_succeeded INTEGER NOT NULL DEFAULT 0 CHECK (
                commit_may_have_succeeded IN (0, 1)
            ),
            provider_request_id TEXT CHECK (
                provider_request_id IS NULL
                OR length(CAST(provider_request_id AS BLOB)) BETWEEN 1 AND 128
            ),
            http_status INTEGER CHECK (http_status IS NULL OR http_status BETWEEN 100 AND 599),
            terminal_result_json TEXT CHECK (
                terminal_result_json IS NULL
                OR length(CAST(terminal_result_json AS BLOB)) BETWEEN 2 AND 65536
            ),
            created_at INTEGER NOT NULL CHECK (created_at >= 0),
            updated_at INTEGER NOT NULL CHECK (updated_at >= created_at),
            completed_at INTEGER CHECK (
                (status IN ('executing', 'publishing') AND completed_at IS NULL AND terminal_result_json IS NULL)
                OR
                (status NOT IN ('executing', 'publishing') AND completed_at IS NOT NULL AND terminal_result_json IS NOT NULL)
            )
        );

        CREATE INDEX IF NOT EXISTS image_generation_executions_status_idx
            ON image_generation_executions (status, updated_at, execution_id);

        CREATE TABLE IF NOT EXISTS image_generation_artifacts (
            execution_id TEXT NOT NULL,
            ordinal INTEGER NOT NULL CHECK (ordinal >= 0),
            schema_version INTEGER NOT NULL CHECK (schema_version = 1),
            artifact_id TEXT NOT NULL CHECK (
                length(artifact_id) = 71
                AND substr(artifact_id, 1, 7) = 'sha256:'
                AND substr(artifact_id, 8) NOT GLOB '*[^0-9a-f]*'
            ),
            state TEXT NOT NULL CHECK (state IN (
                'candidate', 'published', 'discarded', 'indeterminate'
            )),
            storage_relative_path TEXT NOT NULL CHECK (
                length(CAST(storage_relative_path AS BLOB)) BETWEEN 1 AND 1024
            ),
            format TEXT NOT NULL CHECK (format IN ('png', 'jpeg', 'webp')),
            media_type TEXT NOT NULL CHECK (media_type IN ('image/png', 'image/jpeg', 'image/webp')),
            width INTEGER NOT NULL CHECK (width BETWEEN 1 AND 16384),
            height INTEGER NOT NULL CHECK (height BETWEEN 1 AND 16384),
            size_bytes INTEGER NOT NULL CHECK (size_bytes > 0),
            sha256 TEXT NOT NULL CHECK (
                length(sha256) = 64
                AND sha256 NOT GLOB '*[^0-9a-f]*'
            ),
            created_at INTEGER NOT NULL CHECK (created_at >= 0),
            published_at INTEGER,
            PRIMARY KEY (execution_id, ordinal),
            FOREIGN KEY (execution_id) REFERENCES image_generation_executions(execution_id)
                ON DELETE CASCADE,
            CHECK (
                (state = 'published' AND published_at IS NOT NULL)
                OR (state != 'published' AND published_at IS NULL)
            )
        );

        CREATE INDEX IF NOT EXISTS image_generation_artifacts_identity_idx
            ON image_generation_artifacts (artifact_id, state);

        CREATE TABLE IF NOT EXISTS models (
            id TEXT PRIMARY KEY,
            display_name TEXT NOT NULL,
            api_url_override TEXT,
            api_token_override TEXT,
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
            permission_mode_version INTEGER NOT NULL DEFAULT 0 CHECK (permission_mode_version >= 0),
            model_id TEXT,
            project_id TEXT,
            attachments_json TEXT NOT NULL,
            skills_json TEXT NOT NULL DEFAULT '[]',
            queued_messages_json TEXT NOT NULL DEFAULT '[]',
            updated_at INTEGER NOT NULL
        );

        CREATE TABLE IF NOT EXISTS skill_enablement_overrides (
            skill_id TEXT PRIMARY KEY
                CHECK (
                    typeof(skill_id) = 'text'
                    AND length(CAST(skill_id AS BLOB)) BETWEEN 1 AND 16384
                ),
            enabled INTEGER NOT NULL CHECK (enabled IN (0, 1)),
            generation INTEGER NOT NULL DEFAULT 0 CHECK (generation >= 0),
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
            billable_request_count INTEGER NOT NULL DEFAULT 0
                CHECK (billable_request_count >= 0),
            input_price TEXT,
            output_price TEXT,
            estimated_cost REAL,
            UNIQUE(conversation_id, message_id),
            FOREIGN KEY (conversation_id) REFERENCES conversations(id) ON DELETE CASCADE,
            FOREIGN KEY (message_id) REFERENCES messages(id) ON DELETE CASCADE
        );

        CREATE TABLE IF NOT EXISTS agent_deleted_usage_daily_rollups (
            usage_day INTEGER NOT NULL,
            model_id TEXT NOT NULL,
            model_name TEXT NOT NULL,
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
            PRIMARY KEY (usage_day, model_id, model_name)
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
            target_status TEXT,
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

    add_column_if_missing(
        connection,
        "skill_enablement_overrides",
        "generation",
        "INTEGER NOT NULL DEFAULT 0 CHECK (generation >= 0)",
    )?;
    add_column_if_missing(connection, "projects", "pinned_at", "INTEGER")?;
    add_column_if_missing(connection, "models", "context_window_tokens", "INTEGER")?;
    add_column_if_missing(connection, "models", "api_url_override", "TEXT")?;
    add_column_if_missing(connection, "models", "api_token_override", "TEXT")?;
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
    add_column_if_missing(
        connection,
        "composer_drafts",
        "skills_json",
        "TEXT NOT NULL DEFAULT '[]'",
    )?;
    add_column_if_missing(
        connection,
        "composer_drafts",
        "queued_messages_json",
        "TEXT NOT NULL DEFAULT '[]'",
    )?;
    add_column_if_missing(
        connection,
        "composer_drafts",
        "permission_mode_version",
        "INTEGER NOT NULL DEFAULT 0 CHECK (permission_mode_version >= 0)",
    )?;
    // The meaning of `full` was broadened. A legacy, unversioned selection is not evidence that
    // the user opted into the new semantics, so migrate it to the safe default exactly as the
    // storage service does for legacy clients writing after this migration.
    connection.execute(
        "UPDATE composer_drafts
         SET permission_mode = 'default'
         WHERE permission_mode = 'full' AND permission_mode_version = 0",
        [],
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
            item_kind TEXT NOT NULL CHECK (item_kind IN ('assistant_narration', 'user_guidance', 'tool_call', 'tool_result')),
            item_json TEXT NOT NULL,
            PRIMARY KEY (assistant_message_id, sequence),
            FOREIGN KEY (assistant_message_id) REFERENCES conversation_turn_traces(assistant_message_id) ON DELETE CASCADE
        );

        CREATE TABLE IF NOT EXISTS conversation_history_blobs (
            archive_ref TEXT PRIMARY KEY CHECK (
                typeof(archive_ref) = 'text'
                AND length(CAST(archive_ref AS BLOB)) BETWEEN 1 AND 256
            ),
            conversation_id TEXT NOT NULL,
            assistant_message_id TEXT NOT NULL,
            sequence INTEGER NOT NULL CHECK (sequence >= 0),
            call_id TEXT NOT NULL CHECK (
                length(CAST(call_id AS BLOB)) BETWEEN 1 AND 1024
            ),
            tool TEXT NOT NULL CHECK (
                length(CAST(tool AS BLOB)) BETWEEN 1 AND 256
            ),
            content_type TEXT NOT NULL CHECK (
                length(CAST(content_type AS BLOB)) BETWEEN 1 AND 256
            ),
            content_hash TEXT NOT NULL CHECK (
                length(content_hash) = 71
                AND substr(content_hash, 1, 7) = 'sha256:'
                AND substr(content_hash, 8) NOT GLOB '*[^0-9a-f]*'
            ),
            uncompressed_bytes INTEGER NOT NULL CHECK (uncompressed_bytes >= 0),
            uncompressed_chars INTEGER NOT NULL CHECK (uncompressed_chars >= 0),
            chunk_count INTEGER NOT NULL CHECK (chunk_count > 0),
            compression TEXT NOT NULL CHECK (compression = 'zstd'),
            truncated_at_source INTEGER NOT NULL CHECK (truncated_at_source IN (0, 1)),
            archived_completely INTEGER NOT NULL CHECK (archived_completely IN (0, 1)),
            model_projection_truncated INTEGER NOT NULL
                CHECK (model_projection_truncated IN (0, 1)),
            archive_projection_truncated INTEGER NOT NULL
                CHECK (archive_projection_truncated IN (0, 1)),
            created_at INTEGER NOT NULL CHECK (created_at >= 0),
            UNIQUE (conversation_id, assistant_message_id, sequence),
            UNIQUE (conversation_id, call_id),
            FOREIGN KEY (conversation_id) REFERENCES conversations(id) ON DELETE CASCADE,
            FOREIGN KEY (assistant_message_id) REFERENCES messages(id) ON DELETE CASCADE
        );

        CREATE TABLE IF NOT EXISTS conversation_history_blob_chunks (
            archive_ref TEXT NOT NULL,
            chunk_index INTEGER NOT NULL CHECK (chunk_index >= 0),
            uncompressed_start_byte INTEGER NOT NULL
                CHECK (uncompressed_start_byte >= 0),
            uncompressed_start_char INTEGER NOT NULL
                CHECK (uncompressed_start_char >= 0),
            uncompressed_bytes INTEGER NOT NULL CHECK (uncompressed_bytes >= 0),
            uncompressed_chars INTEGER NOT NULL CHECK (uncompressed_chars >= 0),
            compressed_bytes INTEGER NOT NULL CHECK (compressed_bytes > 0),
            payload BLOB NOT NULL CHECK (length(payload) = compressed_bytes),
            PRIMARY KEY (archive_ref, chunk_index),
            FOREIGN KEY (archive_ref)
                REFERENCES conversation_history_blobs(archive_ref) ON DELETE CASCADE
        );

        CREATE INDEX IF NOT EXISTS conversation_history_blobs_trace_item_idx
            ON conversation_history_blobs (
                conversation_id, assistant_message_id, sequence
            );

        CREATE TRIGGER IF NOT EXISTS validate_conversation_history_blob_message_insert
        BEFORE INSERT ON conversation_history_blobs
        WHEN NOT EXISTS (
            SELECT 1
            FROM messages
            WHERE id = NEW.assistant_message_id
              AND conversation_id = NEW.conversation_id
              AND role = 'assistant'
        )
        BEGIN
            SELECT RAISE(
                ABORT,
                'history archive message must be an assistant message in the same conversation'
            );
        END;

        CREATE TABLE IF NOT EXISTS conversation_world_state_epochs (
            conversation_id TEXT NOT NULL,
            epoch_id TEXT NOT NULL CHECK (
                length(CAST(epoch_id AS BLOB)) BETWEEN 1 AND 256
            ),
            generation INTEGER NOT NULL CHECK (generation > 0),
            base_summary_id TEXT,
            created_at INTEGER NOT NULL CHECK (created_at >= 0),
            PRIMARY KEY (conversation_id, epoch_id),
            UNIQUE (conversation_id, generation),
            UNIQUE (base_summary_id),
            FOREIGN KEY (conversation_id) REFERENCES conversations(id) ON DELETE CASCADE,
            FOREIGN KEY (base_summary_id)
                REFERENCES context_compaction_summaries(id) ON DELETE CASCADE
        );

        CREATE TRIGGER IF NOT EXISTS validate_conversation_world_state_epoch_summary_insert
        BEFORE INSERT ON conversation_world_state_epochs
        WHEN NEW.base_summary_id IS NOT NULL
          AND NOT EXISTS (
              SELECT 1
              FROM context_compaction_summaries
              WHERE id = NEW.base_summary_id
                AND conversation_id = NEW.conversation_id
          )
        BEGIN
            SELECT RAISE(
                ABORT,
                'world state epoch summary must belong to the same conversation'
            );
        END;

        CREATE TRIGGER IF NOT EXISTS validate_conversation_world_state_epoch_summary_update
        BEFORE UPDATE OF conversation_id, base_summary_id
        ON conversation_world_state_epochs
        WHEN NEW.base_summary_id IS NOT NULL
          AND NOT EXISTS (
              SELECT 1
              FROM context_compaction_summaries
              WHERE id = NEW.base_summary_id
                AND conversation_id = NEW.conversation_id
          )
        BEGIN
            SELECT RAISE(
                ABORT,
                'world state epoch summary must belong to the same conversation'
            );
        END;

        CREATE TABLE IF NOT EXISTS conversation_world_state_records (
            journal_position INTEGER PRIMARY KEY AUTOINCREMENT,
            conversation_id TEXT NOT NULL,
            schema_version INTEGER NOT NULL CHECK (schema_version > 0),
            epoch_id TEXT NOT NULL CHECK (
                length(CAST(epoch_id AS BLOB)) BETWEEN 1 AND 256
            ),
            sequence INTEGER NOT NULL CHECK (sequence >= 0),
            record_kind TEXT NOT NULL CHECK (record_kind IN ('full', 'diff')),
            base_revision TEXT,
            result_revision TEXT NOT NULL CHECK (
                length(CAST(result_revision AS BLOB)) BETWEEN 1 AND 256
            ),
            effective_before_message_id TEXT,
            record_json TEXT NOT NULL CHECK (length(trim(record_json)) > 0),
            created_at INTEGER NOT NULL CHECK (created_at >= 0),
            UNIQUE (conversation_id, epoch_id, sequence),
            CHECK (
                (record_kind = 'full' AND base_revision IS NULL)
                OR (
                    record_kind = 'diff'
                    AND length(CAST(base_revision AS BLOB)) BETWEEN 1 AND 256
                )
            ),
            FOREIGN KEY (conversation_id) REFERENCES conversations(id) ON DELETE CASCADE,
            FOREIGN KEY (conversation_id, epoch_id)
                REFERENCES conversation_world_state_epochs(conversation_id, epoch_id)
                ON DELETE CASCADE,
            FOREIGN KEY (effective_before_message_id) REFERENCES messages(id) ON DELETE CASCADE
        );

        CREATE TRIGGER IF NOT EXISTS validate_conversation_world_state_anchor_insert
        BEFORE INSERT ON conversation_world_state_records
        WHEN NEW.effective_before_message_id IS NOT NULL
          AND NOT EXISTS (
              SELECT 1
              FROM messages
              WHERE id = NEW.effective_before_message_id
                AND conversation_id = NEW.conversation_id
          )
        BEGIN
            SELECT RAISE(
                ABORT,
                'world state anchor must be a message in the same conversation'
            );
        END;

        CREATE TRIGGER IF NOT EXISTS validate_conversation_world_state_anchor_update
        BEFORE UPDATE OF conversation_id, effective_before_message_id
        ON conversation_world_state_records
        WHEN NEW.effective_before_message_id IS NOT NULL
          AND NOT EXISTS (
              SELECT 1
              FROM messages
              WHERE id = NEW.effective_before_message_id
                AND conversation_id = NEW.conversation_id
          )
        BEGIN
            SELECT RAISE(
                ABORT,
                'world state anchor must be a message in the same conversation'
            );
        END;

        CREATE TRIGGER IF NOT EXISTS rewind_conversation_world_state_after_record_delete
        AFTER DELETE ON conversation_world_state_records
        BEGIN
            DELETE FROM conversation_world_state_records
            WHERE conversation_id = OLD.conversation_id
              AND epoch_id = OLD.epoch_id
              AND sequence > OLD.sequence;
            DELETE FROM conversation_world_state_epochs
            WHERE conversation_id = OLD.conversation_id
              AND epoch_id = OLD.epoch_id
              AND NOT EXISTS (
                  SELECT 1
                  FROM conversation_world_state_records
                  WHERE conversation_id = OLD.conversation_id
                    AND epoch_id = OLD.epoch_id
              );
        END;

        CREATE TABLE IF NOT EXISTS agent_run_guidances (
            guidance_id TEXT PRIMARY KEY CHECK (length(trim(guidance_id)) > 0),
            client_message_id TEXT NOT NULL CHECK (length(trim(client_message_id)) > 0),
            run_id TEXT NOT NULL CHECK (length(trim(run_id)) > 0),
            conversation_id TEXT NOT NULL,
            assistant_message_id TEXT NOT NULL,
            content TEXT NOT NULL CHECK (length(trim(content)) > 0),
            status TEXT NOT NULL CHECK (status IN ('queued', 'applied', 'rejected', 'abandoned')),
            applied_trace_sequence INTEGER CHECK (
                applied_trace_sequence IS NULL OR applied_trace_sequence >= 0
            ),
            terminal_reason TEXT,
            created_at INTEGER NOT NULL CHECK (created_at >= 0),
            updated_at INTEGER NOT NULL CHECK (updated_at >= created_at),
            UNIQUE (run_id, client_message_id),
            UNIQUE (assistant_message_id, applied_trace_sequence),
            CHECK (
                (status = 'queued' AND applied_trace_sequence IS NULL AND terminal_reason IS NULL)
                OR (
                    status = 'applied'
                    AND applied_trace_sequence IS NOT NULL
                    AND terminal_reason IS NULL
                )
                OR (
                    status IN ('rejected', 'abandoned')
                    AND applied_trace_sequence IS NULL
                    AND length(trim(terminal_reason)) > 0
                )
            ),
            FOREIGN KEY (conversation_id) REFERENCES conversations(id) ON DELETE CASCADE,
            FOREIGN KEY (assistant_message_id) REFERENCES messages(id) ON DELETE CASCADE
        );

        CREATE TABLE IF NOT EXISTS agent_run_guidance_attachments (
            guidance_id TEXT NOT NULL,
            attachment_id TEXT NOT NULL UNIQUE,
            position INTEGER NOT NULL CHECK (position >= 0),
            PRIMARY KEY (guidance_id, position),
            FOREIGN KEY (guidance_id) REFERENCES agent_run_guidances(guidance_id) ON DELETE CASCADE,
            FOREIGN KEY (attachment_id) REFERENCES attachments(id) ON DELETE CASCADE
        );

        CREATE TABLE IF NOT EXISTS agent_turn_diffs (
            assistant_message_id TEXT PRIMARY KEY,
            conversation_id TEXT NOT NULL,
            run_id TEXT NOT NULL UNIQUE,
            project_id TEXT NOT NULL,
            workspace_root TEXT NOT NULL CHECK (length(trim(workspace_root)) > 0),
            schema_version INTEGER NOT NULL CHECK (schema_version > 0),
            truncated INTEGER NOT NULL CHECK (truncated IN (0, 1)),
            created_at INTEGER NOT NULL CHECK (created_at >= 0),
            updated_at INTEGER NOT NULL CHECK (updated_at >= created_at),
            FOREIGN KEY (assistant_message_id) REFERENCES messages(id) ON DELETE CASCADE,
            FOREIGN KEY (conversation_id) REFERENCES conversations(id) ON DELETE CASCADE,
            FOREIGN KEY (project_id) REFERENCES projects(id) ON DELETE CASCADE
        );

        CREATE TABLE IF NOT EXISTS agent_turn_diff_files (
            assistant_message_id TEXT NOT NULL,
            path TEXT NOT NULL CHECK (length(trim(path)) > 0),
            before_kind TEXT NOT NULL CHECK (before_kind IN ('missing', 'text', 'binary', 'too_large')),
            before_text TEXT,
            after_kind TEXT NOT NULL CHECK (after_kind IN ('missing', 'text', 'binary', 'too_large')),
            after_text TEXT,
            PRIMARY KEY (assistant_message_id, path),
            CHECK (
                (before_kind = 'text' AND before_text IS NOT NULL)
                OR (before_kind != 'text' AND before_text IS NULL)
            ),
            CHECK (
                (after_kind = 'text' AND after_text IS NOT NULL)
                OR (after_kind != 'text' AND after_text IS NULL)
            ),
            FOREIGN KEY (assistant_message_id) REFERENCES agent_turn_diffs(assistant_message_id) ON DELETE CASCADE
        );

        CREATE TABLE IF NOT EXISTS agent_turn_diff_actions (
            assistant_message_id TEXT NOT NULL,
            action_id TEXT NOT NULL CHECK (length(trim(action_id)) > 0),
            created_at INTEGER NOT NULL CHECK (created_at >= 0),
            PRIMARY KEY (assistant_message_id, action_id),
            FOREIGN KEY (assistant_message_id) REFERENCES agent_turn_diffs(assistant_message_id) ON DELETE CASCADE
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
            uncovered_tail_input_tokens INTEGER NOT NULL DEFAULT 0 CHECK (
                uncovered_tail_input_tokens >= 0
            ),
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

        CREATE TABLE IF NOT EXISTS context_compaction_summary_lineage (
            summary_id TEXT PRIMARY KEY,
            conversation_id TEXT NOT NULL,
            introduced_by_assistant_message_id TEXT NOT NULL,
            source_conversation_id TEXT,
            source_summary_id TEXT,
            created_at INTEGER NOT NULL CHECK (created_at >= 0),
            FOREIGN KEY (summary_id) REFERENCES context_compaction_summaries(id) ON DELETE CASCADE,
            FOREIGN KEY (conversation_id) REFERENCES conversations(id) ON DELETE CASCADE,
            FOREIGN KEY (introduced_by_assistant_message_id) REFERENCES messages(id) ON DELETE CASCADE
        );

        CREATE TRIGGER IF NOT EXISTS delete_context_compaction_summary_with_lineage
        AFTER DELETE ON context_compaction_summary_lineage
        WHEN EXISTS (
            SELECT 1 FROM context_compaction_summaries WHERE id = OLD.summary_id
        )
        BEGIN
            DELETE FROM context_compaction_summaries WHERE id = OLD.summary_id;
        END;

        CREATE TRIGGER IF NOT EXISTS validate_context_compaction_summary_lineage_insert
        BEFORE INSERT ON context_compaction_summary_lineage
        WHEN NOT EXISTS (
            SELECT 1
            FROM messages
            WHERE id = NEW.introduced_by_assistant_message_id
              AND conversation_id = NEW.conversation_id
              AND role = 'assistant'
        )
        BEGIN
            SELECT RAISE(ABORT, 'compaction summary owner must be an assistant message in the same conversation');
        END;

        CREATE TRIGGER IF NOT EXISTS validate_context_compaction_summary_lineage_update
        BEFORE UPDATE OF conversation_id, introduced_by_assistant_message_id
        ON context_compaction_summary_lineage
        WHEN NOT EXISTS (
            SELECT 1
            FROM messages
            WHERE id = NEW.introduced_by_assistant_message_id
              AND conversation_id = NEW.conversation_id
              AND role = 'assistant'
        )
        BEGIN
            SELECT RAISE(ABORT, 'compaction summary owner must be an assistant message in the same conversation');
        END;

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

        CREATE TABLE IF NOT EXISTS conversation_forks (
            request_id TEXT PRIMARY KEY,
            target_conversation_id TEXT NOT NULL UNIQUE,
            source_conversation_id TEXT NOT NULL,
            source_message_id TEXT NOT NULL,
            target_message_id TEXT,
            created_at INTEGER NOT NULL CHECK (created_at >= 0),
            FOREIGN KEY (target_conversation_id) REFERENCES conversations(id) ON DELETE CASCADE,
            FOREIGN KEY (target_message_id) REFERENCES messages(id) ON DELETE CASCADE
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
        CREATE INDEX IF NOT EXISTS idx_conversation_world_state_records_conversation
            ON conversation_world_state_records(conversation_id, journal_position);
        CREATE INDEX IF NOT EXISTS idx_conversation_world_state_records_anchor
            ON conversation_world_state_records(effective_before_message_id);
        CREATE INDEX IF NOT EXISTS idx_conversation_world_state_epochs_active
            ON conversation_world_state_epochs(conversation_id, generation DESC);
        CREATE INDEX IF NOT EXISTS idx_agent_run_guidances_run_status ON agent_run_guidances(run_id, status, created_at);
        CREATE INDEX IF NOT EXISTS idx_agent_run_guidances_conversation ON agent_run_guidances(conversation_id, created_at);
        CREATE INDEX IF NOT EXISTS idx_agent_turn_diffs_conversation_project ON agent_turn_diffs(conversation_id, project_id, updated_at);
        CREATE INDEX IF NOT EXISTS idx_context_compaction_summaries_conversation_id ON context_compaction_summaries(conversation_id, created_at);
        CREATE INDEX IF NOT EXISTS idx_context_compaction_summary_lineage_owner ON context_compaction_summary_lineage(conversation_id, introduced_by_assistant_message_id);
        CREATE INDEX IF NOT EXISTS idx_context_compaction_summary_lineage_source ON context_compaction_summary_lineage(source_conversation_id, source_summary_id);
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

    // Older summaries predate explicit causal ownership. Applied receipts identify the exact
    // assistant turn. For receipt-less development summaries, use the latest assistant message
    // that already existed when the summary was committed and is not before its coverage cursor.
    connection.execute_batch(
        "
        WITH inferred_lineage AS (
            SELECT
                summary.id AS summary_id,
                summary.conversation_id,
                COALESCE(
                    (
                        SELECT receipt.assistant_message_id
                        FROM context_compaction_receipts AS receipt
                        INNER JOIN messages AS receipt_owner
                            ON receipt_owner.id = receipt.assistant_message_id
                           AND receipt_owner.conversation_id = summary.conversation_id
                           AND receipt_owner.role = 'assistant'
                        WHERE receipt.summary_id = summary.id AND receipt.status = 'applied'
                        ORDER BY receipt.completed_at DESC, receipt.operation_id DESC
                        LIMIT 1
                    ),
                    (
                        SELECT owner.id
                        FROM messages AS owner
                        INNER JOIN messages AS covered
                            ON covered.id = summary.covered_through_message_id
                           AND covered.conversation_id = summary.conversation_id
                        WHERE owner.conversation_id = summary.conversation_id
                          AND owner.role = 'assistant'
                          AND owner.position >= covered.position
                          AND owner.created_at <= summary.created_at
                        ORDER BY owner.position DESC, owner.created_at DESC, owner.id DESC
                        LIMIT 1
                    )
                ) AS introduced_by_assistant_message_id,
                summary.created_at
            FROM context_compaction_summaries AS summary
        )
        INSERT OR IGNORE INTO context_compaction_summary_lineage (
            summary_id, conversation_id, introduced_by_assistant_message_id,
            source_conversation_id, source_summary_id, created_at
        )
        SELECT
            summary_id,
            conversation_id,
            introduced_by_assistant_message_id,
            NULL,
            NULL,
            created_at
        FROM inferred_lineage
        WHERE introduced_by_assistant_message_id IS NOT NULL;

        DELETE FROM context_compaction_summaries
        WHERE NOT EXISTS (
            SELECT 1
            FROM context_compaction_summary_lineage AS lineage
            WHERE lineage.summary_id = context_compaction_summaries.id
        );

        DELETE FROM context_compaction_receipts
        WHERE summary_id IS NOT NULL
          AND NOT EXISTS (
              SELECT 1
              FROM context_compaction_summaries AS summary
              WHERE summary.id = context_compaction_receipts.summary_id
          );
        ",
    )?;

    upgrade_conversation_trace_commit_schema(connection)?;
    upgrade_conversation_trace_v3_schema(connection)?;
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
    add_column_if_missing(
        connection,
        "conversation_forks",
        "target_message_id",
        "TEXT REFERENCES messages(id) ON DELETE CASCADE",
    )?;
    // A fork preserves message order while replacing identities. Existing development rows can
    // therefore recover the cloned boundary by matching the source cutoff's position. If the
    // source task was already deleted, leave the legacy row unprojected rather than guessing.
    connection.execute(
        "
        UPDATE conversation_forks
        SET target_message_id = (
            SELECT target_message.id
            FROM messages AS source_message
            INNER JOIN messages AS target_message
                ON target_message.conversation_id = conversation_forks.target_conversation_id
               AND target_message.position = source_message.position
            WHERE source_message.id = conversation_forks.source_message_id
              AND source_message.conversation_id = conversation_forks.source_conversation_id
        )
        WHERE target_message_id IS NULL
        ",
        [],
    )?;
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
    add_column_if_missing(connection, "agent_pending_actions", "target_status", "TEXT")?;
    add_column_if_missing(
        connection,
        "context_compaction_summaries",
        "uncovered_tail_input_tokens",
        "INTEGER NOT NULL DEFAULT 0 CHECK (uncovered_tail_input_tokens >= 0)",
    )?;

    upgrade_canonical_model_identity_schema(connection)?;
    upgrade_usage_consistency_schema(connection)?;
    ensure_legacy_task_state_schema(connection)?;
    ensure_conversation_goal_schema(connection)?;
    ensure_conversation_history_fts_schema(connection)?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backfills_pre_journal_goals_as_unattributed_initial_snapshots() {
        let connection = Connection::open_in_memory().unwrap();
        run_migrations(&connection).unwrap();
        connection
            .execute(
                "INSERT INTO conversations (
                    id, project_id, model_id, title, created_at, updated_at,
                    pinned_at, archived_at, unread_at
                 ) VALUES ('conversation-goal', NULL, NULL, 'goal', 1, 1, NULL, NULL, NULL)",
                [],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO messages (
                    id, conversation_id, role, content, status,
                    agent_run_json, ui_state_json, created_at, position
                 ) VALUES (
                    'message-goal', 'conversation-goal', 'user', 'track it', 'sent',
                    NULL, NULL, 1, 0
                 )",
                [],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO conversation_goals (
                    conversation_id, goal_id, objective, source_message_id, status,
                    stopped_reason, created_at, updated_at
                 ) VALUES (
                    'conversation-goal', 'goal-legacy', 'Finish the migration.',
                    'message-goal', 'blocked', 'Waiting for input.', 2, 3
                 )",
                [],
            )
            .unwrap();

        run_migrations(&connection).unwrap();

        let (actor, event_kind, event_json) = connection
            .query_row(
                "SELECT actor, event_kind, event_json
                 FROM conversation_goal_revisions
                 WHERE goal_id = 'goal-legacy' AND sequence = 1",
                [],
                |row| {
                    Ok((
                        row.get::<_, Option<String>>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                    ))
                },
            )
            .unwrap();
        assert_eq!(actor, None);
        assert_eq!(event_kind, "initial");
        let event: crate::ConversationGoalRevisionEvent =
            serde_json::from_str(&event_json).unwrap();
        assert!(matches!(
            event,
            crate::ConversationGoalRevisionEvent::Initial { goal }
                if goal.goal_id == "goal-legacy"
                    && goal.status == crate::ConversationGoalStatus::Blocked
                    && goal.stopped_reason.as_deref() == Some("Waiting for input.")
        ));
        assert!(connection
            .execute(
                "UPDATE conversation_goal_revisions
                 SET created_at = 4
                 WHERE goal_id = 'goal-legacy'",
                [],
            )
            .unwrap_err()
            .to_string()
            .contains("append-only"));
        assert!(connection
            .execute(
                "DELETE FROM conversation_goal_revisions
                 WHERE goal_id = 'goal-legacy'",
                [],
            )
            .unwrap_err()
            .to_string()
            .contains("append-only"));
    }

    #[test]
    fn repairs_usage_rows_to_match_live_messages_and_zero_default() {
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
                    FOREIGN KEY (conversation_id)
                        REFERENCES conversations(id) ON DELETE CASCADE
                );
                CREATE TABLE agent_usage_records (
                    id TEXT PRIMARY KEY,
                    conversation_id TEXT NOT NULL,
                    message_id TEXT NOT NULL,
                    run_id TEXT NOT NULL,
                    project_id TEXT,
                    model_id TEXT NOT NULL,
                    model_name TEXT NOT NULL,
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
                    FOREIGN KEY (conversation_id)
                        REFERENCES conversations(id) ON DELETE CASCADE
                );

                INSERT INTO conversations (
                    id, project_id, model_id, title, created_at, updated_at,
                    pinned_at, archived_at, unread_at
                ) VALUES
                    ('conversation-1', NULL, NULL, 'one', 1, 1, NULL, NULL, NULL),
                    ('conversation-2', NULL, NULL, 'two', 1, 1, NULL, NULL, NULL);
                INSERT INTO messages (
                    id, conversation_id, role, content, status,
                    agent_run_json, ui_state_json, created_at, position
                ) VALUES (
                    'message-1', 'conversation-1', 'assistant', 'done', 'sent',
                    NULL, NULL, 1, 0
                );
                INSERT INTO agent_usage_records (
                    id, conversation_id, message_id, run_id,
                    model_id, model_name, created_at, billable_request_count
                ) VALUES
                    ('usage-valid', 'conversation-1', 'message-1', 'run-valid',
                     'model', 'Model', 1, 2),
                    ('usage-orphan', 'conversation-1', 'missing-message', 'run-orphan',
                     'model', 'Model', 1, 1),
                    ('usage-mismatch', 'conversation-2', 'message-1', 'run-mismatch',
                     'model', 'Model', 1, 1);
                ",
            )
            .unwrap();

        run_migrations(&connection).unwrap();

        assert!(usage_billable_default_is_zero(&connection).unwrap());
        assert!(usage_has_message_cascade(&connection).unwrap());
        assert_eq!(
            connection
                .query_row(
                    "SELECT id || ':' || billable_request_count
                     FROM agent_usage_records",
                    [],
                    |row| row.get::<_, String>(0),
                )
                .unwrap(),
            "usage-valid:2"
        );
        assert_eq!(
            connection
                .query_row(
                    "SELECT request_count || ':' || message_count
                     FROM agent_deleted_usage_daily_rollups",
                    [],
                    |row| row.get::<_, String>(0),
                )
                .unwrap(),
            "2:2"
        );
        let preserved_request_count: i64 = connection
            .query_row(
                "SELECT
                    (SELECT COALESCE(SUM(billable_request_count), 0)
                     FROM agent_usage_records)
                    +
                    (SELECT COALESCE(SUM(request_count), 0)
                     FROM agent_deleted_usage_daily_rollups)",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(preserved_request_count, 4);

        let mismatch = connection.execute(
            "INSERT INTO agent_usage_records (
                id, conversation_id, message_id, run_id,
                model_id, model_name, created_at
             ) VALUES (
                'usage-invalid', 'conversation-2', 'message-1', 'run-invalid',
                'model', 'Model', 2
             )",
            [],
        );
        assert!(mismatch
            .unwrap_err()
            .to_string()
            .contains("agent usage message must belong to the same conversation"));

        connection
            .execute("DELETE FROM messages WHERE id = 'message-1'", [])
            .unwrap();
        assert_eq!(
            connection
                .query_row("SELECT COUNT(*) FROM agent_usage_records", [], |row| {
                    row.get::<_, i64>(0)
                })
                .unwrap(),
            0
        );

        connection
            .execute(
                "INSERT INTO messages (
                    id, conversation_id, role, content, status,
                    agent_run_json, ui_state_json, created_at, position
                 ) VALUES (
                    'message-2', 'conversation-1', 'assistant', 'done', 'sent',
                    NULL, NULL, 2, 0
                 )",
                [],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO agent_usage_records (
                    id, conversation_id, message_id, run_id,
                    model_id, model_name, created_at
                 ) VALUES (
                    'usage-default', 'conversation-1', 'message-2', 'run-default',
                    'model', 'Model', 2
                 )",
                [],
            )
            .unwrap();
        assert_eq!(
            connection
                .query_row(
                    "SELECT billable_request_count
                     FROM agent_usage_records
                     WHERE id = 'usage-default'",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            0
        );
    }

    #[test]
    fn creates_idempotent_world_state_journal_with_cascade_anchors() {
        let connection = Connection::open_in_memory().unwrap();
        run_migrations(&connection).unwrap();
        run_migrations(&connection).unwrap();

        for column in [
            "conversation_id",
            "schema_version",
            "epoch_id",
            "sequence",
            "record_kind",
            "base_revision",
            "result_revision",
            "effective_before_message_id",
            "record_json",
        ] {
            assert!(
                table_has_column(&connection, "conversation_world_state_records", column).unwrap()
            );
        }
        for column in [
            "conversation_id",
            "epoch_id",
            "generation",
            "base_summary_id",
        ] {
            assert!(
                table_has_column(&connection, "conversation_world_state_epochs", column).unwrap()
            );
        }

        let record_foreign_keys = {
            let mut statement = connection
                .prepare("PRAGMA foreign_key_list(conversation_world_state_records)")
                .unwrap();
            statement
                .query_map([], |row| {
                    Ok((
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, String>(6)?,
                    ))
                })
                .unwrap()
                .collect::<rusqlite::Result<Vec<_>>>()
                .unwrap()
        };
        assert!(record_foreign_keys
            .iter()
            .any(|(table, column, on_delete)| {
                table == "conversations"
                    && column == "conversation_id"
                    && on_delete.eq_ignore_ascii_case("cascade")
            }));
        assert!(record_foreign_keys
            .iter()
            .any(|(table, column, on_delete)| {
                table == "messages"
                    && column == "effective_before_message_id"
                    && on_delete.eq_ignore_ascii_case("cascade")
            }));

        let epoch_foreign_keys = {
            let mut statement = connection
                .prepare("PRAGMA foreign_key_list(conversation_world_state_epochs)")
                .unwrap();
            statement
                .query_map([], |row| {
                    Ok((
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, String>(6)?,
                    ))
                })
                .unwrap()
                .collect::<rusqlite::Result<Vec<_>>>()
                .unwrap()
        };
        assert!(epoch_foreign_keys.iter().any(|(table, column, on_delete)| {
            table == "context_compaction_summaries"
                && column == "base_summary_id"
                && on_delete.eq_ignore_ascii_case("cascade")
        }));
    }

    #[test]
    fn upgrades_existing_composer_drafts_without_granting_new_full_permissions() {
        let connection = Connection::open_in_memory().unwrap();
        connection
            .execute_batch(
                "
                CREATE TABLE composer_drafts (
                    scope_id TEXT PRIMARY KEY,
                    message TEXT NOT NULL,
                    permission_mode TEXT NOT NULL,
                    model_id TEXT,
                    project_id TEXT,
                    attachments_json TEXT NOT NULL,
                    updated_at INTEGER NOT NULL
                );
                INSERT INTO composer_drafts VALUES (
                    'conversation-1', 'draft', 'full', NULL, NULL, '[]', 1
                );
                ",
            )
            .unwrap();

        run_migrations(&connection).unwrap();

        let (permission_mode, permission_mode_version, skills_json, queued_messages_json) =
            connection
            .query_row(
                "SELECT permission_mode, permission_mode_version, skills_json, queued_messages_json
                 FROM composer_drafts WHERE scope_id = 'conversation-1'",
                [],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                    ))
                },
            )
            .unwrap();
        assert_eq!(permission_mode, "default");
        assert_eq!(permission_mode_version, 0);
        assert_eq!(skills_json, "[]");
        assert_eq!(queued_messages_json, "[]");
    }

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

        let (context_window, api_url_override, api_token_override) = connection
            .query_row(
                "SELECT context_window_tokens, api_url_override, api_token_override
                 FROM models WHERE id = 'model-a'",
                [],
                |row| {
                    Ok((
                        row.get::<_, Option<u32>>(0)?,
                        row.get::<_, Option<String>>(1)?,
                        row.get::<_, Option<String>>(2)?,
                    ))
                },
            )
            .unwrap();
        assert_eq!(context_window, None);
        assert_eq!(api_url_override, None);
        assert_eq!(api_token_override, None);
        assert!(!table_has_column(&connection, "models", "short_name").unwrap());
        assert!(!table_has_column(&connection, "models", "provider_path").unwrap());
    }

    #[test]
    fn canonicalizes_legacy_model_identity_without_losing_references_or_usage() {
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

                INSERT INTO conversations (
                    id, model_id, title, created_at, updated_at
                ) VALUES ('conversation-1', 'model-a', 'Conversation', 1, 1);

                INSERT INTO messages (
                    id, conversation_id, role, content, status,
                    agent_run_json, ui_state_json, created_at, position
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

                INSERT INTO agent_deleted_usage_daily_rollups (
                    usage_day, model_id, model_name, provider_path_key,
                    input_tokens, created_at, updated_at
                ) VALUES
                    (0, 'model-a', 'model-a', 'provider/model-a', 13, 1, 1),
                    (0, 'provider/model-a', 'provider/model-a', '', 17, 2, 2);
                ",
            )
            .unwrap();

        run_migrations(&connection).unwrap();

        assert!(!table_has_column(&connection, "models", "short_name").unwrap());
        assert!(!table_has_column(&connection, "models", "provider_path").unwrap());
        assert!(!table_has_column(&connection, "agent_usage_records", "provider_path").unwrap());
        assert!(!table_has_column(
            &connection,
            "agent_deleted_usage_daily_rollups",
            "provider_path_key"
        )
        .unwrap());
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

        let conversation_model: String = connection
            .query_row(
                "SELECT model_id FROM conversations WHERE id = 'conversation-1'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let draft_model: String = connection
            .query_row(
                "SELECT model_id FROM composer_drafts WHERE scope_id = 'conversation-1'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(conversation_model, "provider/model-a");
        assert_eq!(draft_model, "provider/model-a");

        let usage = connection
            .query_row(
                "SELECT model_id, model_name, input_tokens
                 FROM agent_usage_records WHERE id = 'usage-1'",
                [],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, i64>(2)?,
                    ))
                },
            )
            .unwrap();
        assert_eq!(
            usage,
            (
                "provider/model-a".to_string(),
                "provider/model-a".to_string(),
                11,
            )
        );

        let rollup = connection
            .query_row(
                "SELECT model_id, model_name, input_tokens
                 FROM agent_deleted_usage_daily_rollups",
                [],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, i64>(2)?,
                    ))
                },
            )
            .unwrap();
        assert_eq!(
            rollup,
            (
                "provider/model-a".to_string(),
                "provider/model-a".to_string(),
                30,
            )
        );

        run_migrations(&connection).unwrap();
        let model_count: i64 = connection
            .query_row("SELECT COUNT(*) FROM models", [], |row| row.get(0))
            .unwrap();
        assert_eq!(model_count, 1);
    }

    #[test]
    fn adds_enablement_generation_without_changing_existing_preferences() {
        let connection = Connection::open_in_memory().unwrap();
        connection
            .execute_batch(
                "
                CREATE TABLE skill_enablement_overrides (
                    skill_id TEXT PRIMARY KEY,
                    enabled INTEGER NOT NULL CHECK (enabled IN (0, 1)),
                    updated_at INTEGER NOT NULL
                );
                INSERT INTO skill_enablement_overrides (skill_id, enabled, updated_at)
                VALUES ('bundled:application:auditor', 0, 42);
                ",
            )
            .unwrap();

        run_migrations(&connection).unwrap();

        let state = connection
            .query_row(
                "SELECT enabled, generation, updated_at FROM skill_enablement_overrides WHERE skill_id = ?1",
                ["bundled:application:auditor"],
                |row| {
                    Ok((
                        row.get::<_, bool>(0)?,
                        row.get::<_, u64>(1)?,
                        row.get::<_, i64>(2)?,
                    ))
                },
            )
            .unwrap();
        assert_eq!(state, (false, 0, 42));
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
                    item_kind TEXT NOT NULL CHECK (item_kind IN ('assistant_narration', 'user_guidance', 'tool_call', 'tool_result')),
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
    fn upgrades_v2_trace_items_in_place_before_incompatible_cleanup() {
        let connection = Connection::open_in_memory().unwrap();
        run_migrations(&connection).unwrap();
        connection
            .execute_batch(
                "INSERT INTO conversations (
                    id, project_id, model_id, title, created_at, updated_at,
                    pinned_at, archived_at, unread_at
                 ) VALUES ('conversation-v2', NULL, NULL, 'title', 1, 1, NULL, NULL, NULL);
                 INSERT INTO messages (
                    id, conversation_id, role, content, status, agent_run_json,
                    ui_state_json, created_at, position
                 ) VALUES (
                    'assistant-v2', 'conversation-v2', 'assistant', 'done', 'sent',
                    NULL, NULL, 1, 0
                 );
                 INSERT INTO conversation_turn_traces (
                    assistant_message_id, conversation_id, run_id, schema_version,
                    terminal_status, terminal_error, truncated, created_at, updated_at, completed_at
                 ) VALUES (
                    'assistant-v2', 'conversation-v2', 'run-v2', 2,
                    'completed', NULL, 0, 1, 2, 2
                 );
                 INSERT INTO conversation_turn_trace_items (
                    assistant_message_id, sequence, item_kind, item_json
                 ) VALUES (
                    'assistant-v2', 0, 'assistant_narration',
                    '{\"type\":\"assistant_narration\",\"sequence\":0,\"content\":\"kept\",\"truncated\":false}'
                 );
                 PRAGMA foreign_keys = OFF;
                 ALTER TABLE conversation_turn_trace_items
                    RENAME TO conversation_turn_trace_items_current;
                 CREATE TABLE conversation_turn_trace_items (
                    assistant_message_id TEXT NOT NULL,
                    sequence INTEGER NOT NULL CHECK (sequence >= 0),
                    item_kind TEXT NOT NULL CHECK (
                        item_kind IN ('assistant_narration', 'tool_call', 'tool_result')
                    ),
                    item_json TEXT NOT NULL,
                    PRIMARY KEY (assistant_message_id, sequence),
                    FOREIGN KEY (assistant_message_id)
                        REFERENCES conversation_turn_traces(assistant_message_id) ON DELETE CASCADE
                 );
                 INSERT INTO conversation_turn_trace_items
                 SELECT * FROM conversation_turn_trace_items_current;
                 DROP TABLE conversation_turn_trace_items_current;
                 PRAGMA foreign_keys = ON;",
            )
            .unwrap();

        run_migrations(&connection).unwrap();

        let (schema_version, item_count) = connection
            .query_row(
                "SELECT trace.schema_version, COUNT(item.sequence)
                 FROM conversation_turn_traces AS trace
                 LEFT JOIN conversation_turn_trace_items AS item
                   ON item.assistant_message_id = trace.assistant_message_id
                 WHERE trace.assistant_message_id = 'assistant-v2'
                 GROUP BY trace.schema_version",
                [],
                |row| Ok((row.get::<_, u32>(0)?, row.get::<_, usize>(1)?)),
            )
            .unwrap();
        assert_eq!(schema_version, CONVERSATION_TURN_TRACE_SCHEMA_VERSION);
        assert_eq!(item_count, 1);
        let item_table_sql = connection
            .query_row(
                "SELECT sql FROM sqlite_master
                 WHERE type = 'table' AND name = 'conversation_turn_trace_items'",
                [],
                |row| row.get::<_, String>(0),
            )
            .unwrap();
        assert!(item_table_sql.contains("'user_guidance'"));
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
    fn removes_only_the_retired_bundled_skill_from_live_preferences_and_drafts() {
        let connection = Connection::open_in_memory().unwrap();
        run_migrations(&connection).unwrap();
        connection
            .execute(
                "DELETE FROM maintenance_tasks WHERE id = ?1",
                [REMOVE_RETIRED_BUNDLED_SKILL_TASK],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO skill_enablement_overrides (skill_id, enabled, generation, updated_at)
                 VALUES (?1, 0, 0, 1), ('bundled:application:documents', 0, 0, 1)",
                [RETIRED_BUNDLED_SKILL_ID],
            )
            .unwrap();
        let mixed_skills = serde_json::json!([
            { "id": RETIRED_BUNDLED_SKILL_ID, "revision": "retired" },
            { "id": "bundled:application:documents", "revision": "documents" },
            { "id": "installed:user:01234567-89ab-4def-8123-456789abcdef", "revision": "installed" }
        ])
        .to_string();
        connection
            .execute(
                "INSERT INTO composer_drafts (
                    scope_id, message, permission_mode, permission_mode_version,
                    model_id, project_id, attachments_json, skills_json, updated_at
                 ) VALUES ('mixed', '', 'default', 1, NULL, NULL, '[]', ?1, 1),
                          ('malformed', '', 'default', 1, NULL, NULL, '[]', 'not-json', 1)",
                [mixed_skills],
            )
            .unwrap();

        run_migrations(&connection).unwrap();

        let migrated = connection
            .query_row(
                "SELECT skills_json FROM composer_drafts WHERE scope_id = 'mixed'",
                [],
                |row| row.get::<_, String>(0),
            )
            .unwrap();
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&migrated).unwrap(),
            serde_json::json!([
                { "id": "bundled:application:documents", "revision": "documents" },
                { "id": "installed:user:01234567-89ab-4def-8123-456789abcdef", "revision": "installed" }
            ])
        );
        let malformed = connection
            .query_row(
                "SELECT skills_json FROM composer_drafts WHERE scope_id = 'malformed'",
                [],
                |row| row.get::<_, String>(0),
            )
            .unwrap();
        assert_eq!(malformed, "not-json");
        let remaining_overrides = connection
            .query_row(
                "SELECT group_concat(skill_id, ',') FROM skill_enablement_overrides",
                [],
                |row| row.get::<_, String>(0),
            )
            .unwrap();
        assert_eq!(remaining_overrides, "bundled:application:documents");

        run_migrations(&connection).unwrap();
        let rerun = connection
            .query_row(
                "SELECT skills_json FROM composer_drafts WHERE scope_id = 'mixed'",
                [],
                |row| row.get::<_, String>(0),
            )
            .unwrap();
        assert_eq!(rerun, migrated);
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
