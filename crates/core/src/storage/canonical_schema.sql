CREATE TABLE mcp_registry_metadata (
            singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
            schema_version INTEGER NOT NULL CHECK (schema_version = 1),
            revision_watermark INTEGER NOT NULL CHECK (revision_watermark >= 0),
            updated_at INTEGER NOT NULL CHECK (updated_at >= 0)
        );
CREATE TABLE mcp_registry_servers (
            schema_version INTEGER NOT NULL CHECK (schema_version = 1),
            server_id TEXT PRIMARY KEY,
            display_name TEXT NOT NULL,
            scope_kind TEXT NOT NULL CHECK (scope_kind = 'user'),
            source_kind TEXT NOT NULL CHECK (source_kind = 'user_manual'),
            transport_kind TEXT NOT NULL CHECK (transport_kind = 'stdio'),
            executable TEXT NOT NULL,
            arguments_json TEXT NOT NULL CHECK (
                json_valid(arguments_json) AND json_type(arguments_json) = 'array'
            ),
            cwd TEXT NOT NULL,
            enabled INTEGER NOT NULL CHECK (enabled IN (0, 1)),
            trust TEXT NOT NULL CHECK (trust IN ('untrusted', 'user_approved')),
            approval_mode TEXT NOT NULL CHECK (
                approval_mode IN ('prompt', 'auto', 'deny')
            ),
            connect_timeout_ms INTEGER NOT NULL CHECK (
                connect_timeout_ms BETWEEN 1 AND 10000
            ),
            request_timeout_ms INTEGER NOT NULL CHECK (
                request_timeout_ms BETWEEN 1 AND 300000
            ),
            shutdown_timeout_ms INTEGER NOT NULL CHECK (
                shutdown_timeout_ms BETWEEN 1 AND 2000
            ),
            config_digest TEXT NOT NULL,
            config_epoch TEXT NOT NULL,
            registry_revision INTEGER NOT NULL CHECK (registry_revision > 0),
            launch_spec_digest TEXT NOT NULL,
            authorized_launch_spec_digest TEXT,
            authorized_config_epoch TEXT,
            authorized_config_digest TEXT,
            authorization_format_version INTEGER,
            authorization_policy_version INTEGER,
            authorized_at INTEGER,
            record_state TEXT NOT NULL DEFAULT 'active' CHECK (
                record_state IN ('active', 'invalid')
            ),
            safe_error_code TEXT,
            created_at INTEGER NOT NULL CHECK (created_at >= 0),
            updated_at INTEGER NOT NULL CHECK (updated_at >= created_at),
            CHECK (
                (
                    authorized_launch_spec_digest IS NULL
                    AND authorized_config_epoch IS NULL
                    AND authorized_config_digest IS NULL
                    AND authorization_format_version IS NULL
                    AND authorization_policy_version IS NULL
                    AND authorized_at IS NULL
                ) OR (
                    authorized_launch_spec_digest IS NOT NULL
                    AND authorized_config_epoch IS NOT NULL
                    AND authorized_config_digest IS NOT NULL
                    AND authorization_format_version > 0
                    AND authorization_policy_version > 0
                    AND authorized_at >= 0
                )
            )
        );
CREATE INDEX mcp_registry_servers_revision
            ON mcp_registry_servers(registry_revision, server_id);
CREATE TABLE mcp_registry_model_namespaces (
            schema_version INTEGER NOT NULL CHECK (schema_version = 1),
            server_id TEXT PRIMARY KEY,
            model_namespace TEXT NOT NULL UNIQUE,
            created_at INTEGER NOT NULL CHECK (created_at >= 0)
        );
CREATE TABLE model_provider_settings (
            id TEXT PRIMARY KEY CHECK (id = 'default'),
            api_url TEXT NOT NULL,
            api_token TEXT NOT NULL,
            search_mode TEXT NOT NULL,
            tavily_api_key TEXT NOT NULL,
            configuration_revision TEXT NOT NULL CHECK (
                configuration_revision GLOB 'model-settings-v1:?*'
            ),
            search_connection_revision TEXT NOT NULL CHECK (
                search_connection_revision GLOB 'search-connection-v1:?*'
            ),
            updated_at INTEGER NOT NULL
        );
