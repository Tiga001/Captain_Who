use std::path::{Path, PathBuf};

struct ForbiddenMarker {
    value: &'static str,
    allowed_files: &'static [&'static str],
}

const FORBIDDEN_MARKERS: &[ForbiddenMarker] = &[
    ForbiddenMarker {
        value: "create_goal",
        allowed_files: &[],
    },
    ForbiddenMarker {
        value: "get_goal",
        allowed_files: &[],
    },
    ForbiddenMarker {
        value: "update_goal",
        allowed_files: &[],
    },
    ForbiddenMarker {
        value: "ConversationGoal",
        allowed_files: &[],
    },
    ForbiddenMarker {
        value: "conversation_goals",
        allowed_files: &[],
    },
    ForbiddenMarker {
        value: "conversation_goal_revisions",
        allowed_files: &[],
    },
    ForbiddenMarker {
        value: "goalTokens",
        allowed_files: &[],
    },
    ForbiddenMarker {
        value: "goal_tokens",
        allowed_files: &[],
    },
    ForbiddenMarker {
        value: "throughAssistantMessageId",
        allowed_files: &[],
    },
    ForbiddenMarker {
        value: "pub through_assistant_message_id:",
        allowed_files: &[],
    },
    ForbiddenMarker {
        value: "ForkConversationInput",
        allowed_files: &[],
    },
    ForbiddenMarker {
        value: "ParsedSkillUninstallRequest::Legacy",
        allowed_files: &[],
    },
    ForbiddenMarker {
        value: "ManagedSkillLegacyUninstallRequest",
        allowed_files: &[],
    },
    ForbiddenMarker {
        value: "uninstall_legacy",
        allowed_files: &[],
    },
    ForbiddenMarker {
        value: "LegacyPackageRevisionConflict",
        allowed_files: &[],
    },
    ForbiddenMarker {
        value: "CONTEXT_CONTINUITY_V1_SCHEMA_VERSION",
        allowed_files: &[],
    },
    ForbiddenMarker {
        value: "LegacyContinuityIndex",
        allowed_files: &[],
    },
    ForbiddenMarker {
        value: "validate_legacy_profile_echo",
        allowed_files: &[],
    },
    ForbiddenMarker {
        value: "ProviderProfileConfig::resolve(",
        allowed_files: &[],
    },
    ForbiddenMarker {
        value: "COMMAND_RUNTIME_PROFILE_ERROR_LEGACY_REPREPARE",
        allowed_files: &[],
    },
    ForbiddenMarker {
        value: "AgentCommandRuntimeRequest",
        allowed_files: &[],
    },
    ForbiddenMarker {
        value: "providerProfileUpdate?: StorageProviderProfileUpdate",
        allowed_files: &[],
    },
    ForbiddenMarker {
        value: "providerProfileConfig?: ProviderProfileConfig",
        allowed_files: &[],
    },
    ForbiddenMarker {
        value: "permissionModeVersion?: number",
        allowed_files: &[],
    },
    ForbiddenMarker {
        value: "queuedMessagesJson?: string",
        allowed_files: &[],
    },
    ForbiddenMarker {
        value: "command.remove(\"runtime\")",
        allowed_files: &[],
    },
    ForbiddenMarker {
        value: "fn decode_current_pending_checkpoint(",
        allowed_files: &[],
    },
    ForbiddenMarker {
        value: "hydrate_legacy_model_history",
        allowed_files: &[],
    },
    ForbiddenMarker {
        value: "append_reconstructed_conversation_model_context",
        allowed_files: &[],
    },
    ForbiddenMarker {
        value: "PendingActionResumeCheckpointProjection",
        allowed_files: &[],
    },
    ForbiddenMarker {
        value: "PendingActionReconciliationCheckpointProjection",
        allowed_files: &[],
    },
    ForbiddenMarker {
        value: "LegacyManualFileEffectRecovery",
        allowed_files: &[],
    },
    ForbiddenMarker {
        value: "recover_legacy_manual_file_effect_terminal_settlement",
        allowed_files: &[],
    },
    ForbiddenMarker {
        value: "pub(crate) fn tool_call_for_action(",
        allowed_files: &[],
    },
    ForbiddenMarker {
        value: "PERSISTED_AGENT_RESUME_INPUT_SCHEMA_VERSION: u32 = 5",
        allowed_files: &[],
    },
    ForbiddenMarker {
        value: "PERSISTED_AGENT_RESUME_INPUT_SCHEMA_VERSION: u32 = 6",
        allowed_files: &[],
    },
    ForbiddenMarker {
        value: "PERSISTED_AGENT_RESUME_INPUT_SCHEMA_VERSION: u32 = 8",
        allowed_files: &[],
    },
    ForbiddenMarker {
        value: "MODEL_REQUEST_OBSERVATION_SCHEMA_VERSION: u32 = 2",
        allowed_files: &[],
    },
    ForbiddenMarker {
        value: "pub fn commit_pending_agent_action_audited_result_trace(",
        allowed_files: &[],
    },
    ForbiddenMarker {
        value: "alias = \"arguments\"",
        allowed_files: &[],
    },
    ForbiddenMarker {
        value: "arguments?: never",
        allowed_files: &[],
    },
    ForbiddenMarker {
        value: "parameters?: never; arguments:",
        allowed_files: &[],
    },
    ForbiddenMarker {
        value: "OfficeRequestParameters",
        allowed_files: &[],
    },
    ForbiddenMarker {
        value: "pub document_precondition:",
        allowed_files: &[],
    },
    ForbiddenMarker {
        value: "pub output_precondition:",
        allowed_files: &[],
    },
    ForbiddenMarker {
        value: "pub destination_precondition:",
        allowed_files: &[],
    },
    ForbiddenMarker {
        value: "pub resource_preconditions:",
        allowed_files: &[],
    },
    ForbiddenMarker {
        value: "LegacyOrUnsupported",
        allowed_files: &[],
    },
    ForbiddenMarker {
        value: "LlmAssistantTurn::from_legacy",
        allowed_files: &[],
    },
    ForbiddenMarker {
        value: "LegacySplit",
        allowed_files: &[],
    },
    ForbiddenMarker {
        value: "LegacyEffectiveCalls",
        allowed_files: &[],
    },
    ForbiddenMarker {
        value: "LegacyTextFallbackAllowed",
        allowed_files: &[],
    },
    ForbiddenMarker {
        value: "project_legacy_generic_exchange",
        allowed_files: &[],
    },
    ForbiddenMarker {
        value: "add_column_if_missing",
        allowed_files: &[],
    },
    ForbiddenMarker {
        value: "table_has_column",
        allowed_files: &[],
    },
    ForbiddenMarker {
        value: "run_authorized_command(",
        allowed_files: &[],
    },
    ForbiddenMarker {
        value: "run_authorized_command_with_output_observer",
        allowed_files: &[],
    },
    ForbiddenMarker {
        value: "run_authorized_command_with_artifact_runtime(",
        allowed_files: &[],
    },
    ForbiddenMarker {
        value: "run_authorized_command_with_artifact_runtime_and_inputs(",
        allowed_files: &[],
    },
    ForbiddenMarker {
        value: "run_authorized_command_with_artifact_runtime_and_inputs_with_output_observer",
        allowed_files: &[],
    },
    ForbiddenMarker {
        value: "DEFAULT_TIMEOUT_MS",
        allowed_files: &[],
    },
    ForbiddenMarker {
        value: "format!(\"legacy:",
        allowed_files: &[],
    },
];

