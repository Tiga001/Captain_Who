CREATE TABLE agent_collaboration_events (
            global_sequence INTEGER PRIMARY KEY AUTOINCREMENT,
            event_id TEXT NOT NULL UNIQUE CHECK (
                length(CAST(event_id AS BLOB)) BETWEEN 1 AND 128
            ),
            schema_version INTEGER NOT NULL CHECK (schema_version = 2),
            root_agent_id TEXT NOT NULL,
            root_sequence INTEGER NOT NULL CHECK (root_sequence > 0),
            workspace_id TEXT,
            project_id TEXT,
            root_conversation_id TEXT NOT NULL,
            agent_id TEXT NOT NULL,
            conversation_id TEXT NOT NULL,
            turn_id TEXT,
            run_id TEXT,
            message_id TEXT,
            kind TEXT NOT NULL CHECK (kind IN (
                'agent_created', 'agent_updated', 'mailbox_enqueued', 'mailbox_updated',
                'wake_created', 'wake_updated', 'turn_started', 'turn_updated',
                'approval_projected', 'approval_updated'
            )),
            resource_revision INTEGER NOT NULL CHECK (resource_revision > 0),
            activity_schema_version INTEGER CHECK (
                activity_schema_version IS NULL OR activity_schema_version = 2
            ),
            activity_semantic TEXT CHECK (
                activity_semantic IS NULL OR activity_semantic IN (
                    'started', 'updated', 'waiting_approval',
                    'completed', 'failed', 'interrupted'
                )
            ),
            activity_agent_id TEXT,
            activity_task_name_snapshot TEXT CHECK (
                activity_task_name_snapshot IS NULL
                OR length(CAST(activity_task_name_snapshot AS BLOB)) BETWEEN 1 AND 256
            ),
            activity_root_anchor_message_id TEXT CHECK (
                activity_root_anchor_message_id IS NULL
                OR length(CAST(activity_root_anchor_message_id AS BLOB)) BETWEEN 1 AND 2048
            ),
            activity_root_trace_boundary_sequence INTEGER CHECK (
                activity_root_trace_boundary_sequence IS NULL
                OR activity_root_trace_boundary_sequence >= 0
            ),
            created_at INTEGER NOT NULL CHECK (created_at >= 0),
            UNIQUE(root_agent_id, root_sequence),
            CHECK (workspace_id IS project_id),
            CHECK (
                (activity_schema_version IS NULL
                    AND activity_semantic IS NULL
                    AND activity_agent_id IS NULL
                    AND activity_task_name_snapshot IS NULL
                    AND activity_root_anchor_message_id IS NULL
                    AND activity_root_trace_boundary_sequence IS NULL)
                OR (activity_schema_version = 2
                    AND activity_semantic IS NOT NULL
                    AND activity_agent_id IS NOT NULL
                    AND activity_task_name_snapshot IS NOT NULL)
            ),
            CHECK (
                (activity_root_anchor_message_id IS NULL
                    AND activity_root_trace_boundary_sequence IS NULL)
                OR (activity_root_anchor_message_id IS NOT NULL
                    AND activity_root_trace_boundary_sequence IS NOT NULL)
            ),
            CHECK (
                activity_semantic IS NULL
                OR (activity_semantic = 'started'
                    AND kind = 'wake_created'
                    AND activity_agent_id = agent_id)
                OR (activity_semantic = 'updated'
                    AND kind = 'mailbox_enqueued'
                    AND activity_agent_id != agent_id)
                OR (activity_semantic = 'waiting_approval'
                    AND kind = 'approval_projected'
                    AND activity_agent_id = agent_id)
                OR (activity_semantic IN ('completed', 'failed', 'interrupted')
                    AND kind = 'wake_updated'
                    AND activity_agent_id = agent_id)
            ),
            FOREIGN KEY (root_agent_id) REFERENCES agent_nodes(agent_id) ON DELETE CASCADE,
            FOREIGN KEY (agent_id) REFERENCES agent_nodes(agent_id) ON DELETE CASCADE,
            FOREIGN KEY (activity_agent_id, root_agent_id)
                REFERENCES agent_nodes(agent_id, root_agent_id) ON DELETE CASCADE,
            FOREIGN KEY (conversation_id) REFERENCES conversations(id) ON DELETE CASCADE
        );
