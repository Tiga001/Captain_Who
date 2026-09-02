DROP TRIGGER conversations_revision_after_message_update;
DROP TRIGGER prevent_agent_message_projection_ui_rewrite;
ALTER TABLE messages DROP COLUMN ui_state_json;
CREATE TABLE chat_message_ui_states (
            message_id TEXT PRIMARY KEY NOT NULL,
            ui_state_json TEXT NOT NULL CHECK (json_valid(ui_state_json)),
            FOREIGN KEY (message_id) REFERENCES messages(id) ON DELETE CASCADE
        );
CREATE TRIGGER conversations_revision_after_message_update
        AFTER UPDATE ON messages
        WHEN NEW.conversation_id IS NOT OLD.conversation_id
          OR NEW.role IS NOT OLD.role
          OR NEW.content IS NOT OLD.content
          OR NEW.status IS NOT OLD.status
          OR NEW.input_origin_kind IS NOT OLD.input_origin_kind
          OR NEW.input_origin_agent_id IS NOT OLD.input_origin_agent_id
          OR NEW.source_agent_message_id IS NOT OLD.source_agent_message_id
          OR NEW.snapshot_source_conversation_id IS NOT OLD.snapshot_source_conversation_id
          OR NEW.snapshot_source_message_id IS NOT OLD.snapshot_source_message_id
          OR NEW.snapshot_original_origin_kind IS NOT OLD.snapshot_original_origin_kind
          OR NEW.snapshot_original_agent_id IS NOT OLD.snapshot_original_agent_id
          OR NEW.snapshot_original_mailbox_message_id IS NOT OLD.snapshot_original_mailbox_message_id
          OR NEW.agent_run_json IS NOT OLD.agent_run_json
          OR NEW.created_at IS NOT OLD.created_at
          OR NEW.position IS NOT OLD.position
        BEGIN
            UPDATE conversations
            SET revision = revision + 1
            WHERE id IN (OLD.conversation_id, NEW.conversation_id);
        END;
CREATE TRIGGER prevent_agent_message_projection_run_state_rewrite
        BEFORE UPDATE OF agent_run_json ON messages
        WHEN OLD.input_origin_kind = 'agent'
          AND NEW.agent_run_json IS NOT OLD.agent_run_json
        BEGIN
            SELECT RAISE(ABORT, 'Agent input projection cannot carry mutable run state');
        END;