const FORBIDDEN_IDENTIFIERS: &[&str] = &["SkillUninstallRequest"];

#[test]
fn retired_development_compatibility_interfaces_cannot_reenter_production() {
    let workspace_root = workspace_root();
    let mut sources = Vec::new();
    for relative_root in [
        "crates/core/src",
        "crates/core-server/src",
        "crates/protocol-rs/src",
        "packages/protocol/src",
        "packages/host-api/src",
        "src/main",
        "src/preload",
        "src/renderer/src",
    ] {
        collect_sources(&workspace_root.join(relative_root), &mut sources);
    }

    let mut violations = Vec::new();
    for path in sources {
        let relative = normalized_relative(&workspace_root, &path);
        if is_test_source(&relative) {
            continue;
        }
        let source = std::fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("failed to read {relative}: {error}"));
        let production = if relative.ends_with(".rs") {
            rust_without_cfg_test_modules(&source)
        } else {
            source
        };
        for rule in FORBIDDEN_MARKERS {
            if rule.allowed_files.contains(&relative.as_str()) {
                continue;
            }
            for (index, line) in production.lines().enumerate() {
                if line.contains(rule.value) {
                    violations.push(format!(
                        "{relative}:{} contains retired marker `{}`",
                        index + 1,
                        rule.value
                    ));
                }
            }
        }
        for identifier in FORBIDDEN_IDENTIFIERS {
            for (index, line) in production.lines().enumerate() {
                if contains_identifier(line, identifier) {
                    violations.push(format!(
                        "{relative}:{} contains retired identifier `{identifier}`",
                        index + 1
                    ));
                }
            }
        }
    }

    assert!(
        violations.is_empty(),
        "development-era compatibility escaped into current production sources:\n{}\n\
         Add only the current strict protocol; do not expand the allowlist.",
        violations.join("\n")
    );
}