CREATE INDEX agent_collaboration_events_root_sequence
            ON agent_collaboration_events(root_agent_id, root_sequence);
CREATE INDEX agent_collaboration_events_global_sequence
            ON agent_collaboration_events(global_sequence);
CREATE TRIGGER validate_agent_collaboration_event_identity_insert
        BEFORE INSERT ON agent_collaboration_events
        WHEN NOT EXISTS (
            SELECT 1
            FROM agent_nodes AS root
            JOIN agent_nodes AS subject
              ON subject.agent_id = NEW.agent_id
             AND subject.root_agent_id = root.agent_id
             AND subject.root_conversation_id = root.root_conversation_id
             AND subject.conversation_id = NEW.conversation_id
             AND subject.project_id IS NEW.project_id
            WHERE root.agent_id = NEW.root_agent_id
              AND root.parent_agent_id IS NULL
              AND root.root_agent_id = root.agent_id
              AND root.conversation_id = NEW.root_conversation_id
              AND root.root_conversation_id = NEW.root_conversation_id
              AND root.project_id IS NEW.project_id
              AND NEW.workspace_id IS NEW.project_id
        ) OR (
            NEW.activity_agent_id IS NOT NULL
            AND NOT EXISTS (
                SELECT 1 FROM agent_nodes AS activity_subject
                WHERE activity_subject.agent_id = NEW.activity_agent_id
                  AND activity_subject.root_agent_id = NEW.root_agent_id
                  AND activity_subject.root_conversation_id = NEW.root_conversation_id
                  AND activity_subject.parent_agent_id IS NOT NULL
                  AND activity_subject.task_name = NEW.activity_task_name_snapshot
            )
        ) OR (
            NEW.activity_semantic IS NOT NULL
            AND NOT (
                (
                    NEW.activity_root_anchor_message_id IS NULL
                    AND NOT EXISTS (
                        SELECT 1 FROM conversation_turn_traces AS active_root_trace
                        WHERE active_root_trace.conversation_id = NEW.root_conversation_id
                          AND active_root_trace.terminal_status = 'in_progress'
                    )
                ) OR (
                    NEW.activity_root_anchor_message_id IS NOT NULL
                    AND EXISTS (
                        SELECT 1
                        FROM messages AS anchor
                        JOIN conversation_turn_traces AS root_trace
                          ON root_trace.assistant_message_id = anchor.id
                         AND root_trace.conversation_id = NEW.root_conversation_id
                         AND root_trace.terminal_status = 'in_progress'
                        WHERE anchor.id = NEW.activity_root_anchor_message_id
                          AND anchor.conversation_id = NEW.root_conversation_id
                          AND anchor.role = 'assistant'
                          AND NEW.activity_root_trace_boundary_sequence = COALESCE((
                              SELECT MAX(trace_item.sequence) + 1
                              FROM conversation_turn_trace_items AS trace_item
                              WHERE trace_item.assistant_message_id = root_trace.assistant_message_id
                          ), 0)
                    )
                )
            )
        )
        BEGIN
            SELECT RAISE(ABORT, 'Collaboration event identity must match one root tree');
        END;
CREATE TRIGGER validate_agent_collaboration_event_sequence_insert
        BEFORE INSERT ON agent_collaboration_events
        WHEN NOT EXISTS (
            SELECT 1 FROM agent_collaboration_event_sequences
            WHERE root_agent_id = NEW.root_agent_id
              AND NEW.root_sequence = next_sequence - 1
        )
        BEGIN
            SELECT RAISE(ABORT, 'Collaboration event sequence must match its durable root cursor');
        END;
