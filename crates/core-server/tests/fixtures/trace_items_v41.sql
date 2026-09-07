CREATE TABLE conversation_turn_trace_items (
            assistant_message_id TEXT NOT NULL,
            sequence INTEGER NOT NULL CHECK (sequence >= 0),
            item_kind TEXT NOT NULL CHECK (item_kind IN (
                'assistant_narration', 'user_guidance', 'tool_call', 'tool_result',
                'command_session_lifecycle', 'agent_mailbox_delivery',
                'context_compaction_lifecycle', 'runtime_error', 'backend_state'
            )),
            item_json TEXT NOT NULL CHECK (json_valid(item_json)),
            PRIMARY KEY (assistant_message_id, sequence),
            FOREIGN KEY (assistant_message_id) REFERENCES conversation_turn_traces(assistant_message_id) ON DELETE CASCADE
        );
CREATE UNIQUE INDEX conversation_trace_command_session_lifecycle_identity
         ON conversation_turn_trace_items (
             assistant_message_id,
             json_extract(item_json, '$.sessionId'),
             json_extract(item_json, '$.phase')
         )
         WHERE item_kind = 'command_session_lifecycle';
CREATE TRIGGER conversation_history_fts_trace_delete
AFTER DELETE ON conversation_turn_trace_items
BEGIN
    DELETE FROM conversation_history_fts
    WHERE ref_key = 'trace:' || OLD.assistant_message_id || ':' || OLD.sequence;
END;
CREATE TRIGGER conversation_history_fts_trace_insert
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
    WHERE trace.assistant_message_id = NEW.assistant_message_id
      AND NEW.item_kind != 'agent_mailbox_delivery';
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
CREATE TRIGGER conversation_history_fts_trace_update
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
    WHERE trace.assistant_message_id = NEW.assistant_message_id
      AND NEW.item_kind != 'agent_mailbox_delivery';
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
