-- Rebuild agent_run_guidances so attachment-only guidance can retain an empty content field.
-- The temporary table is deliberately copied into a freshly named canonical table instead of
-- using ALTER TABLE ... RENAME: SQLite otherwise quotes the renamed table in sqlite_master,
-- changing the canonical schema fingerprint.
CREATE TABLE agent_run_guidances_v49 (
            guidance_id TEXT PRIMARY KEY CHECK (length(trim(guidance_id)) > 0),
            client_message_id TEXT NOT NULL CHECK (length(trim(client_message_id)) > 0),
            run_id TEXT NOT NULL CHECK (length(trim(run_id)) > 0),
            conversation_id TEXT NOT NULL,
            assistant_message_id TEXT NOT NULL,
            content TEXT NOT NULL,
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
INSERT INTO agent_run_guidances_v49 (
            guidance_id, client_message_id, run_id, conversation_id, assistant_message_id,
            content, status, applied_trace_sequence, terminal_reason, created_at, updated_at
        )
        SELECT guidance_id, client_message_id, run_id, conversation_id, assistant_message_id,
               content, status, applied_trace_sequence, terminal_reason, created_at, updated_at
        FROM agent_run_guidances;
DROP TABLE agent_run_guidances;
CREATE TABLE agent_run_guidances (
            guidance_id TEXT PRIMARY KEY CHECK (length(trim(guidance_id)) > 0),
            client_message_id TEXT NOT NULL CHECK (length(trim(client_message_id)) > 0),
            run_id TEXT NOT NULL CHECK (length(trim(run_id)) > 0),
            conversation_id TEXT NOT NULL,
            assistant_message_id TEXT NOT NULL,
            content TEXT NOT NULL,
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
INSERT INTO agent_run_guidances (
            guidance_id, client_message_id, run_id, conversation_id, assistant_message_id,
            content, status, applied_trace_sequence, terminal_reason, created_at, updated_at
        )
        SELECT guidance_id, client_message_id, run_id, conversation_id, assistant_message_id,
               content, status, applied_trace_sequence, terminal_reason, created_at, updated_at
        FROM agent_run_guidances_v49;
DROP TABLE agent_run_guidances_v49;
CREATE INDEX idx_agent_run_guidances_run_status ON agent_run_guidances(run_id, status, created_at);
CREATE INDEX idx_agent_run_guidances_conversation ON agent_run_guidances(conversation_id, created_at);
CREATE TRIGGER human_interaction_async_guidance_proof
BEFORE UPDATE OF status ON agent_run_guidances
WHEN NEW.status='applied' AND EXISTS(SELECT 1 FROM human_interaction_async_bindings WHERE guidance_id=NEW.guidance_id)
AND (EXISTS(SELECT 1 FROM agent_tree_run_stops WHERE run_id=NEW.run_id) OR NOT EXISTS(
    SELECT 1 FROM conversation_turn_trace_items i JOIN conversation_turn_traces t ON t.assistant_message_id=i.assistant_message_id
    WHERE t.run_id=NEW.run_id AND t.conversation_id=NEW.conversation_id AND i.assistant_message_id=NEW.assistant_message_id AND i.sequence=NEW.applied_trace_sequence AND i.item_kind='user_guidance'
      AND json_extract(i.item_json,'$.guidanceId')=NEW.guidance_id AND json_extract(i.item_json,'$.content')=NEW.content
      AND EXISTS(SELECT 1 FROM conversation_model_context_items c WHERE c.assistant_message_id=i.assistant_message_id AND c.sequence=i.sequence)))
BEGIN SELECT RAISE(ABORT,'async human guidance requires its exact committed trace'); END;
CREATE TRIGGER human_interaction_async_guidance_applied
AFTER UPDATE OF status ON agent_run_guidances
WHEN NEW.status='applied'
BEGIN
    UPDATE human_interaction_deliveries SET status='applied',revision=revision+1
    WHERE status='bound' AND response_id IN (SELECT response_id FROM human_interaction_async_bindings WHERE guidance_id=NEW.guidance_id AND status='bound');
    UPDATE human_interaction_async_bindings SET status='applied',updated_at=MAX(updated_at,NEW.updated_at)
    WHERE guidance_id=NEW.guidance_id AND status='bound';
END;
CREATE TRIGGER human_interaction_async_guidance_unconsumed
AFTER UPDATE OF status ON agent_run_guidances
WHEN NEW.status IN ('rejected','abandoned')
BEGIN
    UPDATE human_interaction_deliveries SET status='pending',target_run_id=NULL,user_message_id=NULL,error_code=NULL,revision=revision+1
    WHERE status='bound' AND response_id IN (SELECT response_id FROM human_interaction_async_bindings WHERE guidance_id=NEW.guidance_id AND status='bound');
    UPDATE human_interaction_async_bindings SET status='pending',route=NULL,claim_id=NULL,target_run_id=NULL,assistant_message_id=NULL,guidance_id=NULL,updated_at=MAX(updated_at,NEW.updated_at)
    WHERE guidance_id=NEW.guidance_id AND status='bound' AND EXISTS(SELECT 1 FROM human_interaction_deliveries d WHERE d.response_id=human_interaction_async_bindings.response_id AND d.status='pending');
END;