CREATE TRIGGER validate_agent_collaboration_event_sequence_update
        BEFORE UPDATE OF next_sequence ON agent_collaboration_event_sequences
        WHEN NEW.next_sequence != OLD.next_sequence + 1
        BEGIN
            SELECT RAISE(ABORT, 'Collaboration event sequence must advance exactly once');
        END;
CREATE TRIGGER prevent_agent_collaboration_event_sequence_delete
        BEFORE DELETE ON agent_collaboration_event_sequences
        WHEN EXISTS (
            SELECT 1 FROM agent_nodes WHERE agent_id = OLD.root_agent_id
        )
        BEGIN
            SELECT RAISE(ABORT, 'Collaboration event sequence is durable while its root exists');
        END;
CREATE TRIGGER prevent_agent_collaboration_event_update
        BEFORE UPDATE ON agent_collaboration_events
        BEGIN
            SELECT RAISE(ABORT, 'Collaboration events are immutable');
        END;
CREATE TRIGGER prevent_agent_collaboration_event_delete
        BEFORE DELETE ON agent_collaboration_events
        WHEN EXISTS (
            SELECT 1 FROM agent_nodes WHERE agent_id = OLD.root_agent_id
        )
        BEGIN
            SELECT RAISE(ABORT, 'Collaboration events are durable while their root exists');
        END;

CREATE TRIGGER emit_agent_created_collaboration_event
        AFTER INSERT ON agent_nodes
        BEGIN
            INSERT INTO agent_collaboration_event_sequences (root_agent_id, next_sequence)
            VALUES (NEW.root_agent_id, 2)
            ON CONFLICT(root_agent_id) DO UPDATE
            SET next_sequence = next_sequence + 1;
            INSERT INTO agent_collaboration_events (
                event_id, schema_version, root_agent_id, root_sequence, workspace_id, project_id,
                root_conversation_id, agent_id, conversation_id, turn_id, run_id, message_id,
                kind, resource_revision, created_at
            ) VALUES (
                'collab-event:' || lower(hex(randomblob(16))), 2, NEW.root_agent_id,
                (SELECT next_sequence - 1 FROM agent_collaboration_event_sequences
                 WHERE root_agent_id = NEW.root_agent_id),
                NEW.project_id, NEW.project_id, NEW.root_conversation_id, NEW.agent_id,
                NEW.conversation_id, NULL, NULL, NULL, 'agent_created', NEW.revision, NEW.created_at
            );
        END;
CREATE TRIGGER emit_agent_updated_collaboration_event
        AFTER UPDATE OF lifecycle, revision ON agent_nodes
        WHEN NEW.lifecycle IS NOT OLD.lifecycle OR NEW.revision IS NOT OLD.revision
        BEGIN
            UPDATE agent_collaboration_event_sequences
            SET next_sequence = next_sequence + 1 WHERE root_agent_id = NEW.root_agent_id;
            INSERT INTO agent_collaboration_events (
                event_id, schema_version, root_agent_id, root_sequence, workspace_id, project_id,
                root_conversation_id, agent_id, conversation_id, turn_id, run_id, message_id,
                kind, resource_revision, created_at
            ) VALUES (
                'collab-event:' || lower(hex(randomblob(16))), 2, NEW.root_agent_id,
                (SELECT next_sequence - 1 FROM agent_collaboration_event_sequences
                 WHERE root_agent_id = NEW.root_agent_id),
                NEW.project_id, NEW.project_id, NEW.root_conversation_id, NEW.agent_id,
                NEW.conversation_id, NULL, NULL, NULL, 'agent_updated', NEW.revision, NEW.updated_at
            );
        END;
