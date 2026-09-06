CREATE TABLE conversation_world_state_records (
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
            record_json TEXT NOT NULL CHECK (
                json_valid(record_json) AND length(trim(record_json)) > 0
            ),
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
CREATE TRIGGER validate_conversation_world_state_anchor_insert
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
CREATE TRIGGER validate_conversation_world_state_anchor_update
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
CREATE TRIGGER rewind_conversation_world_state_after_record_delete
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
CREATE INDEX idx_conversation_world_state_records_conversation
            ON conversation_world_state_records(conversation_id, journal_position);
CREATE INDEX idx_conversation_world_state_records_anchor
            ON conversation_world_state_records(effective_before_message_id);