CREATE TABLE image_generation_profiles (
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
CREATE TABLE image_generation_credential_staging (
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
CREATE TABLE image_generation_credential_cleanup (
            credential_ref TEXT PRIMARY KEY CHECK (
                typeof(credential_ref) = 'text'
                AND length(CAST(credential_ref AS BLOB)) BETWEEN 1 AND 1024
            ),
            created_at INTEGER NOT NULL CHECK (created_at >= 0)
        );
CREATE TABLE image_generation_executions (
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
                json_valid(safe_request_json)
                AND
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
                OR (
                    json_valid(terminal_result_json)
                    AND length(CAST(terminal_result_json AS BLOB)) BETWEEN 2 AND 65536
                )
            ),
            created_at INTEGER NOT NULL CHECK (created_at >= 0),
            updated_at INTEGER NOT NULL CHECK (updated_at >= created_at),
            completed_at INTEGER CHECK (
                (status IN ('executing', 'publishing') AND completed_at IS NULL AND terminal_result_json IS NULL)
                OR
                (status NOT IN ('executing', 'publishing') AND completed_at IS NOT NULL AND terminal_result_json IS NOT NULL)
            )
        );
CREATE INDEX image_generation_executions_status_idx
            ON image_generation_executions (status, updated_at, execution_id);
CREATE TABLE image_generation_artifacts (
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
CREATE INDEX image_generation_artifacts_identity_idx
            ON image_generation_artifacts (artifact_id, state);
CREATE TABLE managed_artifacts (
            artifact_id TEXT PRIMARY KEY CHECK (
                length(artifact_id) = 71
                AND substr(artifact_id, 1, 7) = 'sha256:'
                AND substr(artifact_id, 8) NOT GLOB '*[^0-9a-f]*'
            ),
            schema_version INTEGER NOT NULL CHECK (schema_version = 1),
            kind TEXT NOT NULL CHECK (kind IN ('image', 'document')),
            storage_relative_path TEXT NOT NULL CHECK (
                length(CAST(storage_relative_path AS BLOB)) BETWEEN 1 AND 1024
            ),
            format TEXT NOT NULL CHECK (format IN ('png', 'jpeg', 'webp', 'pdf')),
            media_type TEXT NOT NULL CHECK (
                media_type IN ('image/png', 'image/jpeg', 'image/webp', 'application/pdf')
            ),
            size_bytes INTEGER NOT NULL CHECK (size_bytes > 0),
            sha256 TEXT NOT NULL CHECK (
                length(sha256) = 64
                AND sha256 NOT GLOB '*[^0-9a-f]*'
            ),
            width INTEGER CHECK (width IS NULL OR width BETWEEN 1 AND 16384),
            height INTEGER CHECK (height IS NULL OR height BETWEEN 1 AND 16384),
            created_at INTEGER NOT NULL CHECK (created_at >= 0),
            CHECK (
                (kind = 'image' AND format IN ('png', 'jpeg', 'webp')
                    AND width IS NOT NULL AND height IS NOT NULL)
                OR
                (kind = 'document' AND format = 'pdf'
                    AND width IS NULL AND height IS NULL)
            )
        );
CREATE TABLE managed_artifact_grants (
            artifact_id TEXT NOT NULL,
            conversation_id TEXT NOT NULL,
            run_id TEXT NOT NULL,
            call_id TEXT NOT NULL,
            created_at INTEGER NOT NULL CHECK (created_at >= 0),
            PRIMARY KEY (artifact_id, conversation_id, run_id, call_id),
            FOREIGN KEY (artifact_id) REFERENCES managed_artifacts(artifact_id)
                ON DELETE CASCADE,
            FOREIGN KEY (conversation_id) REFERENCES conversations(id)
                ON DELETE CASCADE
        );
CREATE INDEX managed_artifact_grants_conversation_idx
            ON managed_artifact_grants (conversation_id, artifact_id);
CREATE TABLE models (
            id TEXT PRIMARY KEY,
            display_name TEXT NOT NULL,
            api_url_override TEXT,
            api_token_override TEXT,
            supports_image INTEGER NOT NULL,
            context_window_tokens INTEGER,
            provider_profile_config_json TEXT NOT NULL CHECK (
                json_valid(provider_profile_config_json)
            ),
            provider_connection_revision TEXT NOT NULL CHECK (
                provider_connection_revision GLOB 'provider-connection-v1:?*'
            ),
            provider_protocol_revision TEXT NOT NULL CHECK (
                provider_protocol_revision GLOB 'provider-protocol-v1:?*'
            ),
            input_price TEXT NOT NULL,
            cached_input_price TEXT NOT NULL DEFAULT '',
            output_price TEXT NOT NULL,
            enabled INTEGER NOT NULL,
            position INTEGER NOT NULL,
            created_at INTEGER NOT NULL,
            updated_at INTEGER NOT NULL
        );
CREATE TABLE projects (
            id TEXT PRIMARY KEY,
            name TEXT NOT NULL,
            path TEXT,
            created_at INTEGER NOT NULL,
            pinned_at INTEGER,
            updated_at INTEGER NOT NULL
        );
CREATE TABLE agent_templates (
            template_id TEXT PRIMARY KEY CHECK (
                typeof(template_id) = 'text'
                AND length(CAST(template_id AS BLOB)) BETWEEN 1 AND 128
            ),
            schema_version INTEGER NOT NULL CHECK (schema_version = 1),
            project_id TEXT NOT NULL,
            machine_key TEXT NOT NULL CHECK (
                length(CAST(machine_key AS BLOB)) BETWEEN 1 AND 64
                AND machine_key = lower(machine_key)
                AND substr(machine_key, 1, 1) GLOB '[a-z]'
                AND machine_key NOT GLOB '*[^a-z0-9_-]*'
            ),
            name TEXT NOT NULL CHECK (
                length(CAST(name AS BLOB)) BETWEEN 1 AND 256
                AND name = trim(name)
            ),
            description TEXT NOT NULL CHECK (
                length(CAST(description AS BLOB)) <= 4096
            ),
            instructions TEXT NOT NULL CHECK (
                length(CAST(instructions AS BLOB)) BETWEEN 1 AND 65536
                AND instructions = trim(instructions)
            ),
            model_config_id TEXT NOT NULL CHECK (
                length(CAST(model_config_id AS BLOB)) BETWEEN 1 AND 512
                AND model_config_id = trim(model_config_id)
            ),
            enabled INTEGER NOT NULL CHECK (enabled IN (0, 1)),
            revision INTEGER NOT NULL CHECK (revision > 0),
            created_at INTEGER NOT NULL CHECK (created_at >= 0),
            updated_at INTEGER NOT NULL CHECK (updated_at >= created_at),
            UNIQUE(project_id, machine_key),
            UNIQUE(project_id, name),
            FOREIGN KEY (project_id) REFERENCES projects(id) ON DELETE CASCADE
        );
CREATE TRIGGER prevent_agent_template_identity_update
        BEFORE UPDATE OF template_id, project_id, machine_key, schema_version, created_at
        ON agent_templates
        BEGIN
            SELECT RAISE(ABORT, 'Agent template identity is immutable');
        END;
CREATE TRIGGER validate_agent_template_revision_update
        BEFORE UPDATE ON agent_templates
        WHEN NEW.revision != OLD.revision + 1 OR NEW.updated_at <= OLD.updated_at
        BEGIN
            SELECT RAISE(ABORT, 'invalid Agent template revision transition');
        END;
CREATE TABLE conversations (
            id TEXT PRIMARY KEY,
            project_id TEXT,
            model_id TEXT,
            title TEXT NOT NULL,
            created_at INTEGER NOT NULL,
            updated_at INTEGER NOT NULL,
            pinned_at INTEGER,
            archived_at INTEGER,
            unread_at INTEGER,
            revision INTEGER NOT NULL DEFAULT 0 CHECK (revision >= 0)
        );
CREATE TRIGGER conversations_revision_after_business_update
        AFTER UPDATE OF
            project_id, model_id, title, created_at, updated_at,
            pinned_at, archived_at, unread_at
        ON conversations
        BEGIN
            UPDATE conversations
            SET revision = revision + 1
            WHERE id = NEW.id;
        END;
CREATE TABLE attachments (
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
CREATE TABLE composer_drafts (
            scope_id TEXT PRIMARY KEY,
            message TEXT NOT NULL,
            permission_mode TEXT NOT NULL,
            permission_mode_version INTEGER NOT NULL CHECK (permission_mode_version >= 0),
            model_id TEXT,
            project_id TEXT,
            attachments_json TEXT NOT NULL CHECK (
                json_valid(attachments_json) AND json_type(attachments_json) = 'array'
            ),
            skills_json TEXT NOT NULL CHECK (
                json_valid(skills_json) AND json_type(skills_json) = 'array'
            ),
            queued_messages_json TEXT NOT NULL CHECK (
                json_valid(queued_messages_json) AND json_type(queued_messages_json) = 'array'
            ),
            updated_at INTEGER NOT NULL
        );
CREATE TABLE skill_enablement_overrides (
            skill_id TEXT PRIMARY KEY
                CHECK (
                    typeof(skill_id) = 'text'
                    AND length(CAST(skill_id AS BLOB)) BETWEEN 1 AND 16384
                ),
            enabled INTEGER NOT NULL CHECK (enabled IN (0, 1)),
            generation INTEGER NOT NULL DEFAULT 0 CHECK (generation >= 0),
            updated_at INTEGER NOT NULL
        );
CREATE TABLE ui_preferences (
            id TEXT PRIMARY KEY CHECK (id = 'default'),
            sidebar_conversation_sort TEXT NOT NULL,
            sidebar_project_sort TEXT NOT NULL,
            sidebar_section_order TEXT NOT NULL,
            sidebar_project_order_json TEXT NOT NULL DEFAULT '[]' CHECK (
                json_valid(sidebar_project_order_json)
            ),
            translucent_sidebar INTEGER NOT NULL DEFAULT 0 CHECK (
                translucent_sidebar IN (0, 1)
            ),
            translucent_sidebar_transparency INTEGER NOT NULL DEFAULT 54 CHECK (
                translucent_sidebar_transparency BETWEEN 50 AND 100
            ),
            native_font_smoothing INTEGER NOT NULL DEFAULT 0 CHECK (
                native_font_smoothing IN (0, 1)
            ),
            show_token_usage_details INTEGER NOT NULL DEFAULT 1 CHECK (
                show_token_usage_details IN (0, 1)
            ),
            show_context_window_usage INTEGER NOT NULL DEFAULT 1 CHECK (
                show_context_window_usage IN (0, 1)
            ),
            profile_display_name TEXT NOT NULL DEFAULT '',
            profile_handle TEXT NOT NULL DEFAULT 'USER',
            profile_avatar_data_url TEXT,
            custom_read_permission TEXT NOT NULL DEFAULT 'workspace_only',
            custom_write_permission TEXT NOT NULL DEFAULT 'workspace_only',
            custom_command_permission TEXT NOT NULL DEFAULT 'require_approval',
            custom_patch_permission TEXT NOT NULL DEFAULT 'require_approval',
            full_permission_enabled INTEGER NOT NULL DEFAULT 1 CHECK (
                full_permission_enabled IN (0, 1)
            ),
            custom_permission_enabled INTEGER NOT NULL DEFAULT 1 CHECK (
                custom_permission_enabled IN (0, 1)
            ),
            updated_at INTEGER NOT NULL
        );
CREATE TABLE agent_prompt_preferences (
            id TEXT PRIMARY KEY CHECK (id = 'default'),
            work_mode TEXT NOT NULL,
            tone TEXT NOT NULL,
            detail_level TEXT NOT NULL,
            custom_instructions TEXT NOT NULL,
            updated_at INTEGER NOT NULL
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
            billable_request_count INTEGER NOT NULL DEFAULT 0
                CHECK (billable_request_count >= 0),
            input_price TEXT,
            cached_input_price TEXT,
            output_price TEXT,
            estimated_cost REAL,
            UNIQUE(conversation_id, message_id),
            FOREIGN KEY (conversation_id) REFERENCES conversations(id) ON DELETE CASCADE,
            FOREIGN KEY (message_id) REFERENCES messages(id) ON DELETE CASCADE
        );
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
CREATE TABLE agent_action_audit (
            action_id TEXT PRIMARY KEY,
            run_id TEXT NOT NULL,
            conversation_id TEXT,
            assistant_message_id TEXT,
            action_type TEXT NOT NULL,
            tool_name TEXT NOT NULL,
            decision TEXT,
            status TEXT NOT NULL,
            action_json TEXT NOT NULL CHECK (json_valid(action_json)),
            patch_result_json TEXT CHECK (
                patch_result_json IS NULL OR json_valid(patch_result_json)
            ),
            command_result_json TEXT CHECK (
                command_result_json IS NULL OR json_valid(command_result_json)
            ),
            tool_result_json TEXT CHECK (
                tool_result_json IS NULL OR json_valid(tool_result_json)
            ),
            effective_permissions_json TEXT CHECK (
                effective_permissions_json IS NULL OR json_valid(effective_permissions_json)
            ),
            path_scope TEXT,
            command_cwd_scope TEXT,
            blocked_reason TEXT,
            decision_source TEXT,
            error TEXT,
            created_at INTEGER NOT NULL,
            decided_at INTEGER,
            completed_at INTEGER
        );
CREATE TABLE agent_pending_actions (
            action_id TEXT PRIMARY KEY,
            run_id TEXT NOT NULL,
            conversation_id TEXT,
            assistant_message_id TEXT,
            action_type TEXT NOT NULL,
            tool_name TEXT NOT NULL,
            tool_call_id TEXT,
            status TEXT NOT NULL,
            target_status TEXT,
            action_json TEXT NOT NULL CHECK (json_valid(action_json)),
            agent_input_json TEXT NOT NULL CHECK (json_valid(agent_input_json)),
            created_at INTEGER NOT NULL,
            updated_at INTEGER NOT NULL
        );
CREATE TABLE mcp_approval_payload_envelopes (
            invocation_id TEXT PRIMARY KEY,
            action_id TEXT NOT NULL UNIQUE,
            envelope_version INTEGER NOT NULL,
            envelope_json TEXT NOT NULL CHECK (json_valid(envelope_json)),
            aad_digest TEXT NOT NULL,
            created_at INTEGER NOT NULL,
            expires_at INTEGER NOT NULL,
            CHECK (envelope_version > 0),
            CHECK (expires_at > created_at)
        );
CREATE TABLE provider_continuations (
            continuation_id TEXT PRIMARY KEY CHECK (
                length(continuation_id) = 61
                AND substr(continuation_id, 1, 25) = 'provider-continuation-v1:'
            ),
            schema_version INTEGER NOT NULL CHECK (schema_version = 1),
            envelope_version INTEGER NOT NULL CHECK (envelope_version = 1),
            conversation_id TEXT NOT NULL,
            assistant_message_id TEXT NOT NULL,
            run_id TEXT NOT NULL CHECK (
                length(CAST(run_id AS BLOB)) BETWEEN 1 AND 2048
            ),
            request_index INTEGER NOT NULL CHECK (request_index >= 0),
            assistant_turn_id TEXT NOT NULL CHECK (
                length(assistant_turn_id) = 68
                AND substr(assistant_turn_id, 1, 4) = 'at1_'
                AND substr(assistant_turn_id, 5) NOT GLOB '*[^0-9a-f]*'
            ),
            assistant_turn_digest TEXT NOT NULL CHECK (
                length(assistant_turn_digest) = 71
                AND substr(assistant_turn_digest, 1, 7) = 'sha256:'
                AND substr(assistant_turn_digest, 8) NOT GLOB '*[^0-9a-f]*'
            ),
            provider_protocol_digest TEXT NOT NULL CHECK (
                length(provider_protocol_digest) = 71
                AND substr(provider_protocol_digest, 1, 7) = 'sha256:'
                AND substr(provider_protocol_digest, 8) NOT GLOB '*[^0-9a-f]*'
            ),
            state TEXT NOT NULL CHECK (state IN ('active', 'superseded', 'released')),
            superseded_by TEXT,
            compression TEXT,
            encryption TEXT,
            payload_digest TEXT,
            nonce BLOB,
            ciphertext BLOB,
            decoded_bytes INTEGER,
            compressed_bytes INTEGER,
            created_at INTEGER NOT NULL CHECK (created_at >= 0),
            updated_at INTEGER NOT NULL CHECK (updated_at >= created_at),
            released_at INTEGER CHECK (released_at IS NULL OR released_at >= created_at),
            activated_at INTEGER CHECK (activated_at IS NULL OR activated_at >= created_at),
            UNIQUE (conversation_id, assistant_message_id, run_id, request_index),
            CHECK (
                (
                    state IN ('active', 'superseded')
                    AND compression = 'zstd_binary_v1'
                    AND encryption = 'chacha20_poly1305_v1'
                    AND payload_digest IS NOT NULL
                    AND length(payload_digest) = 71
                    AND substr(payload_digest, 1, 7) = 'sha256:'
                    AND substr(payload_digest, 8) NOT GLOB '*[^0-9a-f]*'
                    AND nonce IS NOT NULL
                    AND length(nonce) = 12
                    AND ciphertext IS NOT NULL
                    AND length(ciphertext) > 16
                    AND length(ciphertext) <= 2097152
                    AND decoded_bytes BETWEEN 1 AND 8388608
                    AND compressed_bytes BETWEEN 1 AND 2097152
                    AND length(ciphertext) = compressed_bytes + 16
                    AND released_at IS NULL
                ) OR (
                    state = 'released'
                    AND superseded_by IS NULL
                    AND compression = 'zstd_binary_v1'
                    AND encryption = 'chacha20_poly1305_v1'
                    AND payload_digest IS NULL
                    AND nonce IS NULL
                    AND ciphertext IS NULL
                    AND decoded_bytes IS NULL
                    AND compressed_bytes IS NULL
                    AND released_at IS NOT NULL
                )
            ),
            FOREIGN KEY (conversation_id) REFERENCES conversations(id) ON DELETE CASCADE,
            FOREIGN KEY (assistant_message_id) REFERENCES messages(id) ON DELETE CASCADE
        );
CREATE TABLE provider_continuation_tool_calls (
            continuation_id TEXT NOT NULL,
            provider_tool_index INTEGER NOT NULL CHECK (provider_tool_index >= 0),
            runtime_call_id TEXT NOT NULL CHECK (
                length(CAST(runtime_call_id AS BLOB)) BETWEEN 1 AND 2048
            ),
            PRIMARY KEY (continuation_id, provider_tool_index),
            UNIQUE (continuation_id, runtime_call_id),
            FOREIGN KEY (continuation_id)
                REFERENCES provider_continuations(continuation_id) ON DELETE CASCADE
        );
CREATE TABLE agent_file_drafts (
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
CREATE TABLE agent_file_draft_chunks (
            draft_id TEXT NOT NULL,
            chunk_index INTEGER NOT NULL,
            content_hash TEXT NOT NULL,
            byte_count INTEGER NOT NULL,
            created_at INTEGER NOT NULL,
            PRIMARY KEY (draft_id, chunk_index),
            FOREIGN KEY (draft_id) REFERENCES agent_file_drafts(id) ON DELETE CASCADE
        );
CREATE TABLE agent_file_draft_operations (
            draft_id TEXT NOT NULL,
            sequence INTEGER NOT NULL,
            operation TEXT NOT NULL,
            payload_hash TEXT NOT NULL,
            created_at INTEGER NOT NULL,
            PRIMARY KEY (draft_id, sequence),
            FOREIGN KEY (draft_id) REFERENCES agent_file_drafts(id) ON DELETE CASCADE
        );
CREATE TABLE agent_nodes (
            agent_id TEXT PRIMARY KEY CHECK (
                typeof(agent_id) = 'text'
                AND length(CAST(agent_id AS BLOB)) BETWEEN 1 AND 128
            ),
            schema_version INTEGER NOT NULL CHECK (schema_version = 1),
            root_agent_id TEXT NOT NULL CHECK (
                length(CAST(root_agent_id AS BLOB)) BETWEEN 1 AND 128
            ),
            root_conversation_id TEXT NOT NULL CHECK (
                length(CAST(root_conversation_id AS BLOB)) BETWEEN 1 AND 128
            ),
            parent_agent_id TEXT CHECK (
                parent_agent_id IS NULL
                OR length(CAST(parent_agent_id AS BLOB)) BETWEEN 1 AND 128
            ),
            conversation_id TEXT NOT NULL UNIQUE CHECK (
                length(CAST(conversation_id AS BLOB)) BETWEEN 1 AND 128
            ),
            project_id TEXT,
            creation_request_id TEXT NOT NULL CHECK (
                length(CAST(creation_request_id AS BLOB)) BETWEEN 1 AND 256
            ),
            task_name TEXT NOT NULL CHECK (
                length(CAST(task_name AS BLOB)) BETWEEN 1 AND 256
                AND task_name = trim(task_name)
            ),
            task_path TEXT NOT NULL CHECK (
                length(CAST(task_path AS BLOB)) BETWEEN 1 AND 2048
                AND substr(task_path, 1, 1) = '/'
                AND task_path = trim(task_path)
            ),
            template_id_snapshot TEXT,
            template_project_id_snapshot TEXT,
            template_machine_key_snapshot TEXT,
            template_name_snapshot TEXT,
            template_description_snapshot TEXT,
            template_instructions_snapshot TEXT,
            template_revision_snapshot INTEGER,
            template_model_config_id_snapshot TEXT,
            model_config_id_snapshot TEXT,
            model_display_name_snapshot TEXT,
            model_supports_image_snapshot INTEGER CHECK (
                model_supports_image_snapshot IS NULL
                OR model_supports_image_snapshot IN (0, 1)
            ),
            model_context_window_tokens_snapshot INTEGER CHECK (
                model_context_window_tokens_snapshot IS NULL
                OR model_context_window_tokens_snapshot > 0
            ),
            model_settings_revision_snapshot TEXT,
            provider_connection_revision_snapshot TEXT,
            provider_protocol_revision_snapshot TEXT,
            model_selection_source_snapshot TEXT CHECK (
                model_selection_source_snapshot IS NULL
                OR model_selection_source_snapshot IN ('explicit', 'template', 'parent', 'default')
            ),
            reasoning_effort_snapshot TEXT CHECK (
                reasoning_effort_snapshot IS NULL
                OR reasoning_effort_snapshot IN ('high', 'max')
            ),
            lifecycle TEXT NOT NULL CHECK (
                lifecycle IN ('active', 'archived', 'disabled')
            ),
            revision INTEGER NOT NULL CHECK (revision > 0),
            created_at INTEGER NOT NULL CHECK (created_at >= 0),
            updated_at INTEGER NOT NULL CHECK (updated_at >= created_at),
            UNIQUE(agent_id, root_agent_id),
            UNIQUE(agent_id, root_agent_id, root_conversation_id),
            UNIQUE(root_agent_id, creation_request_id),
            UNIQUE(root_agent_id, task_name),
            UNIQUE(root_agent_id, task_path),
            FOREIGN KEY (project_id) REFERENCES projects(id) ON DELETE RESTRICT,
            FOREIGN KEY (root_agent_id) REFERENCES agent_nodes(agent_id) ON DELETE RESTRICT,
            FOREIGN KEY (parent_agent_id, root_agent_id, root_conversation_id)
                REFERENCES agent_nodes(agent_id, root_agent_id, root_conversation_id)
                ON DELETE RESTRICT,
            FOREIGN KEY (root_conversation_id) REFERENCES conversations(id) ON DELETE RESTRICT,
            FOREIGN KEY (conversation_id) REFERENCES conversations(id) ON DELETE RESTRICT,
            CHECK (
                (
                    parent_agent_id IS NULL
                    AND agent_id = root_agent_id
                    AND conversation_id = root_conversation_id
                    AND task_path = '/root'
                    AND model_selection_source_snapshot IS NULL
                    AND reasoning_effort_snapshot IS NULL
                ) OR (
                    parent_agent_id IS NOT NULL
                    AND agent_id != root_agent_id
                    AND conversation_id != root_conversation_id
                    AND model_config_id_snapshot IS NOT NULL
                    AND model_selection_source_snapshot IS NOT NULL
                )
            ),
            CHECK (
                parent_agent_id IS NULL
                OR model_selection_source_snapshot = 'explicit'
                OR (
                    model_selection_source_snapshot = 'template'
                    AND template_id_snapshot IS NOT NULL
                    AND model_config_id_snapshot = template_model_config_id_snapshot
                )
                OR (
                    model_selection_source_snapshot IN ('parent', 'default')
                    AND template_id_snapshot IS NULL
                )
            ),
            CHECK (
                (
                    template_id_snapshot IS NULL
                    AND template_project_id_snapshot IS NULL
                    AND template_machine_key_snapshot IS NULL
                    AND template_name_snapshot IS NULL
                    AND template_description_snapshot IS NULL
                    AND template_instructions_snapshot IS NULL
                    AND template_revision_snapshot IS NULL
                    AND template_model_config_id_snapshot IS NULL
                ) OR (
                    template_id_snapshot IS NOT NULL
                    AND template_project_id_snapshot IS NOT NULL
                    AND template_machine_key_snapshot IS NOT NULL
                    AND template_name_snapshot IS NOT NULL
                    AND template_description_snapshot IS NOT NULL
                    AND template_instructions_snapshot IS NOT NULL
                    AND template_revision_snapshot > 0
                    AND template_model_config_id_snapshot IS NOT NULL
                )
            ),
            CHECK (
                (
                    model_config_id_snapshot IS NULL
                    AND model_display_name_snapshot IS NULL
                    AND model_supports_image_snapshot IS NULL
                    AND model_context_window_tokens_snapshot IS NULL
                    AND model_settings_revision_snapshot IS NULL
                    AND provider_connection_revision_snapshot IS NULL
                    AND provider_protocol_revision_snapshot IS NULL
                ) OR (
                    model_config_id_snapshot IS NOT NULL
                    AND model_display_name_snapshot IS NOT NULL
                    AND model_supports_image_snapshot IS NOT NULL
                    AND model_context_window_tokens_snapshot > 0
                    AND model_settings_revision_snapshot IS NOT NULL
                    AND provider_connection_revision_snapshot IS NOT NULL
                    AND provider_protocol_revision_snapshot IS NOT NULL
                )
            )
        );
CREATE UNIQUE INDEX agent_nodes_root_conversation_identity
            ON agent_nodes(root_conversation_id)
            WHERE parent_agent_id IS NULL;
CREATE INDEX agent_nodes_parent
            ON agent_nodes(root_agent_id, parent_agent_id, created_at, agent_id);
CREATE INDEX agent_nodes_conversation
            ON agent_nodes(conversation_id);
CREATE TRIGGER validate_agent_node_project_insert
        BEFORE INSERT ON agent_nodes
        WHEN (
            (
                SELECT project_id FROM conversations WHERE id = NEW.conversation_id
            ) IS NOT NEW.project_id
            OR NEW.project_id IS NOT (
                SELECT project_id FROM conversations WHERE id = NEW.root_conversation_id
            )
        ) OR (
            NEW.template_id_snapshot IS NOT NULL
            AND NEW.template_project_id_snapshot IS NOT (
                SELECT project_id FROM conversations WHERE id = NEW.root_conversation_id
            )
        )
        BEGIN
            SELECT RAISE(ABORT, 'Agent conversation project must match its root');
        END;
CREATE TRIGGER validate_child_agent_conversation_fresh_insert
        BEFORE INSERT ON agent_nodes
        WHEN NEW.parent_agent_id IS NOT NULL AND (
            EXISTS (
                SELECT 1 FROM messages WHERE conversation_id = NEW.conversation_id
            ) OR EXISTS (
                SELECT 1 FROM conversation_turn_traces
                WHERE conversation_id = NEW.conversation_id
            ) OR EXISTS (
                SELECT 1 FROM agent_pending_actions
                WHERE conversation_id = NEW.conversation_id
            ) OR EXISTS (
                SELECT 1 FROM agent_action_audit
                WHERE conversation_id = NEW.conversation_id
            ) OR EXISTS (
                SELECT 1 FROM agent_command_sessions
                WHERE conversation_id = NEW.conversation_id
            )
        )
        BEGIN
            SELECT RAISE(ABORT, 'Low-level child Agent binding requires a fresh execution-free conversation');
        END;
CREATE TRIGGER validate_child_agent_conversation_model_insert
        BEFORE INSERT ON agent_nodes
        WHEN NEW.parent_agent_id IS NOT NULL
          AND (
              SELECT model_id FROM conversations WHERE id = NEW.conversation_id
          ) IS NOT NEW.model_config_id_snapshot
        BEGIN
            SELECT RAISE(ABORT, 'Child Agent conversation model must match its frozen model snapshot');
        END;
CREATE TRIGGER validate_agent_node_parent_path_insert
        BEFORE INSERT ON agent_nodes
        WHEN NEW.parent_agent_id IS NOT NULL AND NOT EXISTS (
            SELECT 1 FROM agent_nodes AS parent
            WHERE parent.agent_id = NEW.parent_agent_id
              AND parent.root_agent_id = NEW.root_agent_id
              AND parent.lifecycle = 'active'
              AND length(NEW.task_path) > length(parent.task_path) + 1
              AND substr(NEW.task_path, 1, length(parent.task_path) + 1)
                    = parent.task_path || '/'
              AND instr(
                    substr(NEW.task_path, length(parent.task_path) + 2), '/'
                  ) = 0
        )
        BEGIN
            SELECT RAISE(ABORT, 'Agent task path must be one active-parent segment');
        END;
CREATE TRIGGER validate_agent_node_template_snapshot_insert
        BEFORE INSERT ON agent_nodes
        WHEN NEW.template_id_snapshot IS NOT NULL AND NOT EXISTS (
            SELECT 1 FROM agent_templates AS template
            WHERE template.template_id = NEW.template_id_snapshot
              AND template.project_id = NEW.template_project_id_snapshot
              AND template.machine_key = NEW.template_machine_key_snapshot
              AND template.name = NEW.template_name_snapshot
              AND template.description = NEW.template_description_snapshot
              AND template.instructions = NEW.template_instructions_snapshot
              AND template.revision = NEW.template_revision_snapshot
              AND template.model_config_id = NEW.template_model_config_id_snapshot
              AND template.enabled = 1
        )
        BEGIN
            SELECT RAISE(ABORT, 'Agent template snapshot is stale or unavailable');
        END;
CREATE TRIGGER prevent_agent_node_identity_update
        BEFORE UPDATE OF
            agent_id, schema_version, root_agent_id, root_conversation_id, parent_agent_id,
            conversation_id, project_id, creation_request_id, task_name, task_path,
            template_id_snapshot, template_project_id_snapshot, template_machine_key_snapshot,
            template_name_snapshot, template_description_snapshot, template_instructions_snapshot,
            template_revision_snapshot, template_model_config_id_snapshot,
            model_config_id_snapshot, model_display_name_snapshot,
            model_supports_image_snapshot, model_context_window_tokens_snapshot,
            model_settings_revision_snapshot, provider_connection_revision_snapshot,
            provider_protocol_revision_snapshot, model_selection_source_snapshot,
            reasoning_effort_snapshot, created_at
        ON agent_nodes
        BEGIN
            SELECT RAISE(ABORT, 'Agent node identity and creation snapshot are immutable');
        END;
CREATE TRIGGER validate_agent_node_lifecycle_update
        BEFORE UPDATE OF lifecycle, revision, updated_at ON agent_nodes
        WHEN NOT (
            NEW.revision = OLD.revision + 1
            AND NEW.updated_at > OLD.updated_at
            AND (
                NEW.lifecycle = OLD.lifecycle
                OR NEW.lifecycle IN ('active', 'archived', 'disabled')
            )
        )
        BEGIN
            SELECT RAISE(ABORT, 'invalid Agent lifecycle revision transition');
        END;
CREATE TRIGGER prevent_agent_lifecycle_deactivation_with_pending_wake
        BEFORE UPDATE OF lifecycle ON agent_nodes
        WHEN NEW.lifecycle != 'active'
          AND EXISTS (
              SELECT 1 FROM agent_wake_requests
              WHERE (agent_id = OLD.agent_id OR requester_agent_id = OLD.agent_id)
                AND status IN ('queued', 'claimed', 'running', 'waiting_for_approval')
          )
        BEGIN
            SELECT RAISE(ABORT, 'Agent has an unsettled wake request');
        END;
CREATE TRIGGER prevent_agent_lifecycle_deactivation_with_active_turn
        BEFORE UPDATE OF lifecycle ON agent_nodes
        WHEN NEW.lifecycle != 'active'
          AND EXISTS (
              SELECT 1 FROM conversation_turn_traces
              WHERE conversation_id = OLD.conversation_id
                AND terminal_status = 'in_progress'
          )
        BEGIN
            SELECT RAISE(ABORT, 'Agent has an active Conversation Turn');
        END;
CREATE TRIGGER prevent_agent_lifecycle_deactivation_with_pending_mailbox
        BEFORE UPDATE OF lifecycle ON agent_nodes
        WHEN NEW.lifecycle != 'active'
          AND EXISTS (
              SELECT 1 FROM agent_mailbox_messages
              WHERE (sender_agent_id = OLD.agent_id OR recipient_agent_id = OLD.agent_id)
                AND delivery_status != 'acknowledged'
          )
        BEGIN
            SELECT RAISE(ABORT, 'Agent has an unsettled mailbox message');
        END;
CREATE TRIGGER prevent_agent_lifecycle_deactivation_with_active_children
        BEFORE UPDATE OF lifecycle ON agent_nodes
        WHEN NEW.lifecycle != 'active'
          AND EXISTS (
              SELECT 1 FROM agent_nodes
              WHERE parent_agent_id = OLD.agent_id AND lifecycle = 'active'
          )
        BEGIN
            SELECT RAISE(ABORT, 'Agent has an active child');
        END;
CREATE TRIGGER validate_agent_lifecycle_activation_parent
        BEFORE UPDATE OF lifecycle ON agent_nodes
        WHEN NEW.lifecycle = 'active'
          AND OLD.lifecycle != 'active'
          AND OLD.parent_agent_id IS NOT NULL
          AND NOT EXISTS (
              SELECT 1 FROM agent_nodes
              WHERE agent_id = OLD.parent_agent_id
                AND root_agent_id = OLD.root_agent_id
                AND lifecycle = 'active'
          )
        BEGIN
            SELECT RAISE(ABORT, 'Agent parent must be active before child activation');
        END;
CREATE TRIGGER prevent_agent_bound_conversation_project_update
        BEFORE UPDATE OF project_id ON conversations
        WHEN NEW.project_id IS NOT OLD.project_id
          AND EXISTS (
              SELECT 1 FROM agent_nodes
              WHERE conversation_id = OLD.id OR root_conversation_id = OLD.id
          )
        BEGIN
            SELECT RAISE(ABORT, 'Agent-bound conversation project is immutable');
        END;
CREATE TRIGGER prevent_child_agent_conversation_model_update
        BEFORE UPDATE OF model_id ON conversations
        WHEN NEW.model_id IS NOT OLD.model_id
          AND EXISTS (
              SELECT 1 FROM agent_nodes
              WHERE conversation_id = OLD.id
                AND parent_agent_id IS NOT NULL
          )
        BEGIN
            SELECT RAISE(ABORT, 'Child Agent conversation model is immutable');
        END;
CREATE TABLE agent_mailbox_messages (
            sequence INTEGER PRIMARY KEY AUTOINCREMENT,
            message_id TEXT NOT NULL UNIQUE CHECK (
                length(CAST(message_id AS BLOB)) BETWEEN 1 AND 128
            ),
            schema_version INTEGER NOT NULL CHECK (schema_version = 1),
            root_agent_id TEXT NOT NULL,
            sender_agent_id TEXT NOT NULL,
            recipient_agent_id TEXT NOT NULL,
            request_id TEXT NOT NULL CHECK (
                length(CAST(request_id AS BLOB)) BETWEEN 1 AND 256
            ),
            kind TEXT NOT NULL CHECK (kind IN ('task', 'message', 'followup', 'result')),
            content TEXT NOT NULL CHECK (
                length(CAST(content AS BLOB)) BETWEEN 1 AND 1048576
            ),
            projection_message_id TEXT NOT NULL UNIQUE CHECK (
                length(CAST(projection_message_id AS BLOB)) BETWEEN 1 AND 128
            ),
            delivery_status TEXT NOT NULL CHECK (
                delivery_status IN ('queued', 'claimed', 'acknowledged')
            ),
            claim_token TEXT,
            lease_expires_at INTEGER,
            created_at INTEGER NOT NULL CHECK (created_at >= 0),
            claimed_at INTEGER,
            acknowledged_at INTEGER,
            UNIQUE(sender_agent_id, request_id),
            UNIQUE(message_id, root_agent_id, sender_agent_id, recipient_agent_id),
            FOREIGN KEY (sender_agent_id, root_agent_id)
                REFERENCES agent_nodes(agent_id, root_agent_id) ON DELETE RESTRICT,
            FOREIGN KEY (recipient_agent_id, root_agent_id)
                REFERENCES agent_nodes(agent_id, root_agent_id) ON DELETE RESTRICT,
            CHECK (sender_agent_id != recipient_agent_id),
            CHECK (
                (delivery_status = 'queued'
                    AND claim_token IS NULL
                    AND lease_expires_at IS NULL
                    AND claimed_at IS NULL
                    AND acknowledged_at IS NULL)
                OR (delivery_status = 'claimed'
                    AND claim_token IS NOT NULL
                    AND lease_expires_at IS NOT NULL
                    AND lease_expires_at > claimed_at
                    AND claimed_at IS NOT NULL
                    AND acknowledged_at IS NULL)
                OR (delivery_status = 'acknowledged'
                    AND claim_token IS NOT NULL
                    AND lease_expires_at IS NOT NULL
                    AND claimed_at IS NOT NULL
                    AND acknowledged_at IS NOT NULL)
            )
        );
CREATE TRIGGER validate_agent_mailbox_active_participants_insert
        BEFORE INSERT ON agent_mailbox_messages
        WHEN (
            SELECT lifecycle FROM agent_nodes WHERE agent_id = NEW.sender_agent_id
        ) != 'active' OR (
            SELECT lifecycle FROM agent_nodes WHERE agent_id = NEW.recipient_agent_id
        ) != 'active'
        BEGIN
            SELECT RAISE(ABORT, 'Agent mailbox participants must be active');
        END;
CREATE TRIGGER validate_agent_mailbox_unbound_quota_insert
        BEFORE INSERT ON agent_mailbox_messages
        WHEN (
            SELECT COUNT(*)
            FROM agent_mailbox_messages AS mailbox
            WHERE mailbox.recipient_agent_id = NEW.recipient_agent_id
              AND NOT EXISTS (
                  SELECT 1 FROM agent_model_batch_receipt_items AS item
                  WHERE item.message_id = mailbox.message_id
              )
        ) >= 1024 OR (
            SELECT COALESCE(SUM(length(CAST(mailbox.content AS BLOB))), 0)
            FROM agent_mailbox_messages AS mailbox
            WHERE mailbox.recipient_agent_id = NEW.recipient_agent_id
              AND NOT EXISTS (
                  SELECT 1 FROM agent_model_batch_receipt_items AS item
                  WHERE item.message_id = mailbox.message_id
              )
        ) + length(CAST(NEW.content AS BLOB)) > 16777216 OR (
            NEW.kind IN ('message', 'followup')
            AND (
                SELECT COUNT(*)
                FROM agent_mailbox_messages AS mailbox
                WHERE mailbox.recipient_agent_id = NEW.recipient_agent_id
                  AND mailbox.kind IN ('message', 'followup')
                  AND NOT EXISTS (
                      SELECT 1 FROM agent_model_batch_receipt_items AS item
                      WHERE item.message_id = mailbox.message_id
                  )
            ) >= 960
        ) OR (
            NEW.kind IN ('message', 'followup')
            AND (
                SELECT COALESCE(SUM(length(CAST(mailbox.content AS BLOB))), 0)
                FROM agent_mailbox_messages AS mailbox
                WHERE mailbox.recipient_agent_id = NEW.recipient_agent_id
                  AND mailbox.kind IN ('message', 'followup')
                  AND NOT EXISTS (
                      SELECT 1 FROM agent_model_batch_receipt_items AS item
                      WHERE item.message_id = mailbox.message_id
                  )
            ) + length(CAST(NEW.content AS BLOB)) > 15728640
        )
        BEGIN
            SELECT RAISE(ABORT, 'Agent recipient unbound Mailbox quota exceeded');
        END;
CREATE TRIGGER validate_agent_mailbox_kind_authority_insert
        BEFORE INSERT ON agent_mailbox_messages
        WHEN (
            NEW.kind = 'task'
            AND NOT EXISTS (
                SELECT 1 FROM agent_nodes AS recipient
                WHERE recipient.agent_id = NEW.recipient_agent_id
                  AND recipient.parent_agent_id = NEW.sender_agent_id
            )
        ) OR (
            NEW.kind = 'followup'
            AND NOT EXISTS (
                WITH RECURSIVE ancestors(agent_id, parent_agent_id) AS (
                    SELECT agent_id, parent_agent_id
                    FROM agent_nodes WHERE agent_id = NEW.recipient_agent_id
                    UNION ALL
                    SELECT parent.agent_id, parent.parent_agent_id
                    FROM agent_nodes AS parent
                    JOIN ancestors AS child ON parent.agent_id = child.parent_agent_id
                )
                SELECT 1 FROM ancestors
                WHERE agent_id = NEW.sender_agent_id
                  AND agent_id != NEW.recipient_agent_id
            )
        ) OR (
            NEW.kind = 'result'
            AND NOT EXISTS (
                SELECT 1 FROM agent_nodes AS sender
                WHERE sender.agent_id = NEW.sender_agent_id
                  AND sender.parent_agent_id = NEW.recipient_agent_id
            )
        )
        BEGIN
            SELECT RAISE(ABORT, 'Agent mailbox kind violates tree communication authority');
        END;
CREATE INDEX agent_mailbox_recipient_pending
            ON agent_mailbox_messages(recipient_agent_id, delivery_status, sequence);
CREATE UNIQUE INDEX agent_mailbox_claim_token_identity
            ON agent_mailbox_messages(claim_token)
            WHERE claim_token IS NOT NULL;
CREATE UNIQUE INDEX agent_mailbox_one_claimed_per_recipient
            ON agent_mailbox_messages(recipient_agent_id)
            WHERE delivery_status = 'claimed';
CREATE TRIGGER validate_agent_mailbox_projection_identity_insert
        BEFORE INSERT ON agent_mailbox_messages
        WHEN EXISTS (
            SELECT 1 FROM messages WHERE id = NEW.projection_message_id
        )
        BEGIN
            SELECT RAISE(ABORT, 'Agent mailbox projection identity is already in use');
        END;
CREATE TRIGGER prevent_agent_mailbox_identity_update
        BEFORE UPDATE OF
            sequence, message_id, schema_version, root_agent_id, sender_agent_id, recipient_agent_id,
            request_id, kind, content, projection_message_id, created_at
        ON agent_mailbox_messages
        BEGIN
            SELECT RAISE(ABORT, 'Agent mailbox identity and payload are immutable');
        END;
CREATE TRIGGER prevent_agent_mailbox_delete
        BEFORE DELETE ON agent_mailbox_messages
        BEGIN
            SELECT RAISE(ABORT, 'Agent mailbox transport facts are immutable');
        END;
CREATE TRIGGER validate_agent_mailbox_delivery_transition
        BEFORE UPDATE OF delivery_status ON agent_mailbox_messages
        WHEN NEW.delivery_status != OLD.delivery_status AND NOT (
            (OLD.delivery_status = 'queued' AND NEW.delivery_status = 'claimed')
            OR (OLD.delivery_status = 'claimed'
                AND NEW.delivery_status IN ('queued', 'acknowledged'))
        )
        BEGIN
            SELECT RAISE(ABORT, 'illegal Agent mailbox delivery transition');
        END;
CREATE TRIGGER prevent_agent_mailbox_claim_reassignment
        BEFORE UPDATE OF claim_token ON agent_mailbox_messages
        WHEN OLD.claim_token IS NOT NULL
          AND NEW.claim_token IS NOT OLD.claim_token
          AND NEW.delivery_status != 'queued'
        BEGIN
            SELECT RAISE(ABORT, 'Agent mailbox claim token is immutable while claimed');
        END;
CREATE TRIGGER validate_agent_mailbox_lease_update
        BEFORE UPDATE OF lease_expires_at ON agent_mailbox_messages
        WHEN OLD.delivery_status = 'claimed'
          AND NEW.delivery_status = 'claimed'
          AND NEW.lease_expires_at IS NOT OLD.lease_expires_at
          AND (
              NEW.claim_token IS NOT OLD.claim_token
              OR NEW.lease_expires_at IS NULL
              OR NEW.lease_expires_at <= OLD.lease_expires_at
          )
        BEGIN
            SELECT RAISE(ABORT, 'Agent mailbox lease renewal must advance for the same claim');
        END;
CREATE TRIGGER prevent_agent_mailbox_claim_time_rewrite
        BEFORE UPDATE OF claimed_at ON agent_mailbox_messages
        WHEN OLD.claimed_at IS NOT NULL
          AND NEW.claimed_at IS NOT OLD.claimed_at
          AND NEW.delivery_status != 'queued'
        BEGIN
            SELECT RAISE(ABORT, 'Agent mailbox claim time is immutable while claimed');
        END;
CREATE TRIGGER prevent_agent_mailbox_acknowledgement_rewrite
        BEFORE UPDATE OF delivery_status, claim_token, lease_expires_at, claimed_at, acknowledged_at
        ON agent_mailbox_messages
        WHEN OLD.delivery_status = 'acknowledged' AND (
            NEW.delivery_status IS NOT OLD.delivery_status
            OR NEW.claim_token IS NOT OLD.claim_token
            OR NEW.lease_expires_at IS NOT OLD.lease_expires_at
            OR NEW.claimed_at IS NOT OLD.claimed_at
            OR NEW.acknowledged_at IS NOT OLD.acknowledged_at
        )
        BEGIN
            SELECT RAISE(ABORT, 'Agent mailbox acknowledgement is immutable');
        END;
CREATE TABLE agent_wake_requests (
            sequence INTEGER PRIMARY KEY AUTOINCREMENT,
            wake_id TEXT NOT NULL UNIQUE CHECK (
                length(CAST(wake_id AS BLOB)) BETWEEN 1 AND 128
            ),
            schema_version INTEGER NOT NULL CHECK (schema_version = 1),
            root_agent_id TEXT NOT NULL,
            agent_id TEXT NOT NULL,
            requester_agent_id TEXT NOT NULL,
            request_id TEXT NOT NULL CHECK (
                length(CAST(request_id AS BLOB)) BETWEEN 1 AND 256
            ),
            source_agent_message_id TEXT,
            status TEXT NOT NULL CHECK (status IN (
                'queued', 'claimed', 'running', 'waiting_for_approval',
                'completed', 'failed', 'interrupted', 'cancelled', 'outcome_unknown',
                'satisfied'
            )),
            status_revision INTEGER NOT NULL DEFAULT 1 CHECK (status_revision > 0),
            claim_token TEXT,
            lease_expires_at INTEGER,
            result_message_id TEXT UNIQUE,
            terminal_error TEXT CHECK (
                terminal_error IS NULL
                OR length(CAST(terminal_error AS BLOB)) BETWEEN 1 AND 4096
            ),
            run_id TEXT CHECK (
                run_id IS NULL OR length(CAST(run_id AS BLOB)) BETWEEN 1 AND 2048
            ),
            assistant_message_id TEXT CHECK (
                assistant_message_id IS NULL
                OR length(CAST(assistant_message_id AS BLOB)) BETWEEN 1 AND 2048
            ),
            created_at INTEGER NOT NULL CHECK (created_at >= 0),
            claimed_at INTEGER,
            started_at INTEGER,
            completed_at INTEGER,
            UNIQUE(requester_agent_id, request_id),
            UNIQUE(source_agent_message_id),
            FOREIGN KEY (agent_id, root_agent_id)
                REFERENCES agent_nodes(agent_id, root_agent_id) ON DELETE RESTRICT,
            FOREIGN KEY (requester_agent_id, root_agent_id)
                REFERENCES agent_nodes(agent_id, root_agent_id) ON DELETE RESTRICT,
            FOREIGN KEY (
                source_agent_message_id, root_agent_id, requester_agent_id, agent_id
            ) REFERENCES agent_mailbox_messages(
                message_id, root_agent_id, sender_agent_id, recipient_agent_id
            ) ON DELETE RESTRICT,
            FOREIGN KEY (result_message_id)
                REFERENCES agent_mailbox_messages(message_id) ON DELETE RESTRICT,
            CHECK (agent_id != requester_agent_id),
            CHECK (
                (status = 'queued'
                    AND claim_token IS NULL
                    AND lease_expires_at IS NULL
                    AND claimed_at IS NULL
                    AND started_at IS NULL
                    AND completed_at IS NULL)
                OR (status = 'claimed'
                    AND claim_token IS NOT NULL
                    AND lease_expires_at IS NOT NULL
                    AND lease_expires_at > claimed_at
                    AND claimed_at IS NOT NULL
                    AND started_at IS NULL
                    AND completed_at IS NULL)
                OR (status IN ('running', 'waiting_for_approval')
                    AND claim_token IS NOT NULL
                    AND lease_expires_at IS NOT NULL
                    AND lease_expires_at > claimed_at
                    AND claimed_at IS NOT NULL
                    AND started_at IS NOT NULL
                    AND completed_at IS NULL)
                OR (status IN ('completed', 'failed', 'interrupted', 'cancelled', 'outcome_unknown')
                    AND completed_at IS NOT NULL)
                OR (status = 'satisfied'
                    AND claim_token IS NULL
                    AND lease_expires_at IS NULL
                    AND claimed_at IS NULL
                    AND started_at IS NULL
                    AND completed_at IS NOT NULL)
            ),
            CHECK (
                result_message_id IS NULL
                OR status IN ('completed', 'failed', 'interrupted', 'outcome_unknown')
            ),
            CHECK (
                terminal_error IS NULL
                OR status IN ('failed', 'interrupted', 'outcome_unknown')
            ),
            CHECK (
                (run_id IS NULL AND assistant_message_id IS NULL)
                OR (run_id IS NOT NULL AND assistant_message_id IS NOT NULL)
            )
        );
CREATE TRIGGER validate_agent_wake_active_target_insert
        BEFORE INSERT ON agent_wake_requests
        WHEN NOT EXISTS (
            SELECT 1 FROM agent_nodes
            WHERE agent_id = NEW.agent_id
              AND root_agent_id = NEW.root_agent_id
              AND lifecycle = 'active'
        ) OR (
            NEW.source_agent_message_id IS NOT NULL
            AND NOT EXISTS (
                SELECT 1 FROM agent_mailbox_messages
                WHERE message_id = NEW.source_agent_message_id
                  AND root_agent_id = NEW.root_agent_id
                  AND sender_agent_id = NEW.requester_agent_id
                  AND recipient_agent_id = NEW.agent_id
            )
        )
        BEGIN
            SELECT RAISE(ABORT, 'Agent wake target must be active and its source must match');
        END;
CREATE TRIGGER validate_agent_wake_source_authority_insert
        BEFORE INSERT ON agent_wake_requests
        WHEN NEW.source_agent_message_id IS NOT NULL
          AND NOT EXISTS (
              SELECT 1
              FROM agent_mailbox_messages AS source
              JOIN agent_nodes AS requester ON requester.agent_id = NEW.requester_agent_id
              JOIN agent_nodes AS target ON target.agent_id = NEW.agent_id
              WHERE source.message_id = NEW.source_agent_message_id
                AND source.root_agent_id = NEW.root_agent_id
                AND source.sender_agent_id = NEW.requester_agent_id
                AND source.recipient_agent_id = NEW.agent_id
                AND (
                    (source.kind = 'task'
                        AND target.parent_agent_id = requester.agent_id)
                    OR (source.kind = 'followup'
                        AND EXISTS (
                            WITH RECURSIVE ancestors(agent_id, parent_agent_id) AS (
                                SELECT agent_id, parent_agent_id
                                FROM agent_nodes WHERE agent_id = target.agent_id
                                UNION ALL
                                SELECT parent.agent_id, parent.parent_agent_id
                                FROM agent_nodes AS parent
                                JOIN ancestors AS child
                                  ON parent.agent_id = child.parent_agent_id
                            )
                            SELECT 1 FROM ancestors
                            WHERE agent_id = requester.agent_id
                              AND agent_id != target.agent_id
                        ))
                    OR (source.kind = 'result'
                        AND requester.parent_agent_id = target.agent_id)
                )
          )
        BEGIN
            SELECT RAISE(ABORT, 'Agent wake source violates tree communication authority');
        END;
CREATE TRIGGER validate_agent_wake_claim_source_delivered
        BEFORE UPDATE OF status ON agent_wake_requests
        WHEN NEW.status = 'claimed'
          AND NEW.source_agent_message_id IS NOT NULL
          AND NOT EXISTS (
              SELECT 1 FROM agent_mailbox_messages
              WHERE message_id = NEW.source_agent_message_id
                AND delivery_status = 'acknowledged'
          )
        BEGIN
            SELECT RAISE(ABORT, 'Agent wake source must be projected before claim');
        END;
CREATE UNIQUE INDEX agent_wake_one_active_turn
            ON agent_wake_requests(agent_id)
            WHERE status IN ('claimed', 'running', 'waiting_for_approval');
CREATE INDEX agent_wake_dispatch_queue
            ON agent_wake_requests(status, sequence);
CREATE UNIQUE INDEX agent_wake_claim_token_identity
            ON agent_wake_requests(claim_token)
            WHERE claim_token IS NOT NULL;
CREATE TRIGGER prevent_agent_wake_identity_update
        BEFORE UPDATE OF
            sequence, wake_id, schema_version, root_agent_id, agent_id, requester_agent_id,
            request_id, source_agent_message_id, created_at
        ON agent_wake_requests
        BEGIN
            SELECT RAISE(ABORT, 'Agent wake identity is immutable');
        END;
CREATE TRIGGER prevent_agent_wake_delete
        BEFORE DELETE ON agent_wake_requests
        BEGIN
            SELECT RAISE(ABORT, 'Agent wake coordination facts are immutable');
        END;
CREATE TRIGGER validate_agent_wake_transition
        BEFORE UPDATE OF status ON agent_wake_requests
        WHEN NEW.status != OLD.status AND NOT (
            (OLD.status = 'queued' AND NEW.status IN ('claimed', 'cancelled'))
            OR (OLD.status = 'queued' AND NEW.status = 'satisfied')
            OR (OLD.status = 'claimed' AND NEW.status IN (
                'queued', 'running', 'failed', 'interrupted', 'cancelled', 'outcome_unknown'
            ))
            OR (OLD.status = 'running' AND NEW.status IN (
                'waiting_for_approval', 'completed', 'failed', 'interrupted',
                'cancelled', 'outcome_unknown'
            ))
            OR (OLD.status = 'waiting_for_approval' AND NEW.status IN (
                'running', 'completed', 'failed', 'interrupted',
                'cancelled', 'outcome_unknown'
            ))
        )
        BEGIN
            SELECT RAISE(ABORT, 'illegal Agent wake transition');
        END;
CREATE TRIGGER validate_agent_wake_status_revision
        BEFORE UPDATE OF status, status_revision ON agent_wake_requests
        WHEN (
            NEW.status != OLD.status
            AND NEW.status_revision != OLD.status_revision + 1
        ) OR (
            NEW.status = OLD.status
            AND NEW.status_revision != OLD.status_revision
        )
        BEGIN
            SELECT RAISE(ABORT, 'Agent wake status revision must advance exactly once');
        END;
CREATE TRIGGER prevent_agent_wake_claim_reassignment
        BEFORE UPDATE OF claim_token ON agent_wake_requests
        WHEN OLD.claim_token IS NOT NULL
          AND NEW.claim_token IS NOT OLD.claim_token
          AND NEW.status != 'queued'
          AND NOT (
              OLD.status IN ('running', 'waiting_for_approval')
              AND NEW.status = OLD.status
              AND OLD.lease_expires_at IS NOT NULL
              AND NEW.claimed_at IS NOT NULL
              AND NEW.claimed_at >= OLD.lease_expires_at
              AND NEW.lease_expires_at > NEW.claimed_at
          )
        BEGIN
            SELECT RAISE(ABORT, 'Agent wake claim token is immutable while active');
        END;
CREATE TRIGGER validate_agent_wake_lease_update
        BEFORE UPDATE OF lease_expires_at ON agent_wake_requests
        WHEN OLD.status IN ('claimed', 'running', 'waiting_for_approval')
          AND NEW.status IN ('claimed', 'running', 'waiting_for_approval')
          AND NEW.lease_expires_at IS NOT OLD.lease_expires_at
          AND (
              (
                  NEW.claim_token IS NOT OLD.claim_token
                  AND NOT (
                      OLD.lease_expires_at IS NOT NULL
                      AND NEW.claimed_at IS NOT NULL
                      AND NEW.claimed_at >= OLD.lease_expires_at
                      AND NEW.lease_expires_at > NEW.claimed_at
                  )
              )
              OR NEW.lease_expires_at IS NULL
              OR NEW.lease_expires_at <= OLD.lease_expires_at
          )
        BEGIN
            SELECT RAISE(ABORT, 'Agent wake lease renewal must advance for the same claim');
        END;
CREATE TRIGGER validate_agent_wake_result_kind
        BEFORE UPDATE OF result_message_id ON agent_wake_requests
        WHEN NEW.result_message_id IS NOT NULL
          AND NOT EXISTS (
              SELECT 1
              FROM agent_mailbox_messages AS result
              JOIN agent_nodes AS child ON child.agent_id = NEW.agent_id
              WHERE result.message_id = NEW.result_message_id
                AND result.kind = 'result'
                AND result.root_agent_id = NEW.root_agent_id
                AND result.sender_agent_id = NEW.agent_id
                AND result.recipient_agent_id = child.parent_agent_id
          )
        BEGIN
            SELECT RAISE(ABORT, 'Agent wake result must target the child direct parent');
        END;
CREATE TRIGGER prevent_agent_wake_execution_identity_rewrite
        BEFORE UPDATE OF run_id, assistant_message_id ON agent_wake_requests
        WHEN (OLD.run_id IS NOT NULL OR OLD.assistant_message_id IS NOT NULL)
          AND (
              NEW.run_id IS NOT OLD.run_id
              OR NEW.assistant_message_id IS NOT OLD.assistant_message_id
          )
        BEGIN
            SELECT RAISE(ABORT, 'Agent wake execution identity is immutable once bound');
        END;
CREATE TRIGGER validate_agent_wake_execution_identity_bind
        BEFORE UPDATE OF run_id, assistant_message_id ON agent_wake_requests
        WHEN OLD.run_id IS NULL
          AND NEW.run_id IS NOT NULL
          AND (
              NEW.status NOT IN (
                  'running', 'waiting_for_approval', 'completed', 'failed',
                  'interrupted', 'outcome_unknown'
              )
              OR NOT EXISTS (
                  SELECT 1
                  FROM agent_nodes AS agent
                  JOIN conversation_turn_traces AS trace
                    ON trace.conversation_id = agent.conversation_id
                  WHERE agent.agent_id = NEW.agent_id
                    AND trace.run_id = NEW.run_id
                    AND trace.assistant_message_id = NEW.assistant_message_id
              )
          )
        BEGIN
            SELECT RAISE(ABORT, 'Agent wake execution identity must reference its exact Turn');
        END;
CREATE TRIGGER prevent_agent_wake_claim_time_rewrite
        BEFORE UPDATE OF claimed_at ON agent_wake_requests
        WHEN OLD.claimed_at IS NOT NULL
          AND NEW.claimed_at IS NOT OLD.claimed_at
          AND NEW.status = OLD.status
          AND NOT (
              OLD.status IN ('running', 'waiting_for_approval')
              AND OLD.lease_expires_at IS NOT NULL
              AND NEW.claimed_at >= OLD.lease_expires_at
              AND NEW.lease_expires_at > NEW.claimed_at
          )
        BEGIN
            SELECT RAISE(ABORT, 'Agent wake claim time is immutable within a state');
        END;
CREATE TRIGGER prevent_agent_wake_start_time_rewrite
        BEFORE UPDATE OF started_at ON agent_wake_requests
        WHEN OLD.started_at IS NOT NULL
          AND NEW.started_at IS NOT OLD.started_at
        BEGIN
            SELECT RAISE(ABORT, 'Agent wake start time is immutable');
        END;
CREATE TRIGGER prevent_agent_wake_terminal_rewrite
        BEFORE UPDATE OF
            status, status_revision, claim_token, lease_expires_at, result_message_id,
            terminal_error, run_id, assistant_message_id, claimed_at, started_at, completed_at
        ON agent_wake_requests
        WHEN OLD.status IN (
            'completed', 'failed', 'interrupted', 'cancelled', 'outcome_unknown', 'satisfied'
        ) AND (
            NEW.status IS NOT OLD.status
            OR NEW.status_revision IS NOT OLD.status_revision
            OR NEW.claim_token IS NOT OLD.claim_token
            OR NEW.lease_expires_at IS NOT OLD.lease_expires_at
            OR NEW.result_message_id IS NOT OLD.result_message_id
            OR NEW.terminal_error IS NOT OLD.terminal_error
            OR NEW.run_id IS NOT OLD.run_id
            OR NEW.assistant_message_id IS NOT OLD.assistant_message_id
            OR NEW.claimed_at IS NOT OLD.claimed_at
            OR NEW.started_at IS NOT OLD.started_at
            OR NEW.completed_at IS NOT OLD.completed_at
        )
        BEGIN
            SELECT RAISE(ABORT, 'Agent wake terminal fact is immutable');
        END;
CREATE TABLE agent_interrupt_requests (
            caller_agent_id TEXT NOT NULL,
            request_id TEXT NOT NULL,
            root_agent_id TEXT NOT NULL,
            target_agent_id TEXT NOT NULL,
            disposition TEXT NOT NULL CHECK (disposition IN (
                'no_pending_execution', 'queued_wake_cancelled', 'active_turn'
            )),
            wake_id TEXT,
            run_id TEXT,
            created_at INTEGER NOT NULL CHECK (created_at >= 0),
            dispatched_at INTEGER CHECK (dispatched_at IS NULL OR dispatched_at >= created_at),
            PRIMARY KEY (caller_agent_id, request_id),
            FOREIGN KEY (caller_agent_id, root_agent_id)
                REFERENCES agent_nodes(agent_id, root_agent_id) ON DELETE RESTRICT,
            FOREIGN KEY (target_agent_id, root_agent_id)
                REFERENCES agent_nodes(agent_id, root_agent_id) ON DELETE RESTRICT,
            FOREIGN KEY (wake_id) REFERENCES agent_wake_requests(wake_id) ON DELETE RESTRICT,
            CHECK (caller_agent_id != target_agent_id),
            CHECK (
                (disposition = 'no_pending_execution' AND wake_id IS NULL AND run_id IS NULL)
                OR (disposition = 'queued_wake_cancelled' AND wake_id IS NOT NULL AND run_id IS NULL)
                OR (disposition = 'active_turn' AND wake_id IS NOT NULL AND run_id IS NOT NULL)
            )
        );
CREATE TRIGGER prevent_agent_interrupt_request_rewrite
        BEFORE UPDATE OF caller_agent_id, request_id, root_agent_id, target_agent_id,
            disposition, wake_id, run_id, created_at
        ON agent_interrupt_requests
        BEGIN
            SELECT RAISE(ABORT, 'Agent interrupt request fact is immutable');
        END;
CREATE TABLE agent_model_batch_receipts (
            receipt_id TEXT PRIMARY KEY CHECK (
                length(CAST(receipt_id AS BLOB)) BETWEEN 1 AND 256
            ),
            agent_id TEXT NOT NULL,
            conversation_id TEXT NOT NULL,
            run_id TEXT NOT NULL CHECK (
                length(CAST(run_id AS BLOB)) BETWEEN 1 AND 256
            ),
            assistant_message_id TEXT NOT NULL,
            model_batch_index INTEGER NOT NULL CHECK (model_batch_index > 0),
            sampling_bound_at INTEGER CHECK (sampling_bound_at IS NULL OR sampling_bound_at >= 0),
            created_at INTEGER NOT NULL CHECK (created_at >= 0),
            updated_at INTEGER NOT NULL CHECK (updated_at >= created_at),
            UNIQUE (run_id, model_batch_index),
            UNIQUE (receipt_id, agent_id),
            FOREIGN KEY (agent_id) REFERENCES agent_nodes(agent_id) ON DELETE RESTRICT,
            FOREIGN KEY (conversation_id) REFERENCES conversations(id) ON DELETE RESTRICT,
            FOREIGN KEY (assistant_message_id)
                REFERENCES conversation_turn_traces(assistant_message_id) ON DELETE RESTRICT
        );
CREATE INDEX agent_model_batch_receipts_conversation_run
            ON agent_model_batch_receipts(conversation_id, run_id, model_batch_index);
CREATE TRIGGER validate_agent_model_batch_receipt_identity_insert
        BEFORE INSERT ON agent_model_batch_receipts
        WHEN NOT EXISTS (
            SELECT 1
            FROM agent_nodes AS agent
            INNER JOIN conversation_turn_traces AS trace
                ON trace.assistant_message_id = NEW.assistant_message_id
            WHERE agent.agent_id = NEW.agent_id
              AND agent.conversation_id = NEW.conversation_id
              AND trace.conversation_id = NEW.conversation_id
              AND trace.run_id = NEW.run_id
              AND trace.terminal_status = 'in_progress'
        )
        BEGIN
            SELECT RAISE(ABORT, 'Agent model batch receipt must bind the active Agent Turn');
        END;
CREATE TRIGGER prevent_agent_model_batch_receipt_identity_update
        BEFORE UPDATE OF
            receipt_id, agent_id, conversation_id, run_id, assistant_message_id,
            model_batch_index, created_at
        ON agent_model_batch_receipts
        BEGIN
            SELECT RAISE(ABORT, 'Agent model batch receipt identity is immutable');
        END;
CREATE TRIGGER prevent_agent_model_batch_receipt_reopen
        BEFORE UPDATE OF sampling_bound_at ON agent_model_batch_receipts
        WHEN OLD.sampling_bound_at IS NOT NULL
          AND NEW.sampling_bound_at IS NOT OLD.sampling_bound_at
        BEGIN
            SELECT RAISE(ABORT, 'Agent model batch receipt sampling boundary is immutable');
        END;
CREATE TRIGGER prevent_agent_model_batch_receipt_delete
        BEFORE DELETE ON agent_model_batch_receipts
        BEGIN
            SELECT RAISE(ABORT, 'Agent model batch receipts are durable coordination facts');
        END;
CREATE TABLE agent_model_batch_receipt_replays (
            receipt_id TEXT PRIMARY KEY,
            source_receipt_id TEXT NOT NULL UNIQUE,
            created_at INTEGER NOT NULL CHECK (created_at >= 0),
            FOREIGN KEY (receipt_id)
                REFERENCES agent_model_batch_receipts(receipt_id) ON DELETE RESTRICT,
            FOREIGN KEY (source_receipt_id)
                REFERENCES agent_model_batch_receipts(receipt_id) ON DELETE RESTRICT,
            CHECK (receipt_id != source_receipt_id)
        );
CREATE TRIGGER validate_agent_model_batch_receipt_replay_insert
        BEFORE INSERT ON agent_model_batch_receipt_replays
        WHEN NOT EXISTS (
            SELECT 1
            FROM agent_model_batch_receipts AS target
            JOIN agent_model_batch_receipts AS source
              ON source.receipt_id = NEW.source_receipt_id
            WHERE target.receipt_id = NEW.receipt_id
              AND target.agent_id = source.agent_id
              AND target.conversation_id = source.conversation_id
              AND target.run_id != source.run_id
              AND target.sampling_bound_at IS NULL
              AND source.sampling_bound_at IS NULL
        ) OR EXISTS (
            WITH RECURSIVE replay_ancestors(receipt_id) AS (
                SELECT NEW.source_receipt_id
                UNION ALL
                SELECT replay.source_receipt_id
                FROM agent_model_batch_receipt_replays AS replay
                JOIN replay_ancestors AS ancestor
                  ON replay.receipt_id = ancestor.receipt_id
            )
            SELECT 1 FROM replay_ancestors WHERE receipt_id = NEW.receipt_id
        )
        BEGIN
            SELECT RAISE(ABORT, 'Agent model batch receipt replay must link one open prior batch');
        END;
CREATE TRIGGER prevent_agent_model_batch_receipt_replay_update
        BEFORE UPDATE ON agent_model_batch_receipt_replays
        BEGIN
            SELECT RAISE(ABORT, 'Agent model batch receipt replay facts are immutable');
        END;
CREATE TRIGGER prevent_agent_model_batch_receipt_replay_delete
        BEFORE DELETE ON agent_model_batch_receipt_replays
        BEGIN
            SELECT RAISE(ABORT, 'Agent model batch receipt replay facts are durable');
        END;
CREATE TABLE agent_model_batch_receipt_items (
            receipt_id TEXT NOT NULL,
            message_id TEXT NOT NULL UNIQUE,
            ordinal INTEGER NOT NULL CHECK (ordinal >= 0),
            mailbox_sequence INTEGER NOT NULL CHECK (mailbox_sequence > 0),
            delivery_path TEXT NOT NULL CHECK (
                delivery_path IN ('turn_start', 'safe_boundary', 'wait_agent')
            ),
            trace_sequence INTEGER CHECK (trace_sequence IS NULL OR trace_sequence >= 0),
            bound_at INTEGER NOT NULL CHECK (bound_at >= 0),
            PRIMARY KEY (receipt_id, ordinal),
            UNIQUE (receipt_id, mailbox_sequence),
            FOREIGN KEY (receipt_id)
                REFERENCES agent_model_batch_receipts(receipt_id) ON DELETE CASCADE,
            FOREIGN KEY (message_id)
                REFERENCES agent_mailbox_messages(message_id) ON DELETE RESTRICT,
            CHECK (
                (delivery_path = 'safe_boundary' AND trace_sequence IS NOT NULL)
                OR (delivery_path != 'safe_boundary' AND trace_sequence IS NULL)
            )
        );
CREATE INDEX agent_model_batch_receipt_items_sequence
            ON agent_model_batch_receipt_items(receipt_id, mailbox_sequence);
CREATE TRIGGER validate_agent_model_batch_receipt_item_insert
        BEFORE INSERT ON agent_model_batch_receipt_items
        WHEN NOT EXISTS (
            SELECT 1
            FROM agent_model_batch_receipts AS receipt
            INNER JOIN agent_mailbox_messages AS message
                ON message.message_id = NEW.message_id
            WHERE receipt.receipt_id = NEW.receipt_id
              AND receipt.sampling_bound_at IS NULL
              AND message.recipient_agent_id = receipt.agent_id
              AND message.delivery_status = 'acknowledged'
              AND message.sequence = NEW.mailbox_sequence
        ) OR (
            NEW.delivery_path = 'safe_boundary'
            AND NOT EXISTS (
                SELECT 1
                FROM agent_model_batch_receipts AS receipt
                INNER JOIN conversation_turn_trace_items AS trace_item
                    ON trace_item.assistant_message_id = receipt.assistant_message_id
                   AND trace_item.sequence = NEW.trace_sequence
                WHERE receipt.receipt_id = NEW.receipt_id
                  AND trace_item.item_kind = 'agent_mailbox_delivery'
                  AND json_extract(trace_item.item_json, '$.receiptId') = NEW.receipt_id
                  AND json_extract(trace_item.item_json, '$.messageId') = NEW.message_id
            )
        )
        BEGIN
            SELECT RAISE(ABORT, 'Agent receipt item must match its acknowledged Mailbox/trace fact');
        END;
CREATE TRIGGER prevent_agent_model_batch_receipt_item_update
        BEFORE UPDATE ON agent_model_batch_receipt_items
        BEGIN
            SELECT RAISE(ABORT, 'Agent model batch receipt items are immutable');
        END;
CREATE TRIGGER prevent_agent_model_batch_receipt_item_delete
        BEFORE DELETE ON agent_model_batch_receipt_items
        BEGIN
            SELECT RAISE(ABORT, 'Agent model batch receipt items are durable coordination facts');
        END;
CREATE TABLE agent_model_batch_receipt_targets (
            receipt_id TEXT NOT NULL,
            target_agent_id TEXT NOT NULL,
            ordinal INTEGER NOT NULL CHECK (ordinal >= 0),
            target_status_version INTEGER NOT NULL CHECK (target_status_version > 0),
            latest_wake_sequence INTEGER CHECK (latest_wake_sequence > 0),
            latest_wake_status_revision INTEGER CHECK (latest_wake_status_revision > 0),
            latest_wake_status TEXT CHECK (
                latest_wake_status IS NULL OR latest_wake_status IN (
                    'queued', 'claimed', 'running', 'waiting_for_approval', 'completed',
                    'failed', 'interrupted', 'cancelled', 'outcome_unknown', 'satisfied'
                )
            ),
            display_status TEXT NOT NULL CHECK (
                display_status IN (
                    'idle', 'queued', 'running', 'waiting_approval', 'latest_completed',
                    'latest_failed', 'latest_interrupted', 'latest_outcome_unknown',
                    'archived', 'disabled'
                )
            ),
            frozen_at INTEGER NOT NULL CHECK (frozen_at >= 0),
            PRIMARY KEY (receipt_id, target_agent_id),
            UNIQUE (receipt_id, ordinal),
            FOREIGN KEY (receipt_id)
                REFERENCES agent_model_batch_receipts(receipt_id) ON DELETE CASCADE,
            FOREIGN KEY (target_agent_id)
                REFERENCES agent_nodes(agent_id) ON DELETE RESTRICT,
            CHECK (
                (latest_wake_sequence IS NULL
                 AND latest_wake_status_revision IS NULL
                 AND latest_wake_status IS NULL)
                OR (latest_wake_sequence IS NOT NULL
                    AND latest_wake_status_revision IS NOT NULL
                    AND latest_wake_status IS NOT NULL)
            )
        );
CREATE TRIGGER validate_agent_model_batch_receipt_target_insert
        BEFORE INSERT ON agent_model_batch_receipt_targets
        WHEN NOT EXISTS (
            WITH RECURSIVE ancestors(agent_id, parent_agent_id) AS (
                SELECT agent.agent_id, agent.parent_agent_id
                FROM agent_nodes AS agent
                WHERE agent.agent_id = NEW.target_agent_id
                UNION ALL
                SELECT parent.agent_id, parent.parent_agent_id
                FROM agent_nodes AS parent
                JOIN ancestors AS child ON parent.agent_id = child.parent_agent_id
            )
            SELECT 1
            FROM agent_model_batch_receipts AS receipt
            JOIN ancestors ON ancestors.agent_id = receipt.agent_id
            WHERE receipt.receipt_id = NEW.receipt_id
              AND receipt.sampling_bound_at IS NULL
              AND receipt.agent_id != NEW.target_agent_id
        )
        BEGIN
            SELECT RAISE(ABORT, 'Agent wait receipt target must be a strict descendant');
        END;
CREATE TRIGGER prevent_agent_model_batch_receipt_target_update
        BEFORE UPDATE ON agent_model_batch_receipt_targets
        BEGIN
            SELECT RAISE(ABORT, 'Agent model batch receipt target facts are immutable');
        END;
CREATE TRIGGER prevent_agent_model_batch_receipt_target_delete
        BEFORE DELETE ON agent_model_batch_receipt_targets
        BEGIN
            SELECT RAISE(ABORT, 'Agent model batch receipt target facts are durable coordination facts');
        END;
CREATE TRIGGER validate_agent_wake_satisfied_receipt
        BEFORE UPDATE OF status ON agent_wake_requests
        WHEN NEW.status = 'satisfied'
          AND OLD.status != 'satisfied'
          AND NOT EXISTS (
              SELECT 1
              FROM agent_model_batch_receipt_items
              WHERE message_id = NEW.source_agent_message_id
          )
        BEGIN
            SELECT RAISE(ABORT, 'Satisfied Agent wake requires a durable delivery receipt');
        END;
CREATE TABLE agent_collaboration_cursors (
            caller_agent_id TEXT NOT NULL,
            run_id TEXT NOT NULL CHECK (
                length(CAST(run_id AS BLOB)) BETWEEN 1 AND 256
            ),
            target_agent_id TEXT NOT NULL,
            last_target_status_version INTEGER NOT NULL DEFAULT 0
                CHECK (last_target_status_version >= 0),
            last_message_sequence INTEGER NOT NULL DEFAULT 0
                CHECK (last_message_sequence >= 0),
            last_wake_sequence INTEGER NOT NULL DEFAULT 0
                CHECK (last_wake_sequence >= 0),
            last_wake_status_revision INTEGER NOT NULL DEFAULT 0
                CHECK (last_wake_status_revision >= 0),
            updated_at INTEGER NOT NULL CHECK (updated_at >= 0),
            PRIMARY KEY (caller_agent_id, run_id, target_agent_id),
            FOREIGN KEY (caller_agent_id) REFERENCES agent_nodes(agent_id) ON DELETE RESTRICT,
            FOREIGN KEY (target_agent_id) REFERENCES agent_nodes(agent_id) ON DELETE RESTRICT,
            CHECK (caller_agent_id != target_agent_id)
        );
CREATE INDEX agent_collaboration_cursors_target
            ON agent_collaboration_cursors(target_agent_id, updated_at);
CREATE TRIGGER validate_agent_collaboration_cursor_authority_insert
        BEFORE INSERT ON agent_collaboration_cursors
        WHEN NOT EXISTS (
            WITH RECURSIVE ancestors(agent_id, parent_agent_id) AS (
                SELECT agent_id, parent_agent_id
                FROM agent_nodes WHERE agent_id = NEW.target_agent_id
                UNION ALL
                SELECT parent.agent_id, parent.parent_agent_id
                FROM agent_nodes AS parent
                JOIN ancestors AS child ON parent.agent_id = child.parent_agent_id
            )
            SELECT 1 FROM ancestors
            WHERE agent_id = NEW.caller_agent_id
              AND agent_id != NEW.target_agent_id
        )
        BEGIN
            SELECT RAISE(ABORT, 'Agent wait cursor target must be a strict descendant');
        END;
CREATE TRIGGER validate_agent_collaboration_cursor_update
        BEFORE UPDATE ON agent_collaboration_cursors
        WHEN NEW.caller_agent_id IS NOT OLD.caller_agent_id
          OR NEW.run_id IS NOT OLD.run_id
          OR NEW.target_agent_id IS NOT OLD.target_agent_id
          OR NEW.last_target_status_version < OLD.last_target_status_version
          OR NEW.last_message_sequence < OLD.last_message_sequence
          OR NEW.last_wake_sequence < OLD.last_wake_sequence
          OR (
              NEW.last_wake_sequence = OLD.last_wake_sequence
              AND NEW.last_wake_status_revision < OLD.last_wake_status_revision
          )
        BEGIN
            SELECT RAISE(ABORT, 'Agent collaboration cursor identity/version cannot rewind');
        END;
CREATE TABLE child_context_snapshots (
            target_conversation_id TEXT PRIMARY KEY,
            schema_version INTEGER NOT NULL CHECK (schema_version = 1),
            source_conversation_id TEXT NOT NULL CHECK (
                typeof(source_conversation_id) = 'text'
                AND length(CAST(source_conversation_id AS BLOB)) BETWEEN 1 AND 256
            ),
            fork_kind TEXT NOT NULL CHECK (fork_kind IN ('none', 'all', 'last')),
            fork_turn_count INTEGER,
            selected_turn_count INTEGER NOT NULL CHECK (selected_turn_count >= 0),
            created_at INTEGER NOT NULL CHECK (created_at >= 0),
            CHECK (
                (fork_kind IN ('none', 'all') AND fork_turn_count IS NULL)
                OR (fork_kind = 'last' AND fork_turn_count > 0)
            ),
            CHECK (fork_kind != 'none' OR selected_turn_count = 0),
            FOREIGN KEY (target_conversation_id)
                REFERENCES conversations(id) ON DELETE CASCADE
        );
CREATE INDEX child_context_snapshots_source_idx
            ON child_context_snapshots (source_conversation_id, created_at);
CREATE TRIGGER validate_child_context_snapshot_insert
        BEFORE INSERT ON child_context_snapshots
        BEGIN
            SELECT CASE WHEN NOT EXISTS (
                SELECT 1
                FROM agent_nodes AS child
                INNER JOIN agent_nodes AS parent
                    ON parent.agent_id = child.parent_agent_id
                WHERE child.conversation_id = NEW.target_conversation_id
                  AND parent.conversation_id = NEW.source_conversation_id
                  AND child.root_agent_id = parent.root_agent_id
                  AND child.root_conversation_id = parent.root_conversation_id
                  AND child.project_id IS parent.project_id
            ) THEN RAISE(ABORT, 'invalid child context snapshot boundary') END;
        END;
CREATE TRIGGER prevent_child_context_snapshot_update
        BEFORE UPDATE ON child_context_snapshots
        BEGIN
            SELECT RAISE(ABORT, 'Child context snapshot provenance is immutable');
        END;
CREATE TRIGGER prevent_child_context_snapshot_delete
        BEFORE DELETE ON child_context_snapshots
        WHEN EXISTS (
            SELECT 1 FROM conversations WHERE id = OLD.target_conversation_id
        )
        BEGIN
            SELECT RAISE(ABORT, 'Child context snapshot provenance is immutable');
        END;
CREATE TABLE messages (
            id TEXT PRIMARY KEY,
            conversation_id TEXT NOT NULL,
            role TEXT NOT NULL,
            content TEXT NOT NULL,
            status TEXT,
            input_origin_kind TEXT CHECK (
                input_origin_kind IS NULL OR input_origin_kind IN ('human', 'agent', 'snapshot')
            ),
            input_origin_agent_id TEXT,
            source_agent_message_id TEXT UNIQUE,
            snapshot_source_conversation_id TEXT,
            snapshot_source_message_id TEXT,
            snapshot_original_origin_kind TEXT CHECK (
                snapshot_original_origin_kind IS NULL
                OR snapshot_original_origin_kind IN ('human', 'agent')
            ),
            snapshot_original_agent_id TEXT,
            snapshot_original_mailbox_message_id TEXT,
            agent_run_json TEXT CHECK (
                agent_run_json IS NULL OR json_valid(agent_run_json)
            ),
            ui_state_json TEXT CHECK (
                ui_state_json IS NULL OR json_valid(ui_state_json)
            ),
            created_at INTEGER NOT NULL,
            position INTEGER NOT NULL,
            FOREIGN KEY (conversation_id) REFERENCES conversations(id) ON DELETE CASCADE,
            FOREIGN KEY (input_origin_agent_id) REFERENCES agent_nodes(agent_id) ON DELETE RESTRICT,
            FOREIGN KEY (source_agent_message_id)
                REFERENCES agent_mailbox_messages(message_id) ON DELETE RESTRICT,
            CHECK (
                (input_origin_kind IS NULL
                    AND input_origin_agent_id IS NULL
                    AND source_agent_message_id IS NULL
                    AND snapshot_source_conversation_id IS NULL
                    AND snapshot_source_message_id IS NULL
                    AND snapshot_original_origin_kind IS NULL
                    AND snapshot_original_agent_id IS NULL
                    AND snapshot_original_mailbox_message_id IS NULL)
                OR (input_origin_kind = 'human'
                    AND role = 'user'
                    AND input_origin_agent_id IS NULL
                    AND source_agent_message_id IS NULL
                    AND snapshot_source_conversation_id IS NULL
                    AND snapshot_source_message_id IS NULL
                    AND snapshot_original_origin_kind IS NULL
                    AND snapshot_original_agent_id IS NULL
                    AND snapshot_original_mailbox_message_id IS NULL)
                OR (input_origin_kind = 'agent'
                    AND role = 'user'
                    AND input_origin_agent_id IS NOT NULL
                    AND source_agent_message_id IS NOT NULL
                    AND snapshot_source_conversation_id IS NULL
                    AND snapshot_source_message_id IS NULL
                    AND snapshot_original_origin_kind IS NULL
                    AND snapshot_original_agent_id IS NULL
                    AND snapshot_original_mailbox_message_id IS NULL)
                OR (input_origin_kind = 'snapshot'
                    AND role IN ('user', 'assistant')
                    AND input_origin_agent_id IS NULL
                    AND source_agent_message_id IS NULL
                    AND snapshot_source_conversation_id IS NOT NULL
                    AND snapshot_source_message_id IS NOT NULL
                    AND (
                        (role = 'assistant'
                            AND snapshot_original_origin_kind IS NULL
                            AND snapshot_original_agent_id IS NULL
                            AND snapshot_original_mailbox_message_id IS NULL)
                        OR (role = 'user'
                            AND snapshot_original_origin_kind = 'human'
                            AND snapshot_original_agent_id IS NULL
                            AND snapshot_original_mailbox_message_id IS NULL)
                        OR (role = 'user'
                            AND snapshot_original_origin_kind = 'agent'
                            AND snapshot_original_agent_id IS NOT NULL
                            AND snapshot_original_mailbox_message_id IS NOT NULL)
                    ))
            )
        );
CREATE TRIGGER conversations_revision_after_message_insert
        AFTER INSERT ON messages
        BEGIN
            UPDATE conversations
            SET revision = revision + 1
            WHERE id = NEW.conversation_id;
        END;
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
          OR NEW.ui_state_json IS NOT OLD.ui_state_json
          OR NEW.created_at IS NOT OLD.created_at
          OR NEW.position IS NOT OLD.position
        BEGIN
            UPDATE conversations
            SET revision = revision + 1
            WHERE id IN (OLD.conversation_id, NEW.conversation_id);
        END;
CREATE TRIGGER conversations_revision_after_message_delete
        AFTER DELETE ON messages
        BEGIN
            UPDATE conversations
            SET revision = revision + 1
            WHERE id = OLD.conversation_id;
        END;
CREATE UNIQUE INDEX messages_snapshot_source_idx
            ON messages (
                conversation_id, snapshot_source_conversation_id, snapshot_source_message_id
            )
            WHERE input_origin_kind = 'snapshot';
CREATE TRIGGER validate_child_context_snapshot_message_insert
        BEFORE INSERT ON messages
        WHEN NEW.input_origin_kind = 'snapshot'
        BEGIN
            SELECT CASE WHEN NOT EXISTS (
                SELECT 1 FROM child_context_snapshots AS snapshot
                WHERE snapshot.target_conversation_id = NEW.conversation_id
                  AND snapshot.source_conversation_id = NEW.snapshot_source_conversation_id
            ) AND NOT EXISTS (
                SELECT 1 FROM conversation_forks AS fork
                WHERE fork.fork_authority = 'collaboration_root'
                  AND fork.target_conversation_id = NEW.conversation_id
                  AND fork.source_conversation_id = NEW.snapshot_source_conversation_id
            ) THEN RAISE(ABORT, 'invalid child context snapshot message') END;
            SELECT CASE WHEN NOT EXISTS (
                SELECT 1
                FROM messages AS source
                WHERE source.conversation_id = NEW.snapshot_source_conversation_id
                  AND source.id = NEW.snapshot_source_message_id
                  AND source.role = NEW.role
                  AND source.content = NEW.content
                  AND source.status IS NEW.status
                  AND source.created_at = NEW.created_at
                  AND (
                    (source.role = 'assistant'
                        AND NEW.snapshot_original_origin_kind IS NULL
                        AND NEW.snapshot_original_agent_id IS NULL
                        AND NEW.snapshot_original_mailbox_message_id IS NULL)
                    OR (source.role = 'user'
                        AND (source.input_origin_kind IS NULL
                            OR source.input_origin_kind = 'human')
                        AND NEW.snapshot_original_origin_kind = 'human'
                        AND NEW.snapshot_original_agent_id IS NULL
                        AND NEW.snapshot_original_mailbox_message_id IS NULL)
                    OR (source.role = 'user'
                        AND source.input_origin_kind = 'agent'
                        AND NEW.snapshot_original_origin_kind = 'agent'
                        AND NEW.snapshot_original_agent_id = source.input_origin_agent_id
                        AND NEW.snapshot_original_mailbox_message_id = source.source_agent_message_id)
                    OR (source.role = 'user'
                        AND source.input_origin_kind = 'snapshot'
                        AND NEW.snapshot_original_origin_kind = source.snapshot_original_origin_kind
                        AND NEW.snapshot_original_agent_id IS source.snapshot_original_agent_id
                        AND NEW.snapshot_original_mailbox_message_id
                            IS source.snapshot_original_mailbox_message_id)
                  )
            ) THEN RAISE(ABORT, 'child context snapshot source facts do not match') END;
        END;
CREATE TRIGGER validate_context_snapshot_message_update
        BEFORE UPDATE OF
            input_origin_kind, input_origin_agent_id, source_agent_message_id,
            snapshot_source_conversation_id, snapshot_source_message_id,
            snapshot_original_origin_kind, snapshot_original_agent_id,
            snapshot_original_mailbox_message_id
        ON messages
        WHEN NEW.input_origin_kind = 'snapshot' AND OLD.input_origin_kind IS NOT 'snapshot'
        BEGIN
            SELECT CASE WHEN NOT EXISTS (
                SELECT 1 FROM conversation_forks AS fork
                WHERE fork.fork_authority = 'collaboration_root'
                  AND fork.target_conversation_id = NEW.conversation_id
                  AND fork.source_conversation_id = NEW.snapshot_source_conversation_id
            ) THEN RAISE(ABORT, 'invalid collaboration root fork snapshot message') END;
            SELECT CASE WHEN NOT EXISTS (
                SELECT 1
                FROM messages AS source
                WHERE source.conversation_id = NEW.snapshot_source_conversation_id
                  AND source.id = NEW.snapshot_source_message_id
                  AND source.role = NEW.role
                  AND source.content = NEW.content
                  AND source.status IS NEW.status
                  AND source.created_at = NEW.created_at
                  AND (
                    (source.role = 'assistant'
                        AND NEW.snapshot_original_origin_kind IS NULL
                        AND NEW.snapshot_original_agent_id IS NULL
                        AND NEW.snapshot_original_mailbox_message_id IS NULL)
                    OR (source.role = 'user'
                        AND (source.input_origin_kind IS NULL
                            OR source.input_origin_kind = 'human')
                        AND NEW.snapshot_original_origin_kind = 'human'
                        AND NEW.snapshot_original_agent_id IS NULL
                        AND NEW.snapshot_original_mailbox_message_id IS NULL)
                    OR (source.role = 'user'
                        AND source.input_origin_kind = 'agent'
                        AND NEW.snapshot_original_origin_kind = 'agent'
                        AND NEW.snapshot_original_agent_id = source.input_origin_agent_id
                        AND NEW.snapshot_original_mailbox_message_id = source.source_agent_message_id)
                    OR (source.role = 'user'
                        AND source.input_origin_kind = 'snapshot'
                        AND NEW.snapshot_original_origin_kind = source.snapshot_original_origin_kind
                        AND NEW.snapshot_original_agent_id IS source.snapshot_original_agent_id
                        AND NEW.snapshot_original_mailbox_message_id
                            IS source.snapshot_original_mailbox_message_id)
                  )
            ) THEN RAISE(ABORT, 'collaboration root fork snapshot source facts do not match') END;
        END;
CREATE TRIGGER validate_agent_message_projection_insert
        BEFORE INSERT ON messages
        WHEN NEW.input_origin_kind = 'agent'
        BEGIN
            SELECT CASE WHEN NOT EXISTS (
                SELECT 1
                FROM agent_mailbox_messages AS mailbox
                INNER JOIN agent_nodes AS recipient
                    ON recipient.agent_id = mailbox.recipient_agent_id
                WHERE mailbox.message_id = NEW.source_agent_message_id
                  AND mailbox.sender_agent_id = NEW.input_origin_agent_id
                  AND mailbox.projection_message_id = NEW.id
                  AND mailbox.content = NEW.content
                  AND mailbox.delivery_status = 'claimed'
                  AND recipient.conversation_id = NEW.conversation_id
            ) THEN RAISE(ABORT, 'invalid Agent mailbox message projection') END;
        END;
CREATE TRIGGER prevent_human_input_to_child_agent
        BEFORE INSERT ON messages
        WHEN NEW.role = 'user'
          AND (NEW.input_origin_kind IS NULL OR NEW.input_origin_kind = 'human')
          AND EXISTS (
              SELECT 1 FROM agent_nodes
              WHERE conversation_id = NEW.conversation_id
                AND parent_agent_id IS NOT NULL
          )
        BEGIN
            SELECT RAISE(ABORT, 'Child Agent conversations accept only Agent-origin input');
        END;
CREATE TRIGGER prevent_human_input_update_to_child_agent
        BEFORE UPDATE OF role, input_origin_kind, input_origin_agent_id, source_agent_message_id
        ON messages
        WHEN NEW.role = 'user'
          AND (NEW.input_origin_kind IS NULL OR NEW.input_origin_kind = 'human')
          AND EXISTS (
              SELECT 1 FROM agent_nodes
              WHERE conversation_id = NEW.conversation_id
                AND parent_agent_id IS NOT NULL
          )
        BEGIN
            SELECT RAISE(ABORT, 'Child Agent conversations accept only Agent-origin input');
        END;
CREATE TRIGGER validate_agent_message_projection_update
        BEFORE UPDATE OF
            id, conversation_id, role, content, input_origin_kind,
            input_origin_agent_id, source_agent_message_id
        ON messages
        WHEN NEW.input_origin_kind = 'agent'
        BEGIN
            SELECT CASE WHEN NOT EXISTS (
                SELECT 1
                FROM agent_mailbox_messages AS mailbox
                INNER JOIN agent_nodes AS recipient
                    ON recipient.agent_id = mailbox.recipient_agent_id
                WHERE mailbox.message_id = NEW.source_agent_message_id
                  AND mailbox.sender_agent_id = NEW.input_origin_agent_id
                  AND mailbox.projection_message_id = NEW.id
                  AND mailbox.content = NEW.content
                  AND mailbox.delivery_status IN ('claimed', 'acknowledged')
                  AND recipient.conversation_id = NEW.conversation_id
            ) THEN RAISE(ABORT, 'invalid Agent mailbox message projection') END;
        END;
CREATE TRIGGER prevent_agent_message_projection_rewrite
        BEFORE UPDATE OF
            id, conversation_id, role, content, status, input_origin_kind,
            input_origin_agent_id, source_agent_message_id, created_at, position
        ON messages
        WHEN OLD.input_origin_kind = 'agent' AND (
            NEW.id IS NOT OLD.id
            OR NEW.conversation_id IS NOT OLD.conversation_id
            OR NEW.role IS NOT OLD.role
            OR NEW.content IS NOT OLD.content
            OR NEW.status IS NOT OLD.status
            OR NEW.input_origin_kind IS NOT OLD.input_origin_kind
            OR NEW.input_origin_agent_id IS NOT OLD.input_origin_agent_id
            OR NEW.source_agent_message_id IS NOT OLD.source_agent_message_id
            OR NEW.created_at IS NOT OLD.created_at
            OR NEW.position IS NOT OLD.position
        )
        BEGIN
            SELECT RAISE(ABORT, 'Agent message projection is immutable');
        END;
CREATE TRIGGER prevent_agent_message_projection_delete
        BEFORE DELETE ON messages
        WHEN OLD.input_origin_kind = 'agent'
        BEGIN
            SELECT RAISE(ABORT, 'Agent message projection is immutable');
        END;
CREATE TRIGGER prevent_agent_message_projection_ui_rewrite
        BEFORE UPDATE OF agent_run_json, ui_state_json ON messages
        WHEN OLD.input_origin_kind = 'agent' AND (
            NEW.agent_run_json IS NOT OLD.agent_run_json
            OR NEW.ui_state_json IS NOT OLD.ui_state_json
        )
        BEGIN
            SELECT RAISE(ABORT, 'Agent input projection cannot carry mutable run UI state');
        END;
CREATE TRIGGER prevent_child_context_snapshot_message_rewrite
        BEFORE UPDATE ON messages
        WHEN OLD.input_origin_kind = 'snapshot'
        BEGIN
            SELECT RAISE(ABORT, 'Child context snapshot message is immutable');
        END;
CREATE TRIGGER prevent_child_context_snapshot_message_delete
        BEFORE DELETE ON messages
        WHEN OLD.input_origin_kind = 'snapshot'
          AND EXISTS (
              SELECT 1 FROM conversations WHERE id = OLD.conversation_id
          )
        BEGIN
            SELECT RAISE(ABORT, 'Child context snapshot message is immutable');
        END;
CREATE TRIGGER validate_agent_mailbox_acknowledgement
        BEFORE UPDATE OF delivery_status ON agent_mailbox_messages
        WHEN NEW.delivery_status = 'acknowledged' AND NOT EXISTS (
            SELECT 1 FROM messages
            WHERE source_agent_message_id = OLD.message_id
              AND id = OLD.projection_message_id
        )
        BEGIN
            SELECT RAISE(ABORT, 'Agent mailbox acknowledgement requires its projection');
        END;
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
CREATE UNIQUE INDEX conversation_turn_traces_one_active_turn
            ON conversation_turn_traces (conversation_id)
            WHERE terminal_status = 'in_progress';
CREATE TABLE agent_effective_permission_snapshots (
            agent_id TEXT PRIMARY KEY CHECK (
                typeof(agent_id) = 'text'
                AND length(CAST(agent_id AS BLOB)) BETWEEN 1 AND 128
            ),
            schema_version INTEGER NOT NULL CHECK (schema_version = 1),
            root_agent_id TEXT NOT NULL CHECK (
                typeof(root_agent_id) = 'text'
                AND length(CAST(root_agent_id AS BLOB)) BETWEEN 1 AND 128
            ),
            conversation_id TEXT NOT NULL UNIQUE CHECK (
                typeof(conversation_id) = 'text'
                AND length(CAST(conversation_id AS BLOB)) BETWEEN 1 AND 128
            ),
            source_run_id TEXT NOT NULL CHECK (
                typeof(source_run_id) = 'text'
                AND length(CAST(source_run_id AS BLOB)) BETWEEN 1 AND 128
            ),
            source_assistant_message_id TEXT NOT NULL CHECK (
                typeof(source_assistant_message_id) = 'text'
                AND length(CAST(source_assistant_message_id AS BLOB)) BETWEEN 1 AND 128
            ),
            read_permission TEXT NOT NULL CHECK (
                read_permission IN ('workspace_only', 'all')
            ),
            write_permission TEXT NOT NULL CHECK (
                write_permission IN ('denied', 'workspace_only', 'all')
            ),
            command_permission TEXT NOT NULL CHECK (
                command_permission IN ('require_approval', 'auto_approve')
            ),
            command_safety_policy TEXT NOT NULL CHECK (
                command_safety_policy IN ('guarded', 'full_access')
            ),
            patch_permission TEXT NOT NULL CHECK (
                patch_permission IN ('require_approval', 'auto_approve')
            ),
            revision INTEGER NOT NULL CHECK (revision > 0),
            created_at INTEGER NOT NULL CHECK (created_at >= 0),
            updated_at INTEGER NOT NULL CHECK (updated_at >= created_at),
            FOREIGN KEY (agent_id) REFERENCES agent_nodes(agent_id) ON DELETE CASCADE
        );
CREATE INDEX agent_effective_permission_snapshots_root
            ON agent_effective_permission_snapshots(root_agent_id, agent_id);
CREATE TRIGGER validate_agent_effective_permission_snapshot_source_insert
        BEFORE INSERT ON agent_effective_permission_snapshots
        WHEN NOT EXISTS (
            SELECT 1
            FROM agent_nodes AS node
            JOIN conversation_turn_traces AS trace
              ON trace.conversation_id = node.conversation_id
            WHERE node.agent_id = NEW.agent_id
              AND node.root_agent_id = NEW.root_agent_id
              AND node.conversation_id = NEW.conversation_id
              AND node.lifecycle = 'active'
              AND trace.run_id = NEW.source_run_id
              AND trace.assistant_message_id = NEW.source_assistant_message_id
              AND trace.terminal_status = 'in_progress'
        )
        BEGIN
            SELECT RAISE(ABORT, 'Agent effective permissions require an active exact Turn');
        END;
CREATE TRIGGER validate_agent_effective_permission_snapshot_source_update
        BEFORE UPDATE OF
            source_run_id, source_assistant_message_id, read_permission, write_permission,
            command_permission, command_safety_policy, patch_permission, revision, updated_at
        ON agent_effective_permission_snapshots
        WHEN NOT EXISTS (
            SELECT 1
            FROM agent_nodes AS node
            JOIN conversation_turn_traces AS trace
              ON trace.conversation_id = node.conversation_id
            WHERE node.agent_id = NEW.agent_id
              AND node.root_agent_id = NEW.root_agent_id
              AND node.conversation_id = NEW.conversation_id
              AND node.lifecycle = 'active'
              AND trace.run_id = NEW.source_run_id
              AND trace.assistant_message_id = NEW.source_assistant_message_id
              AND trace.terminal_status = 'in_progress'
        )
        BEGIN
            SELECT RAISE(ABORT, 'Agent effective permissions require an active exact Turn');
        END;
CREATE TRIGGER prevent_agent_effective_permission_snapshot_identity_update
        BEFORE UPDATE OF agent_id, schema_version, root_agent_id, conversation_id, created_at
        ON agent_effective_permission_snapshots
        BEGIN
            SELECT RAISE(ABORT, 'Agent effective permission snapshot identity is immutable');
        END;
CREATE TRIGGER validate_agent_effective_permission_snapshot_revision
        BEFORE UPDATE OF revision, updated_at ON agent_effective_permission_snapshots
        WHEN NEW.revision != OLD.revision + 1 OR NEW.updated_at <= OLD.updated_at
        BEGIN
            SELECT RAISE(ABORT, 'invalid Agent effective permission snapshot revision');
        END;
CREATE TABLE conversation_turn_trace_items (
            assistant_message_id TEXT NOT NULL,
            sequence INTEGER NOT NULL CHECK (sequence >= 0),
            item_kind TEXT NOT NULL CHECK (item_kind IN (
                'assistant_narration', 'user_guidance', 'tool_call', 'tool_result',
                'command_session_lifecycle', 'agent_mailbox_delivery'
            )),
            item_json TEXT NOT NULL CHECK (json_valid(item_json)),
            PRIMARY KEY (assistant_message_id, sequence),
            FOREIGN KEY (assistant_message_id) REFERENCES conversation_turn_traces(assistant_message_id) ON DELETE CASCADE
        );
CREATE TABLE conversation_model_context_items (
            assistant_message_id TEXT NOT NULL,
            sequence INTEGER NOT NULL CHECK (sequence >= 0),
            ordinal INTEGER NOT NULL DEFAULT 0 CHECK (ordinal >= 0),
            content_hash TEXT NOT NULL CHECK (
                length(content_hash) = 71
                AND substr(content_hash, 1, 7) = 'sha256:'
                AND substr(content_hash, 8) NOT GLOB '*[^0-9a-f]*'
            ),
            uncompressed_bytes INTEGER NOT NULL CHECK (uncompressed_bytes >= 0),
            compression TEXT NOT NULL CHECK (compression = 'zstd'),
            payload BLOB NOT NULL,
            PRIMARY KEY (assistant_message_id, sequence, ordinal),
            FOREIGN KEY (assistant_message_id)
                REFERENCES conversation_turn_traces(assistant_message_id) ON DELETE CASCADE
        );
CREATE TABLE conversation_history_blobs (
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
CREATE TABLE conversation_history_blob_chunks (
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
CREATE INDEX conversation_history_blobs_trace_item_idx
            ON conversation_history_blobs (
                conversation_id, assistant_message_id, sequence
            );
CREATE TRIGGER validate_conversation_history_blob_message_insert
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
CREATE TABLE conversation_world_state_epochs (
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
CREATE TRIGGER validate_conversation_world_state_epoch_summary_insert
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
CREATE TRIGGER validate_conversation_world_state_epoch_summary_update
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
CREATE TABLE agent_run_guidances (
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
CREATE TABLE agent_run_guidance_attachments (
            guidance_id TEXT NOT NULL,
            attachment_id TEXT NOT NULL UNIQUE,
            position INTEGER NOT NULL CHECK (position >= 0),
            PRIMARY KEY (guidance_id, position),
            FOREIGN KEY (guidance_id) REFERENCES agent_run_guidances(guidance_id) ON DELETE CASCADE,
            FOREIGN KEY (attachment_id) REFERENCES attachments(id) ON DELETE CASCADE
        );
CREATE TABLE agent_turn_diffs (
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
CREATE TABLE agent_turn_diff_files (
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
CREATE TABLE agent_turn_diff_actions (
            assistant_message_id TEXT NOT NULL,
            action_id TEXT NOT NULL CHECK (length(trim(action_id)) > 0),
            created_at INTEGER NOT NULL CHECK (created_at >= 0),
            PRIMARY KEY (assistant_message_id, action_id),
            FOREIGN KEY (assistant_message_id) REFERENCES agent_turn_diffs(assistant_message_id) ON DELETE CASCADE
        );
CREATE TABLE context_compaction_summaries (
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
            continuity_json TEXT NOT NULL CHECK (
                json_valid(continuity_json) AND length(trim(continuity_json)) > 0
            ),
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
CREATE TABLE context_compaction_summary_lineage (
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
CREATE TRIGGER delete_context_compaction_summary_with_lineage
        AFTER DELETE ON context_compaction_summary_lineage
        WHEN EXISTS (
            SELECT 1 FROM context_compaction_summaries WHERE id = OLD.summary_id
        )
        BEGIN
            DELETE FROM context_compaction_summaries WHERE id = OLD.summary_id;
        END;
CREATE TRIGGER validate_context_compaction_summary_lineage_insert
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
CREATE TRIGGER validate_context_compaction_summary_lineage_update
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
CREATE TABLE model_request_observations (
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
            observation_json TEXT NOT NULL CHECK (
                json_valid(observation_json) AND length(trim(observation_json)) > 0
            ),
            started_at INTEGER NOT NULL CHECK (started_at >= 0),
            completed_at INTEGER NOT NULL CHECK (completed_at >= started_at),
            CHECK (
                (conversation_id IS NULL AND assistant_message_id IS NULL)
                OR (conversation_id IS NOT NULL AND assistant_message_id IS NOT NULL)
            ),
            FOREIGN KEY (conversation_id) REFERENCES conversations(id) ON DELETE CASCADE,
            FOREIGN KEY (assistant_message_id) REFERENCES messages(id) ON DELETE CASCADE
        );
CREATE TABLE context_compaction_receipts (
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
            receipt_json TEXT NOT NULL CHECK (
                json_valid(receipt_json) AND length(trim(receipt_json)) > 0
            ),
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
CREATE TABLE provider_transition_terminal_records (
            operation_id TEXT PRIMARY KEY CHECK (
                typeof(operation_id) = 'text'
                AND length(CAST(operation_id AS BLOB)) BETWEEN 21 AND 1024
                AND substr(operation_id, 1, 20) = 'provider-transition-'
            ),
            schema_version INTEGER NOT NULL CHECK (schema_version = 1),
            conversation_id TEXT NOT NULL CHECK (
                typeof(conversation_id) = 'text'
                AND length(CAST(conversation_id AS BLOB)) BETWEEN 1 AND 512
                AND conversation_id = trim(conversation_id)
            ),
            target_model_id TEXT NOT NULL CHECK (
                typeof(target_model_id) = 'text'
                AND length(CAST(target_model_id AS BLOB)) BETWEEN 1 AND 512
                AND target_model_id = trim(target_model_id)
            ),
            source_model_display_name TEXT CHECK (
                source_model_display_name IS NULL OR (
                    length(CAST(source_model_display_name AS BLOB)) BETWEEN 1 AND 512
                    AND source_model_display_name = trim(source_model_display_name)
                )
            ),
            target_model_display_name TEXT CHECK (
                target_model_display_name IS NULL OR (
                    length(CAST(target_model_display_name AS BLOB)) BETWEEN 1 AND 512
                    AND target_model_display_name = trim(target_model_display_name)
                )
            ),
            started_at INTEGER NOT NULL CHECK (started_at >= 0),
            completed_at INTEGER NOT NULL CHECK (completed_at >= started_at),
            conversation_updated_at INTEGER NOT NULL CHECK (
                conversation_updated_at >= started_at
            ),
            FOREIGN KEY (conversation_id) REFERENCES conversations(id) ON DELETE CASCADE
        );
CREATE TABLE conversation_context_compaction_heads (
            conversation_id TEXT PRIMARY KEY,
            summary_id TEXT NOT NULL UNIQUE,
            revision INTEGER NOT NULL CHECK (revision > 0),
            updated_at INTEGER NOT NULL CHECK (updated_at >= 0),
            FOREIGN KEY (conversation_id) REFERENCES conversations(id) ON DELETE CASCADE,
            FOREIGN KEY (summary_id) REFERENCES context_compaction_summaries(id) ON DELETE CASCADE
        );
CREATE TABLE conversation_forks (
            request_id TEXT PRIMARY KEY,
            target_conversation_id TEXT NOT NULL UNIQUE,
            source_conversation_id TEXT NOT NULL,
            source_message_id TEXT NOT NULL,
            target_message_id TEXT NOT NULL,
            fork_authority TEXT NOT NULL DEFAULT 'legacy' CHECK (
                fork_authority IN ('legacy', 'collaboration_root')
            ),
            source_root_agent_id TEXT,
            target_root_agent_id TEXT,
            source_fork_point_json TEXT NOT NULL CHECK (
                json_valid(source_fork_point_json)
            ),
            created_at INTEGER NOT NULL CHECK (created_at >= 0),
            FOREIGN KEY (target_conversation_id) REFERENCES conversations(id) ON DELETE CASCADE,
            FOREIGN KEY (target_message_id) REFERENCES messages(id) ON DELETE CASCADE,
            FOREIGN KEY (source_root_agent_id) REFERENCES agent_nodes(agent_id) ON DELETE RESTRICT,
            FOREIGN KEY (target_root_agent_id) REFERENCES agent_nodes(agent_id) ON DELETE RESTRICT,
            CHECK (
                (fork_authority = 'legacy'
                    AND source_root_agent_id IS NULL
                    AND target_root_agent_id IS NULL)
                OR (fork_authority = 'collaboration_root'
                    AND source_root_agent_id IS NOT NULL
                    AND target_root_agent_id IS NOT NULL
                    AND source_root_agent_id != target_root_agent_id)
            )
        );
CREATE TRIGGER prevent_agent_bound_conversation_fork_insert
        BEFORE INSERT ON conversation_forks
        WHEN (
            NEW.fork_authority = 'legacy'
            AND EXISTS (
                SELECT 1 FROM agent_nodes
                WHERE conversation_id = NEW.source_conversation_id
                   OR conversation_id = NEW.target_conversation_id
            )
        ) OR (
            NEW.fork_authority = 'collaboration_root'
            AND NOT EXISTS (
                SELECT 1
                FROM agent_nodes AS source
                INNER JOIN agent_nodes AS target
                    ON target.agent_id = NEW.target_root_agent_id
                WHERE source.agent_id = NEW.source_root_agent_id
                  AND source.parent_agent_id IS NULL
                  AND source.lifecycle = 'active'
                  AND source.conversation_id = NEW.source_conversation_id
                  AND source.root_conversation_id = NEW.source_conversation_id
                  AND target.parent_agent_id IS NULL
                  AND target.lifecycle = 'active'
                  AND target.conversation_id = NEW.target_conversation_id
                  AND target.root_conversation_id = NEW.target_conversation_id
                  AND target.project_id IS source.project_id
                  AND target.root_agent_id != source.root_agent_id
            )
        )
        BEGIN
            SELECT RAISE(ABORT, 'invalid Agent-bound Conversation fork authority');
        END;
CREATE TRIGGER prevent_conversation_fork_update
        BEFORE UPDATE ON conversation_forks
        BEGIN
            SELECT RAISE(ABORT, 'Conversation fork receipt is immutable');
        END;
CREATE TABLE conversation_context_adaptation_requirements (
            conversation_id TEXT PRIMARY KEY,
            schema_version INTEGER NOT NULL CHECK (schema_version = 1),
            reason TEXT NOT NULL CHECK (
                reason = 'fork_released_provider_state_requires_compaction'
            ),
            source_conversation_id TEXT NOT NULL CHECK (
                typeof(source_conversation_id) = 'text'
                AND length(CAST(source_conversation_id AS BLOB)) BETWEEN 1 AND 512
                AND source_conversation_id = trim(source_conversation_id)
            ),
            source_message_id TEXT NOT NULL CHECK (
                typeof(source_message_id) = 'text'
                AND length(CAST(source_message_id AS BLOB)) BETWEEN 1 AND 512
                AND source_message_id = trim(source_message_id)
            ),
            created_at INTEGER NOT NULL CHECK (created_at >= 0),
            resolved_summary_id TEXT,
            resolved_at INTEGER CHECK (resolved_at IS NULL OR resolved_at >= created_at),
            CHECK (
                (resolved_summary_id IS NULL AND resolved_at IS NULL)
                OR (
                    typeof(resolved_summary_id) = 'text'
                    AND length(CAST(resolved_summary_id AS BLOB)) BETWEEN 1 AND 512
                    AND resolved_summary_id = trim(resolved_summary_id)
                    AND resolved_at IS NOT NULL
                )
            ),
            FOREIGN KEY (conversation_id) REFERENCES conversations(id) ON DELETE CASCADE
        );
CREATE INDEX idx_models_position ON models(position);
CREATE INDEX idx_projects_updated_at ON projects(updated_at);
CREATE INDEX idx_conversations_project_id ON conversations(project_id);
CREATE INDEX idx_conversations_pinned_at ON conversations(pinned_at);
CREATE INDEX idx_conversations_archived_at ON conversations(archived_at);
CREATE INDEX idx_conversations_unread_at ON conversations(unread_at);
CREATE INDEX idx_conversations_updated_at ON conversations(updated_at);
CREATE INDEX idx_messages_conversation_id ON messages(conversation_id, position);
CREATE INDEX idx_conversation_world_state_records_conversation
            ON conversation_world_state_records(conversation_id, journal_position);
CREATE INDEX idx_conversation_world_state_records_anchor
            ON conversation_world_state_records(effective_before_message_id);
CREATE INDEX idx_conversation_world_state_epochs_active
            ON conversation_world_state_epochs(conversation_id, generation DESC);
CREATE INDEX idx_agent_run_guidances_run_status ON agent_run_guidances(run_id, status, created_at);
CREATE INDEX idx_agent_run_guidances_conversation ON agent_run_guidances(conversation_id, created_at);
CREATE INDEX idx_agent_turn_diffs_conversation_project ON agent_turn_diffs(conversation_id, project_id, updated_at);
CREATE INDEX idx_context_compaction_summaries_conversation_id ON context_compaction_summaries(conversation_id, created_at);
CREATE INDEX idx_context_compaction_summary_lineage_owner ON context_compaction_summary_lineage(conversation_id, introduced_by_assistant_message_id);
CREATE INDEX idx_context_compaction_summary_lineage_source ON context_compaction_summary_lineage(source_conversation_id, source_summary_id);
CREATE INDEX idx_model_request_observations_conversation_id ON model_request_observations(conversation_id, completed_at);
CREATE INDEX idx_model_request_observations_operation_id ON model_request_observations(operation_id);
CREATE INDEX idx_model_request_observations_profile ON model_request_observations(model, api_style, purpose, completed_at);
CREATE INDEX idx_context_compaction_receipts_conversation_id ON context_compaction_receipts(conversation_id, started_at);
CREATE INDEX idx_context_compaction_receipts_run_id ON context_compaction_receipts(run_id, request_index, attempt_index);
CREATE INDEX idx_context_compaction_receipts_status ON context_compaction_receipts(status, updated_at);
CREATE INDEX idx_provider_transition_terminal_records_conversation
            ON provider_transition_terminal_records(conversation_id, started_at DESC, operation_id DESC);
CREATE INDEX idx_attachments_conversation_id ON attachments(conversation_id, created_at);
CREATE INDEX idx_attachments_project_id ON attachments(project_id, created_at);
CREATE INDEX idx_attachments_message_id ON attachments(message_id);
CREATE INDEX idx_agent_usage_records_created_at ON agent_usage_records(created_at);
CREATE INDEX idx_agent_usage_records_model_id ON agent_usage_records(model_id);
CREATE INDEX idx_agent_usage_records_project_id ON agent_usage_records(project_id);
CREATE INDEX idx_agent_deleted_usage_daily_rollups_usage_day ON agent_deleted_usage_daily_rollups(usage_day);
CREATE INDEX idx_agent_deleted_usage_daily_rollups_model_id ON agent_deleted_usage_daily_rollups(model_id);
CREATE INDEX idx_agent_action_audit_run_id ON agent_action_audit(run_id);
CREATE INDEX idx_agent_action_audit_conversation_id ON agent_action_audit(conversation_id);
CREATE INDEX idx_agent_action_audit_created_at ON agent_action_audit(created_at);
CREATE INDEX idx_agent_action_audit_status ON agent_action_audit(status);
CREATE INDEX idx_agent_pending_actions_status ON agent_pending_actions(status);
CREATE INDEX idx_agent_pending_actions_run_id ON agent_pending_actions(run_id);
CREATE INDEX idx_agent_pending_actions_conversation_id ON agent_pending_actions(conversation_id);
CREATE INDEX idx_mcp_approval_payload_envelopes_expires_at
            ON mcp_approval_payload_envelopes(expires_at);
CREATE INDEX idx_mcp_approval_payload_envelopes_action_id
            ON mcp_approval_payload_envelopes(action_id);
CREATE INDEX idx_provider_continuations_replay_scope
            ON provider_continuations(
                conversation_id, assistant_message_id, run_id, request_index
            );
CREATE INDEX idx_provider_continuations_state
            ON provider_continuations(state, updated_at);
CREATE INDEX idx_provider_continuation_tool_calls_runtime
            ON provider_continuation_tool_calls(runtime_call_id, continuation_id);
CREATE INDEX idx_agent_file_drafts_conversation_status ON agent_file_drafts(conversation_id, status);
CREATE INDEX idx_agent_file_drafts_project_id ON agent_file_drafts(project_id);
CREATE INDEX idx_agent_file_drafts_expires_at ON agent_file_drafts(expires_at);
CREATE INDEX idx_composer_drafts_updated_at ON composer_drafts(updated_at);
CREATE UNIQUE INDEX conversation_trace_command_session_lifecycle_identity
         ON conversation_turn_trace_items (
             assistant_message_id,
             json_extract(item_json, '$.sessionId'),
             json_extract(item_json, '$.phase')
         )
         WHERE item_kind = 'command_session_lifecycle';
CREATE INDEX idx_conversation_turn_traces_conversation_id
            ON conversation_turn_traces(conversation_id, updated_at);
CREATE TRIGGER validate_agent_usage_message_insert
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
CREATE TRIGGER validate_agent_usage_message_update
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
CREATE TABLE conversation_goals (
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
CREATE TRIGGER validate_conversation_goal_source_insert
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
CREATE TRIGGER validate_conversation_goal_source_update
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
CREATE TABLE conversation_goal_revisions (
            conversation_id TEXT NOT NULL,
            goal_id TEXT NOT NULL,
            sequence INTEGER NOT NULL CHECK (sequence > 0),
            schema_version INTEGER NOT NULL CHECK (schema_version = 1),
            actor TEXT NOT NULL CHECK (actor IN ('model', 'user')),
            event_kind TEXT NOT NULL CHECK (
                event_kind IN ('initial', 'objective_changed', 'status_changed')
            ),
            event_json TEXT NOT NULL CHECK (json_valid(event_json)),
            created_at INTEGER NOT NULL CHECK (created_at >= 0),
            PRIMARY KEY (goal_id, sequence),
            CHECK (
                (sequence = 1 AND event_kind = 'initial')
                OR (sequence > 1 AND event_kind != 'initial')
            ),
            FOREIGN KEY (conversation_id) REFERENCES conversations(id) ON DELETE CASCADE
        );
CREATE INDEX conversation_goal_revisions_conversation
        ON conversation_goal_revisions(conversation_id, created_at, goal_id, sequence);
CREATE TRIGGER prevent_conversation_goal_revision_update
        BEFORE UPDATE ON conversation_goal_revisions
        BEGIN
            SELECT RAISE(ABORT, 'goal revisions are append-only');
        END;
CREATE TRIGGER prevent_conversation_goal_revision_delete
        BEFORE DELETE ON conversation_goal_revisions
        WHEN EXISTS (
            SELECT 1 FROM conversations WHERE id = OLD.conversation_id
        )
        BEGIN
            SELECT RAISE(ABORT, 'goal revisions are append-only');
        END;
CREATE TABLE agent_command_sessions (
            session_id TEXT PRIMARY KEY CHECK (
                length(session_id) = 36
                AND substr(session_id, 1, 4) = 'cmd_'
                AND substr(session_id, 5) NOT GLOB '*[^0-9A-Fa-f]*'
            ),
            schema_version INTEGER NOT NULL CHECK (schema_version = 1),
            conversation_id TEXT NOT NULL,
            assistant_message_id TEXT NOT NULL,
            origin_run_id TEXT NOT NULL CHECK (
                length(CAST(origin_run_id AS BLOB)) BETWEEN 1 AND 1024
            ),
            call_id TEXT NOT NULL CHECK (
                length(CAST(call_id AS BLOB)) BETWEEN 1 AND 1024
            ),
            project_id TEXT,
            command_projection TEXT NOT NULL CHECK (
                length(CAST(command_projection AS BLOB)) BETWEEN 1 AND 65536
            ),
            cwd_projection TEXT NOT NULL CHECK (
                length(CAST(cwd_projection AS BLOB)) BETWEEN 1 AND 8192
            ),
            command_digest TEXT NOT NULL CHECK (
                length(command_digest) = 71
                AND substr(command_digest, 1, 7) = 'sha256:'
                AND substr(command_digest, 8) NOT GLOB '*[^0-9a-f]*'
            ),
            authorization_source TEXT NOT NULL CHECK (
                authorization_source IN ('automatic', 'explicit_user')
            ),
            approval_provenance_json TEXT NOT NULL CHECK (
                json_valid(approval_provenance_json)
                AND length(CAST(approval_provenance_json AS BLOB)) BETWEEN 2 AND 65536
            ),
            permission_provenance_json TEXT NOT NULL CHECK (
                json_valid(permission_provenance_json)
                AND length(CAST(permission_provenance_json AS BLOB)) BETWEEN 2 AND 65536
            ),
            status TEXT NOT NULL CHECK (
                status IN (
                    'starting', 'running', 'exited', 'interrupted',
                    'timed_out', 'failed', 'outcome_unknown'
                )
            ),
            started_at INTEGER NOT NULL CHECK (started_at >= 0),
            ended_at INTEGER CHECK (ended_at IS NULL OR ended_at >= started_at),
            exit_code INTEGER,
            latest_sequence INTEGER NOT NULL DEFAULT 0 CHECK (latest_sequence >= 0),
            model_read_sequence INTEGER NOT NULL DEFAULT 0 CHECK (
                model_read_sequence >= 0 AND model_read_sequence <= latest_sequence
            ),
            transcript_truncated INTEGER NOT NULL DEFAULT 0 CHECK (
                transcript_truncated IN (0, 1)
            ),
            output_capture_truncated INTEGER NOT NULL DEFAULT 0 CHECK (
                output_capture_truncated IN (0, 1)
            ),
            archive_ref TEXT,
            terminal_reason TEXT CHECK (
                terminal_reason IS NULL
                OR length(CAST(terminal_reason AS BLOB)) BETWEEN 1 AND 8192
            ),
            created_at INTEGER NOT NULL CHECK (created_at >= 0),
            updated_at INTEGER NOT NULL CHECK (updated_at >= created_at),
            settled_at INTEGER CHECK (
                settled_at IS NULL OR settled_at >= created_at
            ),
            UNIQUE (conversation_id, assistant_message_id, call_id),
            CHECK (
                (
                    status IN ('starting', 'running')
                    AND ended_at IS NULL
                    AND exit_code IS NULL
                    AND settled_at IS NULL
                ) OR (
                    status NOT IN ('starting', 'running')
                    AND ended_at IS NOT NULL
                    AND settled_at IS NOT NULL
                )
            ),
            CHECK (status = 'exited' OR exit_code IS NULL),
            FOREIGN KEY (conversation_id) REFERENCES conversations(id) ON DELETE CASCADE,
            FOREIGN KEY (assistant_message_id) REFERENCES messages(id) ON DELETE CASCADE,
            FOREIGN KEY (project_id) REFERENCES projects(id) ON DELETE CASCADE,
            FOREIGN KEY (archive_ref)
                REFERENCES conversation_history_blobs(archive_ref) ON DELETE SET NULL
        );
CREATE TABLE agent_command_session_output_chunks (
            session_id TEXT NOT NULL,
            sequence INTEGER NOT NULL CHECK (sequence > 0),
            stream TEXT NOT NULL CHECK (stream IN ('stdout', 'stderr')),
            output TEXT NOT NULL CHECK (
                length(CAST(output AS BLOB)) BETWEEN 1 AND 65536
            ),
            output_bytes INTEGER NOT NULL CHECK (
                output_bytes = length(CAST(output AS BLOB))
            ),
            created_at INTEGER NOT NULL CHECK (created_at >= 0),
            PRIMARY KEY (session_id, sequence),
            FOREIGN KEY (session_id)
                REFERENCES agent_command_sessions(session_id) ON DELETE CASCADE
        );
CREATE TABLE agent_command_session_published_outputs (
            session_id TEXT NOT NULL,
            ordinal INTEGER NOT NULL CHECK (ordinal >= 0 AND ordinal < 32),
            name TEXT NOT NULL CHECK (
                length(CAST(name AS BLOB)) BETWEEN 1 AND 1024
            ),
            kind TEXT NOT NULL CHECK (kind IN ('image', 'document')),
            read_path TEXT NOT NULL CHECK (
                length(CAST(read_path AS BLOB)) BETWEEN 1 AND 1024
            ),
            mime_type TEXT NOT NULL CHECK (
                length(CAST(mime_type AS BLOB)) BETWEEN 1 AND 128
            ),
            size_bytes INTEGER NOT NULL CHECK (size_bytes > 0),
            sha256 TEXT NOT NULL CHECK (
                length(sha256) = 64
                AND sha256 NOT GLOB '*[^0-9a-f]*'
            ),
            width INTEGER CHECK (width IS NULL OR width BETWEEN 1 AND 16384),
            height INTEGER CHECK (height IS NULL OR height BETWEEN 1 AND 16384),
            PRIMARY KEY (session_id, ordinal),
            FOREIGN KEY (session_id)
                REFERENCES agent_command_sessions(session_id) ON DELETE CASCADE
        );
CREATE TABLE agent_command_session_model_read_receipts (
            receipt_id INTEGER PRIMARY KEY AUTOINCREMENT,
            conversation_id TEXT NOT NULL,
            session_id TEXT NOT NULL,
            run_id TEXT NOT NULL CHECK (
                length(CAST(run_id AS BLOB)) BETWEEN 1 AND 1024
            ),
            call_id TEXT NOT NULL CHECK (
                length(CAST(call_id AS BLOB)) BETWEEN 1 AND 1024
            ),
            action TEXT NOT NULL CHECK (action IN ('poll', 'interrupt')),
            max_output_bytes INTEGER NOT NULL CHECK (max_output_bytes > 0),
            requested_after_sequence INTEGER NOT NULL CHECK (
                requested_after_sequence >= 0
            ),
            first_output_sequence INTEGER CHECK (
                first_output_sequence IS NULL OR first_output_sequence > 0
            ),
            last_output_sequence INTEGER CHECK (
                last_output_sequence IS NULL OR last_output_sequence > 0
            ),
            status TEXT NOT NULL CHECK (
                status IN (
                    'starting', 'running', 'exited', 'interrupted',
                    'timed_out', 'failed', 'outcome_unknown'
                )
            ),
            exit_code INTEGER,
            latest_sequence INTEGER NOT NULL CHECK (latest_sequence >= 0),
            truncated_before INTEGER NOT NULL CHECK (truncated_before IN (0, 1)),
            output_truncated INTEGER NOT NULL CHECK (output_truncated IN (0, 1)),
            output_bytes INTEGER NOT NULL CHECK (output_bytes >= 0),
            output_hash TEXT NOT NULL CHECK (
                length(output_hash) = 64
                AND output_hash NOT GLOB '*[^0-9a-f]*'
            ),
            output_payload_compression TEXT NOT NULL CHECK (
                output_payload_compression = 'zstd_json_v1'
            ),
            output_payload BLOB NOT NULL CHECK (
                length(output_payload) BETWEEN 1 AND 16777216
            ),
            output_chunk_count INTEGER NOT NULL CHECK (output_chunk_count >= 0),
            created_at INTEGER NOT NULL CHECK (created_at >= 0),
            UNIQUE (conversation_id, session_id, run_id, call_id),
            CHECK (
                (first_output_sequence IS NULL AND last_output_sequence IS NULL)
                OR (
                    first_output_sequence IS NOT NULL
                    AND last_output_sequence IS NOT NULL
                    AND first_output_sequence > requested_after_sequence
                    AND first_output_sequence <= last_output_sequence
                    AND last_output_sequence <= latest_sequence
                )
            ),
            CHECK (status = 'exited' OR exit_code IS NULL),
            FOREIGN KEY (session_id)
                REFERENCES agent_command_sessions(session_id) ON DELETE CASCADE
        );
CREATE TABLE agent_command_session_lifecycle_events (
            event_id INTEGER PRIMARY KEY AUTOINCREMENT,
            session_id TEXT NOT NULL,
            phase TEXT NOT NULL CHECK (phase IN ('started', 'terminal')),
            conversation_id TEXT NOT NULL,
            assistant_message_id TEXT NOT NULL,
            call_id TEXT NOT NULL,
            event_json TEXT NOT NULL CHECK (
                json_valid(event_json)
                AND length(CAST(event_json AS BLOB)) BETWEEN 2 AND 65536
            ),
            archive_ref TEXT,
            created_at INTEGER NOT NULL CHECK (created_at >= 0),
            recorded_at INTEGER NOT NULL CHECK (recorded_at >= 0),
            trace_sequence INTEGER CHECK (trace_sequence IS NULL OR trace_sequence >= 0),
            materialized_at INTEGER CHECK (
                materialized_at IS NULL OR materialized_at >= recorded_at
            ),
            UNIQUE (session_id, phase),
            CHECK (
                (trace_sequence IS NULL AND materialized_at IS NULL)
                OR (trace_sequence IS NOT NULL AND materialized_at IS NOT NULL)
            ),
            FOREIGN KEY (session_id)
                REFERENCES agent_command_sessions(session_id) ON DELETE CASCADE,
            FOREIGN KEY (conversation_id)
                REFERENCES conversations(id) ON DELETE CASCADE,
            FOREIGN KEY (assistant_message_id)
                REFERENCES messages(id) ON DELETE CASCADE,
            FOREIGN KEY (archive_ref)
                REFERENCES conversation_history_blobs(archive_ref) ON DELETE CASCADE
        );
CREATE INDEX agent_command_sessions_conversation_state
            ON agent_command_sessions(conversation_id, status, updated_at DESC);
CREATE INDEX agent_command_sessions_project_state
            ON agent_command_sessions(project_id, status, updated_at DESC);
CREATE INDEX agent_command_sessions_origin_run
            ON agent_command_sessions(origin_run_id, started_at);
CREATE INDEX agent_command_session_model_receipts_retention
            ON agent_command_session_model_read_receipts(session_id, receipt_id DESC);
CREATE INDEX agent_command_session_lifecycle_pending
            ON agent_command_session_lifecycle_events(
                assistant_message_id, trace_sequence, created_at, event_id
            );
CREATE TRIGGER validate_agent_command_session_message_insert
        BEFORE INSERT ON agent_command_sessions
        WHEN NOT EXISTS (
            SELECT 1 FROM messages
            WHERE id = NEW.assistant_message_id
              AND conversation_id = NEW.conversation_id
              AND role = 'assistant'
        )
        BEGIN
            SELECT RAISE(
                ABORT,
                'command session message must be an assistant message in the same conversation'
            );
        END;
CREATE TRIGGER validate_agent_command_session_project_insert
        BEFORE INSERT ON agent_command_sessions
        WHEN NEW.project_id IS NOT NULL
          AND NOT EXISTS (
              SELECT 1 FROM conversations
              WHERE id = NEW.conversation_id AND project_id = NEW.project_id
          )
        BEGIN
            SELECT RAISE(
                ABORT,
                'command session project must match its conversation project'
            );
        END;
CREATE TRIGGER prevent_agent_command_session_identity_update
        BEFORE UPDATE OF
            session_id, schema_version, conversation_id, assistant_message_id,
            origin_run_id, call_id, project_id, command_projection, cwd_projection,
            command_digest, authorization_source, approval_provenance_json,
            permission_provenance_json, started_at
        ON agent_command_sessions
        BEGIN
            SELECT RAISE(ABORT, 'command session identity and provenance are immutable');
        END;
CREATE TRIGGER validate_agent_command_session_archive_insert
        BEFORE INSERT ON agent_command_sessions
        WHEN NEW.archive_ref IS NOT NULL
          AND NOT EXISTS (
              SELECT 1 FROM conversation_history_blobs
              WHERE archive_ref = NEW.archive_ref
                AND conversation_id = NEW.conversation_id
                AND assistant_message_id = NEW.assistant_message_id
          )
        BEGIN
            SELECT RAISE(
                ABORT,
                'command session archive must belong to its conversation and assistant message'
            );
        END;
CREATE TRIGGER validate_agent_command_session_archive_update
        BEFORE UPDATE OF archive_ref ON agent_command_sessions
        WHEN NEW.archive_ref IS NOT NULL
          AND NOT EXISTS (
              SELECT 1 FROM conversation_history_blobs
              WHERE archive_ref = NEW.archive_ref
                AND conversation_id = NEW.conversation_id
                AND assistant_message_id = NEW.assistant_message_id
          )
        BEGIN
            SELECT RAISE(
                ABORT,
                'command session archive must belong to its conversation and assistant message'
            );
        END;
CREATE TRIGGER validate_agent_command_session_output_active
        BEFORE INSERT ON agent_command_session_output_chunks
        WHEN NOT EXISTS (
            SELECT 1 FROM agent_command_sessions
            WHERE session_id = NEW.session_id
              AND status IN ('starting', 'running')
        )
        BEGIN
            SELECT RAISE(ABORT, 'command session output cannot append after settlement');
        END;
CREATE TRIGGER validate_agent_command_session_model_receipt_owner
        BEFORE INSERT ON agent_command_session_model_read_receipts
        WHEN NOT EXISTS (
            SELECT 1 FROM agent_command_sessions
            WHERE session_id = NEW.session_id
              AND conversation_id = NEW.conversation_id
        )
        BEGIN
            SELECT RAISE(ABORT, 'command session model receipt owner is invalid');
        END;
CREATE TRIGGER validate_agent_command_session_lifecycle_owner
        BEFORE INSERT ON agent_command_session_lifecycle_events
        WHEN NOT EXISTS (
            SELECT 1 FROM agent_command_sessions
            WHERE session_id = NEW.session_id
              AND conversation_id = NEW.conversation_id
              AND assistant_message_id = NEW.assistant_message_id
              AND call_id = NEW.call_id
        )
        BEGIN
            SELECT RAISE(ABORT, 'command session lifecycle owner is invalid');
        END;
CREATE TRIGGER validate_agent_command_session_lifecycle_archive
        BEFORE INSERT ON agent_command_session_lifecycle_events
        WHEN NEW.archive_ref IS NOT NULL
          AND NOT EXISTS (
              SELECT 1 FROM conversation_history_blobs
              WHERE archive_ref = NEW.archive_ref
                AND conversation_id = NEW.conversation_id
                AND assistant_message_id = NEW.assistant_message_id
          )
        BEGIN
            SELECT RAISE(ABORT, 'command session lifecycle archive owner is invalid');
        END;
CREATE TRIGGER prevent_agent_command_session_lifecycle_rewrite
        BEFORE UPDATE OF
            session_id, phase, conversation_id, assistant_message_id,
            call_id, event_json, archive_ref, created_at, recorded_at
        ON agent_command_session_lifecycle_events
        BEGIN
            SELECT RAISE(ABORT, 'command session lifecycle events are append-only');
        END;
CREATE TRIGGER validate_agent_command_session_model_receipt_payload_insert
         BEFORE INSERT ON agent_command_session_model_read_receipts
         WHEN NEW.output_payload_compression IS NULL
           OR NEW.output_payload IS NULL
           OR NEW.output_chunk_count IS NULL
         BEGIN
             SELECT RAISE(ABORT, 'command session model receipt payload is required');
        END;

-- Root-scoped durable collaboration invalidation log. Domain tables remain authoritative; this
-- append-only outbox gives Host/renderer a crash-safe monotonic sequence for duplicate/gap
-- detection. SQLite write serialization makes the counter increment and event insert atomic with
-- the state mutation which fired the trigger.
CREATE TABLE agent_collaboration_event_sequences (
            root_agent_id TEXT PRIMARY KEY,
            next_sequence INTEGER NOT NULL CHECK (next_sequence > 0),
            FOREIGN KEY (root_agent_id) REFERENCES agent_nodes(agent_id) ON DELETE CASCADE
        );
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
                activity_schema_version IS NULL OR activity_schema_version = 1
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
            created_at INTEGER NOT NULL CHECK (created_at >= 0),
            UNIQUE(root_agent_id, root_sequence),
            CHECK (workspace_id IS project_id),
            CHECK (
                (activity_schema_version IS NULL
                    AND activity_semantic IS NULL
                    AND activity_agent_id IS NULL
                    AND activity_task_name_snapshot IS NULL
                    AND activity_root_anchor_message_id IS NULL)
                OR (activity_schema_version = 1
                    AND activity_semantic IS NOT NULL
                    AND activity_agent_id IS NOT NULL
                    AND activity_task_name_snapshot IS NOT NULL)
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
            NEW.activity_root_anchor_message_id IS NOT NULL
            AND NOT EXISTS (
                SELECT 1 FROM messages AS anchor
                WHERE anchor.id = NEW.activity_root_anchor_message_id
                  AND anchor.conversation_id = NEW.root_conversation_id
                  AND anchor.role = 'assistant'
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
                activity_root_anchor_message_id, created_at
            ) SELECT
                'collab-event:' || lower(hex(randomblob(16))), 2, NEW.root_agent_id,
                sequence_state.next_sequence - 1, recipient.project_id, recipient.project_id,
                recipient.root_conversation_id, recipient.agent_id, recipient.conversation_id,
                NULL, NULL, NEW.message_id, 'mailbox_enqueued', NEW.sequence,
                CASE
                    WHEN NEW.kind = 'message' AND sender.parent_agent_id = recipient.agent_id
                    THEN 1 ELSE NULL
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
                NULL, NEW.created_at
            FROM agent_nodes AS recipient
            JOIN agent_nodes AS sender
              ON sender.agent_id = NEW.sender_agent_id
             AND sender.root_agent_id = NEW.root_agent_id
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
                activity_root_anchor_message_id, created_at
            ) SELECT
                'collab-event:' || lower(hex(randomblob(16))), 2, NEW.root_agent_id,
                sequence_state.next_sequence - 1, target.project_id, target.project_id,
                target.root_conversation_id, target.agent_id, target.conversation_id,
                NEW.assistant_message_id, NEW.run_id, NEW.source_agent_message_id,
                'wake_created', NEW.status_revision,
                CASE WHEN source.kind IN ('task', 'followup') THEN 1 ELSE NULL END,
                CASE WHEN source.kind IN ('task', 'followup') THEN 'started' ELSE NULL END,
                CASE WHEN source.kind IN ('task', 'followup') THEN target.agent_id ELSE NULL END,
                CASE WHEN source.kind IN ('task', 'followup') THEN target.task_name ELSE NULL END,
                NULL, NEW.created_at
            FROM agent_nodes AS target
            LEFT JOIN agent_mailbox_messages AS source
              ON source.message_id = NEW.source_agent_message_id
             AND source.root_agent_id = NEW.root_agent_id
             AND source.recipient_agent_id = NEW.agent_id
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
                activity_root_anchor_message_id, created_at
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
                    THEN 1 ELSE NULL
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
                NULL,
                COALESCE(NEW.completed_at, NEW.started_at, NEW.claimed_at, NEW.created_at)
            FROM agent_nodes AS target
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
        WHEN EXISTS (
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
                activity_root_anchor_message_id, created_at
            ) SELECT
                'collab-event:' || lower(hex(randomblob(16))), 2, node.root_agent_id,
                sequence_state.next_sequence - 1, node.project_id, node.project_id,
                node.root_conversation_id, node.agent_id, node.conversation_id,
                NEW.assistant_message_id, NEW.run_id, NEW.assistant_message_id,
                'approval_projected', CASE WHEN NEW.updated_at > 0 THEN NEW.updated_at ELSE 1 END,
                1, 'waiting_approval', node.agent_id, node.task_name, NULL, NEW.created_at
            FROM agent_nodes AS node
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
CREATE TRIGGER prevent_agent_command_session_model_receipt_update
         BEFORE UPDATE ON agent_command_session_model_read_receipts
         BEGIN
             SELECT RAISE(ABORT, 'command session model read receipts are immutable');
         END;
CREATE VIRTUAL TABLE conversation_history_fts USING fts5(
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