CREATE TRIGGER emit_root_conversation_model_updated_collaboration_event
        AFTER UPDATE OF model_id ON conversations
        WHEN NEW.model_id IS NOT OLD.model_id
          AND EXISTS (
              SELECT 1 FROM agent_nodes
              WHERE conversation_id = NEW.id AND parent_agent_id IS NULL
          )
        BEGIN
            UPDATE agent_collaboration_event_sequences
            SET next_sequence = next_sequence + 1
            WHERE root_agent_id = (
                SELECT agent_id FROM agent_nodes
                WHERE conversation_id = NEW.id AND parent_agent_id IS NULL
            );
            INSERT INTO agent_collaboration_events (
                event_id, schema_version, root_agent_id, root_sequence, workspace_id, project_id,
                root_conversation_id, agent_id, conversation_id, turn_id, run_id, message_id,
                kind, resource_revision, created_at
            ) SELECT
                'collab-event:' || lower(hex(randomblob(16))), 2, root.agent_id,
                sequence_state.next_sequence - 1, root.project_id, root.project_id,
                root.root_conversation_id, root.agent_id, root.conversation_id,
                NULL, NULL, NULL, 'agent_updated',
                CASE WHEN NEW.revision + 1 > 0 THEN NEW.revision + 1 ELSE 1 END,
                CASE WHEN NEW.updated_at >= 0 THEN NEW.updated_at ELSE 0 END
            FROM agent_nodes AS root
            JOIN agent_collaboration_event_sequences AS sequence_state
              ON sequence_state.root_agent_id = root.agent_id
            WHERE root.conversation_id = NEW.id AND root.parent_agent_id IS NULL;
        END;
CREATE TRIGGER emit_agent_mailbox_enqueued_collaboration_event
        AFTER INSERT ON agent_mailbox_messages
        BEGIN
            UPDATE agent_collaboration_event_sequences
            SET next_sequence = next_sequence + 1 WHERE root_agent_id = NEW.root_agent_id;
            INSERT INTO agent_collaboration_events (
                event_id, schema_version, root_agent_id, root_sequence, workspace_id, project_id,
                root_conversation_id, agent_id, conversation_id, turn_id, run_id, message_id,
                kind, resource_revision, activity_schema_version, activity_semantic,
                activity_agent_id, activity_task_name_snapshot,
                activity_root_anchor_message_id, activity_root_trace_boundary_sequence, created_at
            ) SELECT
                'collab-event:' || lower(hex(randomblob(16))), 2, NEW.root_agent_id,
                sequence_state.next_sequence - 1, recipient.project_id, recipient.project_id,
                recipient.root_conversation_id, recipient.agent_id, recipient.conversation_id,
                NULL, NULL, NEW.message_id, 'mailbox_enqueued', NEW.sequence,
                CASE
                    WHEN NEW.kind = 'message' AND sender.parent_agent_id = recipient.agent_id
                    THEN 2 ELSE NULL
                END,
                CASE
                    WHEN NEW.kind = 'message' AND sender.parent_agent_id = recipient.agent_id
                    THEN 'updated' ELSE NULL
                END,
                CASE
                    WHEN NEW.kind = 'message' AND sender.parent_agent_id = recipient.agent_id
                    THEN sender.agent_id ELSE NULL
                END,
                CASE
                    WHEN NEW.kind = 'message' AND sender.parent_agent_id = recipient.agent_id
                    THEN sender.task_name ELSE NULL
                END,
                CASE
                    WHEN NEW.kind = 'message' AND sender.parent_agent_id = recipient.agent_id
                    THEN root_trace.assistant_message_id ELSE NULL
                END,
                CASE
                    WHEN NEW.kind = 'message' AND sender.parent_agent_id = recipient.agent_id
                     AND root_trace.assistant_message_id IS NOT NULL
                    THEN COALESCE((
                        SELECT MAX(trace_item.sequence) + 1
                        FROM conversation_turn_trace_items AS trace_item
                        WHERE trace_item.assistant_message_id = root_trace.assistant_message_id
                    ), 0)
                    ELSE NULL
                END,
                NEW.created_at
            FROM agent_nodes AS recipient
            JOIN agent_nodes AS sender
              ON sender.agent_id = NEW.sender_agent_id
             AND sender.root_agent_id = NEW.root_agent_id
            LEFT JOIN conversation_turn_traces AS root_trace
              ON root_trace.conversation_id = recipient.root_conversation_id
             AND root_trace.terminal_status = 'in_progress'
            JOIN agent_collaboration_event_sequences AS sequence_state
              ON sequence_state.root_agent_id = NEW.root_agent_id
            WHERE recipient.agent_id = NEW.recipient_agent_id;
        END;