fn contains_identifier(line: &str, identifier: &str) -> bool {
    line.match_indices(identifier).any(|(start, _)| {
        let end = start + identifier.len();
        let before = line[..start].chars().next_back();
        let after = line[end..].chars().next();
        before.is_none_or(|character| !is_identifier_character(character))
            && after.is_none_or(|character| !is_identifier_character(character))
    })
}

fn is_identifier_character(character: char) -> bool {
    character == '_' || character.is_ascii_alphanumeric()
}

#[test]
fn whole_settings_revision_marker_is_confined_to_the_current_document_identity() {
    let root = workspace_root();
    let mut sources = Vec::new();
    for relative_root in [
        "crates/core/src",
        "crates/core-server/src",
        "crates/protocol-rs/src",
        "packages/protocol/src",
        "packages/host-api/src",
        "src/main",
        "src/preload",
        "src/renderer/src",
    ] {
        collect_sources(&root.join(relative_root), &mut sources);
    }

    let mut occurrences = Vec::new();
    for path in sources {
        let relative = normalized_relative(&root, &path);
        if is_test_source(&relative) {
            continue;
        }
        let source = std::fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("failed to read {relative}: {error}"));
        let production = if relative.ends_with(".rs") {
            rust_without_cfg_test_modules(&source)
        } else {
            source
        };
        for line in production.lines().map(str::trim) {
            if line.contains("model-settings-v1") {
                occurrences.push((relative.clone(), line.to_string()));
            }
        }
    }
    occurrences.sort();
    assert_eq!(
        occurrences,
        vec![
            (
                "crates/core/src/storage/canonical_schema.sql".to_string(),
                "configuration_revision GLOB 'model-settings-v1:?*'".to_string(),
            ),
            (
                "crates/core/src/storage/config_repository.rs".to_string(),
                "const MODEL_SETTINGS_REVISION_PREFIX: &str = \"model-settings-v1:\";".to_string(),
            ),
            (
                "packages/protocol/src/storageParsers.ts".to_string(),
                "!/^model-settings-v1:[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/.test("
                    .to_string(),
            ),
        ],
        "the whole-settings revision must never be accepted as Provider protocol provenance",
    );
}

#[test]
fn llm_retry_wire_has_no_provider_authored_reason_field() {
    let root = workspace_root();
    assert_section_excludes(
        &root.join("crates/core/src/protocol.rs"),
        "    LlmRetry {",
        "    ToolInputProgress {",
        "reason",
    );
    assert_section_excludes(
        &root.join("packages/protocol/src/agent.ts"),
        "      type: 'llm_retry'",
        "      type: 'tool_input_progress'",
        "reason",
    );
    assert_section_excludes(
        &root.join("packages/protocol/src/agentParsers/eventPayloads.ts"),
        "function parseAgentLlmRetryEvent(",
        "function parseAgentMessageStreamResetEvent(",
        "reason",
    );
}

#[test]
fn persisted_resume_envelope_has_no_inert_agent_chat_compatibility_fields() {
    let path =
        workspace_root().join("crates/core-server/src/application/agent/persisted_resume_input.rs");
    for retired_field in [
        "approval_decision:",
        "tool_continuation:",
        "attachments:",
        "messages:",
    ] {
        assert_section_excludes(
            &path,
            "pub(super) struct PersistedAgentResumeInput {",
            "struct PersistedAgentSearchConfig {",
            retired_field,
        );
    }
}

