CREATE TABLE manual_context_compaction_operations (
            operation_id TEXT PRIMARY KEY,
            request_id TEXT NOT NULL UNIQUE,
            conversation_id TEXT NOT NULL,
            status TEXT NOT NULL CHECK (status IN ('running', 'completed', 'noop', 'cancelled', 'failed', 'interrupted')),
            phase TEXT NOT NULL CHECK (phase IN ('preparing', 'generating', 'committing')),
            summary_id TEXT,
            operation_json TEXT NOT NULL CHECK (json_valid(operation_json)),
            started_at INTEGER NOT NULL,
            updated_at INTEGER NOT NULL,
            FOREIGN KEY (conversation_id) REFERENCES conversations(id) ON DELETE CASCADE
        );
CREATE UNIQUE INDEX idx_manual_context_compaction_active
            ON manual_context_compaction_operations(conversation_id) WHERE status = 'running';
CREATE INDEX idx_manual_context_compaction_history
            ON manual_context_compaction_operations(conversation_id, started_at, operation_id);
CREATE TABLE manual_context_compaction_usage_records (
            operation_id TEXT PRIMARY KEY,
            conversation_id TEXT NOT NULL,
            model_id TEXT NOT NULL,
            model_name TEXT NOT NULL,
            created_at INTEGER NOT NULL,
            input_tokens INTEGER CHECK (input_tokens IS NULL OR input_tokens >= 0),
            output_tokens INTEGER CHECK (output_tokens IS NULL OR output_tokens >= 0),
            output_thinking_tokens INTEGER CHECK (output_thinking_tokens IS NULL OR output_thinking_tokens >= 0),
            total_tokens INTEGER CHECK (total_tokens IS NULL OR total_tokens >= 0),
            cached_input_tokens INTEGER CHECK (cached_input_tokens IS NULL OR cached_input_tokens >= 0),
            cache_creation_input_tokens INTEGER CHECK (cache_creation_input_tokens IS NULL OR cache_creation_input_tokens >= 0),
            billable_request_count INTEGER NOT NULL CHECK (billable_request_count >= 0),
            estimated_cost REAL,
            record_json TEXT NOT NULL CHECK (json_valid(record_json)),
            cleared_at INTEGER,
            FOREIGN KEY (operation_id) REFERENCES manual_context_compaction_operations(operation_id) ON DELETE CASCADE,
            FOREIGN KEY (conversation_id) REFERENCES conversations(id) ON DELETE CASCADE
        );
CREATE INDEX idx_manual_context_compaction_usage_created_at
            ON manual_context_compaction_usage_records(created_at, model_id);