CREATE TRIGGER emit_agent_mailbox_updated_collaboration_event
        AFTER UPDATE OF delivery_status ON agent_mailbox_messages
        WHEN NEW.delivery_status IS NOT OLD.delivery_status
        BEGIN
            UPDATE agent_collaboration_event_sequences
            SET next_sequence = next_sequence + 1 WHERE root_agent_id = NEW.root_agent_id;
            INSERT INTO agent_collaboration_events (
                event_id, schema_version, root_agent_id, root_sequence, workspace_id, project_id,
                root_conversation_id, agent_id, conversation_id, turn_id, run_id, message_id,
                kind, resource_revision, created_at
            ) SELECT
                'collab-event:' || lower(hex(randomblob(16))), 2, NEW.root_agent_id,
                sequence_state.next_sequence - 1, recipient.project_id, recipient.project_id,
                recipient.root_conversation_id, recipient.agent_id, recipient.conversation_id,
                NULL, NULL, NEW.message_id, 'mailbox_updated', NEW.sequence,
                COALESCE(NEW.acknowledged_at, NEW.claimed_at, NEW.created_at)
            FROM agent_nodes AS recipient
            JOIN agent_collaboration_event_sequences AS sequence_state
              ON sequence_state.root_agent_id = NEW.root_agent_id
            WHERE recipient.agent_id = NEW.recipient_agent_id;
        END;
CREATE TRIGGER emit_agent_wake_created_collaboration_event
        AFTER INSERT ON agent_wake_requests
        BEGIN
            UPDATE agent_collaboration_event_sequences
            SET next_sequence = next_sequence + 1 WHERE root_agent_id = NEW.root_agent_id;
            INSERT INTO agent_collaboration_events (
                event_id, schema_version, root_agent_id, root_sequence, workspace_id, project_id,
                root_conversation_id, agent_id, conversation_id, turn_id, run_id, message_id,
                kind, resource_revision, activity_schema_version, activity_semantic,
                activity_agent_id, activity_task_name_snapshot,
                activity_root_anchor_message_id, activity_root_trace_boundary_sequence, created_at
            ) SELECT
                'collab-event:' || lower(hex(randomblob(16))), 2, NEW.root_agent_id,
                sequence_state.next_sequence - 1, target.project_id, target.project_id,
                target.root_conversation_id, target.agent_id, target.conversation_id,
                NEW.assistant_message_id, NEW.run_id, NEW.source_agent_message_id,
                'wake_created', NEW.status_revision,
                CASE WHEN source.kind IN ('task', 'followup') THEN 2 ELSE NULL END,
                CASE WHEN source.kind IN ('task', 'followup') THEN 'started' ELSE NULL END,
                CASE WHEN source.kind IN ('task', 'followup') THEN target.agent_id ELSE NULL END,
                CASE WHEN source.kind IN ('task', 'followup') THEN target.task_name ELSE NULL END,
                CASE WHEN source.kind IN ('task', 'followup')
                     THEN root_trace.assistant_message_id ELSE NULL END,
                CASE
                    WHEN source.kind IN ('task', 'followup')
                     AND root_trace.assistant_message_id IS NOT NULL
                    THEN COALESCE((
                        SELECT MAX(trace_item.sequence) + 1
                        FROM conversation_turn_trace_items AS trace_item
                        WHERE trace_item.assistant_message_id = root_trace.assistant_message_id
                    ), 0)
                    ELSE NULL
                END,
                NEW.created_at
            FROM agent_nodes AS target
            LEFT JOIN agent_mailbox_messages AS source
              ON source.message_id = NEW.source_agent_message_id
             AND source.root_agent_id = NEW.root_agent_id
             AND source.recipient_agent_id = NEW.agent_id
            LEFT JOIN conversation_turn_traces AS root_trace
              ON root_trace.conversation_id = target.root_conversation_id
             AND root_trace.terminal_status = 'in_progress'
            JOIN agent_collaboration_event_sequences AS sequence_state
              ON sequence_state.root_agent_id = NEW.root_agent_id
            WHERE target.agent_id = NEW.agent_id;
        END;