#[test]
fn canonical_schema_bootstrap_contains_no_incremental_upgrade_helpers() {
    let migrations =
        std::fs::read_to_string(workspace_root().join("crates/core/src/storage/migrations.rs"))
            .expect("migrations source must be readable");
    let production = rust_without_cfg_test_modules(&migrations);
    for forbidden in ["ALTER TABLE", "fn upgrade_", "fn backfill_"] {
        assert!(
            !production.contains(forbidden),
            "canonical schema bootstrap contains retired incremental migration marker `{forbidden}`"
        );
    }
}

#[test]
fn mcp_approval_mode_remains_a_required_current_configuration_field() {
    let config = std::fs::read_to_string(workspace_root().join("crates/mcp-client/src/config.rs"))
        .expect("MCP config source must be readable");
    assert_section_excludes(
        &workspace_root().join("crates/mcp-client/src/config.rs"),
        "pub struct McpServerConfig {",
        "impl McpServerConfig {",
        "serde(default)",
    );
    assert!(
        config.contains("pub approval_mode: McpApprovalMode"),
        "current MCP server config must explicitly carry approval_mode"
    );
}

fn assert_section_excludes(path: &Path, start: &str, end: &str, forbidden: &str) {
    let source = std::fs::read_to_string(path)
        .unwrap_or_else(|error| panic!("failed to read {}: {error}", path.display()));
    let start_index = source
        .find(start)
        .unwrap_or_else(|| panic!("missing section start `{start}` in {}", path.display()));
    let remainder = &source[start_index..];
    let end_index = remainder
        .find(end)
        .unwrap_or_else(|| panic!("missing section end `{end}` in {}", path.display()));
    let section = &remainder[..end_index];
    assert!(
        !section.contains(forbidden),
        "current wire section in {} contains retired field `{forbidden}`",
        path.display()
    );
}

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("core-server crate must live under workspace/crates")
        .to_path_buf()
}

fn collect_sources(root: &Path, output: &mut Vec<PathBuf>) {
    let mut entries = std::fs::read_dir(root)
        .unwrap_or_else(|error| panic!("failed to read {}: {error}", root.display()))
        .collect::<Result<Vec<_>, _>>()
        .unwrap_or_else(|error| panic!("failed to enumerate {}: {error}", root.display()));
    entries.sort_by_key(std::fs::DirEntry::file_name);
    for entry in entries {
        let path = entry.path();
        if path.is_dir() {
            collect_sources(&path, output);
        } else if matches!(
            path.extension().and_then(|extension| extension.to_str()),
            Some("rs" | "ts" | "tsx" | "sql")
        ) {
            output.push(path);
        }
    }
}

fn normalized_relative(workspace_root: &Path, path: &Path) -> String {
    path.strip_prefix(workspace_root)
        .expect("source path must remain inside workspace")
        .to_string_lossy()
        .replace('\\', "/")
}

fn is_test_source(relative: &str) -> bool {
    relative.contains("/tests/")
        || relative.ends_with("/tests.rs")
        || relative.ends_with(".test.ts")
        || relative.ends_with(".test.tsx")
}

fn rust_without_cfg_test_modules(source: &str) -> String {
    const CFG_TEST: &str = "#[cfg(test)]";
    let mut output = String::with_capacity(source.len());
    let mut cursor = 0;
    while let Some(relative_start) = source[cursor..].find(CFG_TEST) {
        let start = cursor + relative_start;
        let after_attribute = start + CFG_TEST.len();
        let item_start = skip_ascii_whitespace(source, after_attribute);
        if !source[item_start..].starts_with("mod ") {
            output.push_str(&source[cursor..after_attribute]);
            cursor = after_attribute;
            continue;
        }
        let Some(relative_open) = source[item_start..].find('{') else {
            output.push_str(&source[cursor..after_attribute]);
            cursor = after_attribute;
            continue;
        };
        let open = item_start + relative_open;
        let Some(end) = matching_rust_brace(source, open) else {
            panic!("unterminated #[cfg(test)] module in architecture source scan");
        };
        output.push_str(&source[cursor..start]);
        output.extend(
            source[start..end]
                .chars()
                .filter(|character| *character == '\n'),
        );
        cursor = end;
    }
    output.push_str(&source[cursor..]);
    output
}

