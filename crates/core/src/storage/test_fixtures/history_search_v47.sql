CREATE TRIGGER conversation_history_fts_message_insert
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

CREATE TRIGGER conversation_history_fts_message_update
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

CREATE TRIGGER conversation_history_fts_message_delete
AFTER DELETE ON messages
BEGIN
    DELETE FROM conversation_history_fts WHERE ref_key = 'message:' || OLD.id;
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

CREATE TRIGGER conversation_history_fts_trace_delete
AFTER DELETE ON conversation_turn_trace_items
BEGIN
    DELETE FROM conversation_history_fts
    WHERE ref_key = 'trace:' || OLD.assistant_message_id || ':' || OLD.sequence;
END;

CREATE TRIGGER conversation_history_fts_archive_delete
AFTER DELETE ON conversation_history_blobs
BEGIN
    DELETE FROM conversation_history_fts WHERE ref_key = 'archive:' || OLD.archive_ref;
END;