CREATE TRIGGER emit_agent_wake_updated_collaboration_event
        AFTER UPDATE OF status, status_revision, run_id, assistant_message_id ON agent_wake_requests
        WHEN NEW.status IS NOT OLD.status
          OR NEW.status_revision IS NOT OLD.status_revision
          OR NEW.run_id IS NOT OLD.run_id
          OR NEW.assistant_message_id IS NOT OLD.assistant_message_id
        BEGIN
            UPDATE agent_collaboration_event_sequences
            SET next_sequence = next_sequence + 1 WHERE root_agent_id = NEW.root_agent_id;
            INSERT INTO agent_collaboration_events (
                event_id, schema_version, root_agent_id, root_sequence, workspace_id, project_id,
                root_conversation_id, agent_id, conversation_id, turn_id, run_id, message_id,
                kind, resource_revision, activity_schema_version, activity_semantic,
                activity_agent_id, activity_task_name_snapshot,
                activity_root_anchor_message_id, activity_root_trace_boundary_sequence, created_at
            ) SELECT
                'collab-event:' || lower(hex(randomblob(16))), 2, NEW.root_agent_id,
                sequence_state.next_sequence - 1, target.project_id, target.project_id,
                target.root_conversation_id, target.agent_id, target.conversation_id,
                NEW.assistant_message_id, NEW.run_id, NEW.source_agent_message_id,
                'wake_updated', NEW.status_revision,
                CASE
                    WHEN NEW.status IS NOT OLD.status
                     AND (
                        NEW.status IN ('interrupted', 'cancelled')
                        OR (NEW.status IN ('completed', 'failed')
                            AND NEW.result_message_id IS NOT NULL)
                     )
                    THEN 2 ELSE NULL
                END,
                CASE
                    WHEN NEW.status IS NOT OLD.status AND NEW.status = 'completed'
                     AND NEW.result_message_id IS NOT NULL
                    THEN 'completed'
                    WHEN NEW.status IS NOT OLD.status AND NEW.status = 'failed'
                     AND NEW.result_message_id IS NOT NULL
                    THEN 'failed'
                    WHEN NEW.status IS NOT OLD.status
                     AND NEW.status IN ('interrupted', 'cancelled')
                    THEN 'interrupted'
                    ELSE NULL
                END,
                CASE
                    WHEN NEW.status IS NOT OLD.status
                     AND (
                        NEW.status IN ('interrupted', 'cancelled')
                        OR (NEW.status IN ('completed', 'failed')
                            AND NEW.result_message_id IS NOT NULL)
                     )
                    THEN target.agent_id ELSE NULL
                END,
                CASE
                    WHEN NEW.status IS NOT OLD.status
                     AND (
                        NEW.status IN ('interrupted', 'cancelled')
                        OR (NEW.status IN ('completed', 'failed')
                            AND NEW.result_message_id IS NOT NULL)
                     )
                    THEN target.task_name ELSE NULL
                END,
                CASE
                    WHEN NEW.status IS NOT OLD.status
                     AND (
                        NEW.status IN ('interrupted', 'cancelled')
                        OR (NEW.status IN ('completed', 'failed')
                            AND NEW.result_message_id IS NOT NULL)
                     )
                    THEN root_trace.assistant_message_id ELSE NULL
                END,
                CASE
                    WHEN NEW.status IS NOT OLD.status
                     AND (
                        NEW.status IN ('interrupted', 'cancelled')
                        OR (NEW.status IN ('completed', 'failed')
                            AND NEW.result_message_id IS NOT NULL)
                     )
                     AND root_trace.assistant_message_id IS NOT NULL
                    THEN COALESCE((
                        SELECT MAX(trace_item.sequence) + 1
                        FROM conversation_turn_trace_items AS trace_item
                        WHERE trace_item.assistant_message_id = root_trace.assistant_message_id
                    ), 0)
                    ELSE NULL
                END,
                COALESCE(NEW.completed_at, NEW.started_at, NEW.claimed_at, NEW.created_at)
            FROM agent_nodes AS target
            LEFT JOIN conversation_turn_traces AS root_trace
              ON root_trace.conversation_id = target.root_conversation_id
             AND root_trace.terminal_status = 'in_progress'
            JOIN agent_collaboration_event_sequences AS sequence_state
              ON sequence_state.root_agent_id = NEW.root_agent_id
            WHERE target.agent_id = NEW.agent_id;
        END;