fn skip_ascii_whitespace(source: &str, mut cursor: usize) -> usize {
    let bytes = source.as_bytes();
    while cursor < bytes.len() && bytes[cursor].is_ascii_whitespace() {
        cursor += 1;
    }
    cursor
}

fn matching_rust_brace(source: &str, open: usize) -> Option<usize> {
    let bytes = source.as_bytes();
    let mut cursor = open;
    let mut depth = 0_u32;
    while cursor < bytes.len() {
        match bytes[cursor] {
            b'/' if bytes.get(cursor + 1) == Some(&b'/') => {
                cursor += 2;
                while cursor < bytes.len() && bytes[cursor] != b'\n' {
                    cursor += 1;
                }
            }
            b'/' if bytes.get(cursor + 1) == Some(&b'*') => {
                cursor = skip_block_comment(bytes, cursor + 2)?;
            }
            b'"' => cursor = skip_quoted(bytes, cursor + 1, b'"')?,
            b'\'' if is_char_literal(bytes, cursor) => {
                cursor = skip_quoted(bytes, cursor + 1, b'\'')?;
            }
            b'r' if raw_string_hash_count(bytes, cursor).is_some() => {
                let hashes = raw_string_hash_count(bytes, cursor)?;
                cursor = skip_raw_string(bytes, cursor, hashes)?;
            }
            b'{' => {
                depth = depth.checked_add(1)?;
                cursor += 1;
            }
            b'}' => {
                depth = depth.checked_sub(1)?;
                cursor += 1;
                if depth == 0 {
                    return Some(cursor);
                }
            }
            _ => cursor += 1,
        }
    }
    None
}

fn skip_block_comment(bytes: &[u8], mut cursor: usize) -> Option<usize> {
    let mut depth = 1_u32;
    while cursor < bytes.len() {
        if bytes.get(cursor..cursor + 2) == Some(b"/*") {
            depth = depth.checked_add(1)?;
            cursor += 2;
        } else if bytes.get(cursor..cursor + 2) == Some(b"*/") {
            depth = depth.checked_sub(1)?;
            cursor += 2;
            if depth == 0 {
                return Some(cursor);
            }
        } else {
            cursor += 1;
        }
    }
    None
}

fn skip_quoted(bytes: &[u8], mut cursor: usize, quote: u8) -> Option<usize> {
    while cursor < bytes.len() {
        if bytes[cursor] == b'\\' {
            cursor = cursor.checked_add(2)?;
        } else if bytes[cursor] == quote {
            return Some(cursor + 1);
        } else {
            cursor += 1;
        }
    }
    None
}

fn is_char_literal(bytes: &[u8], cursor: usize) -> bool {
    matches!(
        bytes.get(cursor + 1..),
        Some([b'\\', _, b'\'', ..]) | Some([_, b'\'', ..])
    )
}

fn raw_string_hash_count(bytes: &[u8], cursor: usize) -> Option<usize> {
    let mut probe = cursor.checked_add(1)?;
    while bytes.get(probe) == Some(&b'#') {
        probe += 1;
    }
    (bytes.get(probe) == Some(&b'"')).then_some(probe - cursor - 1)
}

fn skip_raw_string(bytes: &[u8], cursor: usize, hashes: usize) -> Option<usize> {
    let mut probe = cursor.checked_add(hashes + 2)?;
    while probe < bytes.len() {
        if bytes[probe] == b'"'
            && bytes
                .get(probe + 1..probe + 1 + hashes)
                .is_some_and(|suffix| suffix.iter().all(|byte| *byte == b'#'))
        {
            return Some(probe + 1 + hashes);
        }
        probe += 1;
    }
    None
}

#[test]
fn cfg_test_module_stripping_keeps_later_production_items() {
    let source = r##"
pub const BEFORE: &str = "safe";
#[cfg(test)]
mod tests {
    const TEST_ONLY: &str = r#"} model-settings-v1 {"#;
}
pub struct ForkConversationInput;
"##;
    let production = rust_without_cfg_test_modules(source);
    assert!(!production.contains("model-settings-v1"));
    assert!(production.contains("ForkConversationInput"));
}