CREATE TRIGGER emit_agent_turn_started_collaboration_event
        AFTER INSERT ON conversation_turn_traces
        WHEN EXISTS (SELECT 1 FROM agent_nodes WHERE conversation_id = NEW.conversation_id)
        BEGIN
            UPDATE agent_collaboration_event_sequences
            SET next_sequence = next_sequence + 1
            WHERE root_agent_id = (
                SELECT root_agent_id FROM agent_nodes WHERE conversation_id = NEW.conversation_id
            );
            INSERT INTO agent_collaboration_events (
                event_id, schema_version, root_agent_id, root_sequence, workspace_id, project_id,
                root_conversation_id, agent_id, conversation_id, turn_id, run_id, message_id,
                kind, resource_revision, created_at
            ) SELECT
                'collab-event:' || lower(hex(randomblob(16))), 2, node.root_agent_id,
                sequence_state.next_sequence - 1, node.project_id, node.project_id,
                node.root_conversation_id, node.agent_id, node.conversation_id,
                NEW.assistant_message_id, NEW.run_id, NEW.assistant_message_id,
                'turn_started', CASE WHEN NEW.updated_at > 0 THEN NEW.updated_at ELSE 1 END,
                NEW.created_at
            FROM agent_nodes AS node
            JOIN agent_collaboration_event_sequences AS sequence_state
              ON sequence_state.root_agent_id = node.root_agent_id
            WHERE node.conversation_id = NEW.conversation_id;
        END;
CREATE TRIGGER emit_agent_turn_updated_collaboration_event
        AFTER UPDATE OF terminal_status, updated_at ON conversation_turn_traces
        WHEN (NEW.terminal_status IS NOT OLD.terminal_status OR NEW.updated_at IS NOT OLD.updated_at)
          AND EXISTS (SELECT 1 FROM agent_nodes WHERE conversation_id = NEW.conversation_id)
        BEGIN
            UPDATE agent_collaboration_event_sequences
            SET next_sequence = next_sequence + 1
            WHERE root_agent_id = (
                SELECT root_agent_id FROM agent_nodes WHERE conversation_id = NEW.conversation_id
            );
            INSERT INTO agent_collaboration_events (
                event_id, schema_version, root_agent_id, root_sequence, workspace_id, project_id,
                root_conversation_id, agent_id, conversation_id, turn_id, run_id, message_id,
                kind, resource_revision, created_at
            ) SELECT
                'collab-event:' || lower(hex(randomblob(16))), 2, node.root_agent_id,
                sequence_state.next_sequence - 1, node.project_id, node.project_id,
                node.root_conversation_id, node.agent_id, node.conversation_id,
                NEW.assistant_message_id, NEW.run_id, NEW.assistant_message_id,
                'turn_updated', CASE WHEN NEW.updated_at > 0 THEN NEW.updated_at ELSE 1 END,
                NEW.updated_at
            FROM agent_nodes AS node
            JOIN agent_collaboration_event_sequences AS sequence_state
              ON sequence_state.root_agent_id = node.root_agent_id
            WHERE node.conversation_id = NEW.conversation_id;
        END;
CREATE TRIGGER emit_agent_approval_projected_collaboration_event
        AFTER INSERT ON agent_pending_actions
        WHEN NEW.status = 'pending'
          AND EXISTS (
            SELECT 1 FROM agent_nodes
            WHERE conversation_id = NEW.conversation_id AND parent_agent_id IS NOT NULL
        )
        BEGIN
            UPDATE agent_collaboration_event_sequences
            SET next_sequence = next_sequence + 1
            WHERE root_agent_id = (
                SELECT root_agent_id FROM agent_nodes WHERE conversation_id = NEW.conversation_id
            );
            INSERT INTO agent_collaboration_events (
                event_id, schema_version, root_agent_id, root_sequence, workspace_id, project_id,
                root_conversation_id, agent_id, conversation_id, turn_id, run_id, message_id,
                kind, resource_revision, activity_schema_version, activity_semantic,
                activity_agent_id, activity_task_name_snapshot,
                activity_root_anchor_message_id, activity_root_trace_boundary_sequence, created_at
            ) SELECT
                'collab-event:' || lower(hex(randomblob(16))), 2, node.root_agent_id,
                sequence_state.next_sequence - 1, node.project_id, node.project_id,
                node.root_conversation_id, node.agent_id, node.conversation_id,
                NEW.assistant_message_id, NEW.run_id, NEW.assistant_message_id,
                'approval_projected', CASE WHEN NEW.updated_at > 0 THEN NEW.updated_at ELSE 1 END,
                2, 'waiting_approval', node.agent_id, node.task_name,
                root_trace.assistant_message_id,
                CASE
                    WHEN root_trace.assistant_message_id IS NOT NULL
                    THEN COALESCE((
                        SELECT MAX(trace_item.sequence) + 1
                        FROM conversation_turn_trace_items AS trace_item
                        WHERE trace_item.assistant_message_id = root_trace.assistant_message_id
                    ), 0)
                    ELSE NULL
                END,
                NEW.created_at
            FROM agent_nodes AS node
            LEFT JOIN conversation_turn_traces AS root_trace
              ON root_trace.conversation_id = node.root_conversation_id
             AND root_trace.terminal_status = 'in_progress'
            JOIN agent_collaboration_event_sequences AS sequence_state
              ON sequence_state.root_agent_id = node.root_agent_id
            WHERE node.conversation_id = NEW.conversation_id;
        END;
CREATE TRIGGER emit_agent_approval_updated_collaboration_event
        AFTER UPDATE OF status, target_status, updated_at ON agent_pending_actions
        WHEN (NEW.status IS NOT OLD.status OR NEW.target_status IS NOT OLD.target_status
              OR NEW.updated_at IS NOT OLD.updated_at)
          AND EXISTS (SELECT 1 FROM agent_nodes WHERE conversation_id = NEW.conversation_id)
          AND EXISTS (
              SELECT 1 FROM agent_nodes
              WHERE conversation_id = NEW.conversation_id AND parent_agent_id IS NOT NULL
          )
        BEGIN
            UPDATE agent_collaboration_event_sequences
            SET next_sequence = next_sequence + 1
            WHERE root_agent_id = (
                SELECT root_agent_id FROM agent_nodes WHERE conversation_id = NEW.conversation_id
            );
            INSERT INTO agent_collaboration_events (
                event_id, schema_version, root_agent_id, root_sequence, workspace_id, project_id,
                root_conversation_id, agent_id, conversation_id, turn_id, run_id, message_id,
                kind, resource_revision, created_at
            ) SELECT
                'collab-event:' || lower(hex(randomblob(16))), 2, node.root_agent_id,
                sequence_state.next_sequence - 1, node.project_id, node.project_id,
                node.root_conversation_id, node.agent_id, node.conversation_id,
                NEW.assistant_message_id, NEW.run_id, NEW.assistant_message_id,
                'approval_updated', CASE WHEN NEW.updated_at > 0 THEN NEW.updated_at ELSE 1 END,
                NEW.updated_at
            FROM agent_nodes AS node
            JOIN agent_collaboration_event_sequences AS sequence_state
              ON sequence_state.root_agent_id = node.root_agent_id
            WHERE node.conversation_id = NEW.conversation_id;
        END;
