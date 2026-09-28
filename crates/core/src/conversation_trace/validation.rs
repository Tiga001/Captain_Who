fn validate_narration_binding(
    provider_turn_id: Option<&str>,
    first_tool_call_id: Option<&str>,
) -> Result<(), String> {
    if provider_turn_id.is_some() != first_tool_call_id.is_some() {
        return Err(
            "narration Provider and first Tool identities must be bound together".to_string(),
        );
    }
    if let Some(id) = first_tool_call_id {
        crate::llm::validate_model_tool_call_id(id).map_err(|error| error.to_string())?;
    }
    if let Some(id) = provider_turn_id {
        if id.trim().is_empty() || id.len() > 1_024 || id.chars().any(char::is_control) {
            return Err("narration Provider turn identity is invalid".to_string());
        }
        ensure_no_binary_text("narration Provider turn identity", id)?;
    }
    Ok(())
}

impl ConversationTurnTrace {
    pub fn validate(&self) -> Result<(), String> {
        if self.schema_version != CONVERSATION_TURN_TRACE_SCHEMA_VERSION {
            return Err(format!(
                "unsupported conversation trace schema version: {}",
                self.schema_version
            ));
        }
        if self.run_id.trim().is_empty()
            || self.conversation_id.trim().is_empty()
            || self.assistant_message_id.trim().is_empty()
        {
            return Err("conversation trace identifiers cannot be empty".to_string());
        }
        if self.terminal_status == ConversationTurnTraceTerminalStatus::InProgress
            && self.terminal_error.is_some()
        {
            return Err("in-progress conversation trace cannot have a terminal error".to_string());
        }

        ConversationTraceValidationState::default().append(&self.items, self.terminal_status)
    }

    /// Number of items that form complete exchanges and may be rendered into model context.
    #[must_use]
    pub fn model_context_item_count(&self) -> usize {
        let unresolved_call_index = (self.terminal_status
            == ConversationTurnTraceTerminalStatus::InProgress)
            .then(|| {
                self.items
                    .iter()
                    .enumerate()
                    .rev()
                    .find_map(|(index, item)| {
                        let ConversationTurnTraceItem::ToolCall { call_id, .. } = item else {
                            return None;
                        };
                        let closed = self.items[index + 1..].iter().any(|candidate| {
                            matches!(
                                candidate,
                                ConversationTurnTraceItem::ToolResult {
                                    call_id: result_id,
                                    ..
                                } if result_id == call_id
                            )
                        });
                        (!closed).then_some(index)
                    })
            })
            .flatten();
        self.items[..unresolved_call_index.unwrap_or(self.items.len())]
            .iter()
            .filter(|item| item.is_model_visible())
            .count()
    }

    /// Validates the complete current model-context projection for this trace.
    ///
    /// A prefix is only valid for an actively unresolved final ToolCall. Every closed
    /// model-visible item must otherwise have been atomically persisted with the trace; missing
    /// suffixes are corruption, not a signal to reconstruct history from the lossy audit view.
    pub fn validate_complete_model_context(
        &self,
        items: &[ConversationModelContextItem],
    ) -> Result<(), String> {
        validate_model_context_prefix(self, items)?;
        let unresolved_call_sequence = (self.terminal_status
            == ConversationTurnTraceTerminalStatus::InProgress)
            .then(|| {
                self.items.iter().rev().find_map(|item| {
                    let ConversationTurnTraceItem::ToolCall { sequence, call_id, .. } = item else {
                        return None;
                    };
                    let closed = self.items.iter().any(|candidate| {
                        matches!(candidate, ConversationTurnTraceItem::ToolResult { call_id: result_id, .. } if result_id == call_id)
                    });
                    (!closed).then_some(*sequence)
                })
            })
            .flatten();
        // The exact open call may already be staged durably for crash recovery. Its content was
        // validated above, but neither side of the closed-prefix comparison should include it.
        let actual_sequences = items
            .iter()
            .filter(|item| unresolved_call_sequence != Some(item.sequence))
            .map(|item| item.sequence)
            .collect::<BTreeSet<_>>();
        let expected_sequences = self
            .items
            .iter()
            .filter(|item| item.is_model_visible())
            .filter(|item| unresolved_call_sequence != Some(item.sequence()))
            .map(ConversationTurnTraceItem::sequence)
            .collect::<BTreeSet<_>>();
        if actual_sequences != expected_sequences {
            return Err(
                "conversation model context must cover every closed trace item".to_string(),
            );
        }
        Ok(())
    }

    /// Returns the closed portion of a persisted model-context log.
    ///
    /// An in-progress trace may durably stage the exact Assistant ToolCall identity needed for
    /// crash recovery. That final open exchange is never model-visible until an authoritative or
    /// synthetic ToolResult closes it.
    pub(crate) fn committed_model_context_prefix<'a>(
        &self,
        items: &'a [ConversationModelContextItem],
    ) -> Result<&'a [ConversationModelContextItem], String> {
        validate_model_context_prefix(self, items)?;
        let unresolved_sequence = (self.terminal_status
            == ConversationTurnTraceTerminalStatus::InProgress)
            .then(|| {
                self.items.iter().rev().find_map(|item| {
                    let ConversationTurnTraceItem::ToolCall { sequence, call_id, .. } = item else {
                        return None;
                    };
                    let closed = self.items.iter().any(|candidate| {
                        matches!(candidate, ConversationTurnTraceItem::ToolResult { call_id: result_id, .. } if result_id == call_id)
                    });
                    (!closed).then_some(*sequence)
                })
            })
            .flatten();
        let committed_len = unresolved_sequence
            .and_then(|sequence| items.iter().position(|item| item.sequence == sequence))
            .unwrap_or(items.len());
        Ok(&items[..committed_len])
    }
}

pub(crate) fn validate_tool_identity(
    trace_tool: &str,
    identity: &AgentToolIdentity,
) -> Result<(), String> {
    match identity {
        AgentToolIdentity::Builtin { tool_name } => {
            if tool_name != trace_tool || tool_name.chars().any(char::is_control) {
                return Err("conversation trace builtin provenance is inconsistent".to_string());
            }
        }
        AgentToolIdentity::RuntimeExtension {
            extension_id,
            tool_name,
        } => {
            if extension_id.trim().is_empty()
                || extension_id.trim() != extension_id
                || extension_id.chars().any(char::is_control)
                || tool_name != trace_tool
            {
                return Err(
                    "conversation trace runtime-extension provenance is invalid".to_string()
                );
            }
        }
        AgentToolIdentity::BuiltinCapability {
            capability_id,
            managed_mcp_id,
            package_name,
            package_version,
            upstream_catalog_digest,
            policy_digest,
            manifest_digest,
            tool_id,
            raw_name,
            model_name,
            upstream_schema_digest,
            host_overlay_digest,
            host_input_schema_digest,
        } => {
            let valid_digest = |digest: &str| {
                digest.len() == "sha256:".len() + 64
                    && digest.starts_with("sha256:")
                    && digest["sha256:".len()..]
                        .bytes()
                        .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
            };
            if crate::BuiltinCapabilityId::parse(capability_id.clone()).is_err()
                || crate::BuiltinCapabilityId::parse(managed_mcp_id.clone()).is_err()
                || crate::BuiltinCapabilityId::parse(tool_id.clone()).is_err()
                || crate::BuiltinCapabilityId::parse(raw_name.clone()).is_err()
                || package_name.trim().is_empty()
                || package_name.trim() != package_name.as_ref()
                || package_name.len() > 256
                || package_name.chars().any(char::is_control)
                || package_version.trim().is_empty()
                || package_version.trim() != package_version.as_ref()
                || package_version.len() > 128
                || package_version.chars().any(char::is_control)
                || !valid_digest(upstream_catalog_digest)
                || !valid_digest(policy_digest)
                || !valid_digest(manifest_digest)
                || !valid_digest(upstream_schema_digest)
                || !valid_digest(host_overlay_digest)
                || !valid_digest(host_input_schema_digest)
                || model_name.as_ref() != trace_tool
            {
                return Err(
                    "conversation trace built-in capability provenance is invalid".to_string(),
                );
            }
        }
        AgentToolIdentity::Mcp { provenance } => {
            let config_epoch_is_valid =
                uuid::Uuid::parse_str(&provenance.config_epoch).is_ok_and(|epoch| {
                    !epoch.is_nil()
                        && epoch.get_version() == Some(uuid::Version::Random)
                        && provenance.config_epoch == epoch.to_string()
                });
            let scoped_id_is_valid = match &provenance.scope {
                AgentMcpServerScope::Project { project_id } => {
                    !project_id.trim().is_empty()
                        && project_id.trim() == project_id
                        && project_id.len() <= 1_024
                        && !project_id.chars().any(char::is_control)
                }
                AgentMcpServerScope::Plugin { plugin_id } => {
                    !plugin_id.trim().is_empty()
                        && plugin_id.trim() == plugin_id
                        && plugin_id.len() <= 1_024
                        && !plugin_id.chars().any(char::is_control)
                }
                AgentMcpServerScope::Builtin
                | AgentMcpServerScope::User
                | AgentMcpServerScope::Managed => true,
            };
            if provenance.model_tool_name != trace_tool
                || uuid::Uuid::parse_str(&provenance.server_id).is_err()
                || !config_epoch_is_valid
                || provenance.registry_revision == 0
                || provenance.raw_tool_name.trim().is_empty()
                || provenance.raw_tool_name.trim() != provenance.raw_tool_name
                || provenance.raw_tool_name.len() > 1_024
                || provenance.raw_tool_name.chars().any(char::is_control)
                || provenance.catalog_generation == 0
                || provenance.config_digest.len() != 64
                || !provenance
                    .config_digest
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
                || provenance.catalog_digest.len() != 64
                || !provenance
                    .catalog_digest
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
                || provenance.catalog_schema_digest.len() != 64
                || !provenance
                    .catalog_schema_digest
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
                || provenance.schema_digest.len() != 64
                || !provenance
                    .schema_digest
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
                || provenance.schema_normalizer_version
                    != crate::MCP_INPUT_SCHEMA_NORMALIZER_VERSION
                || !scoped_id_is_valid
            {
                return Err("conversation trace MCP provenance is invalid".to_string());
            }
        }
        AgentToolIdentity::Unregistered { tool_name } => {
            if tool_name != trace_tool || !is_provider_safe_tool_name(tool_name) {
                return Err(
                    "conversation trace unregistered provenance is inconsistent".to_string()
                );
            }
        }
    }
    Ok(())
}

impl ConversationHistoryArchiveTraceMetadata {
    fn validate(&self) -> Result<(), String> {
        let has_archive_identity = self.archive_ref.is_some()
            || self.content_hash.is_some()
            || self.archived_bytes.is_some()
            || self.archived_completely.is_some();
        if !has_archive_identity {
            return Ok(());
        }
        let archive_ref = self
            .archive_ref
            .as_deref()
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| "conversation trace history archive ref is invalid".to_string())?;
        let content_hash = self
            .content_hash
            .as_deref()
            .filter(|value| {
                value.len() == 71
                    && value.starts_with("sha256:")
                    && value[7..]
                        .bytes()
                        .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
            })
            .ok_or_else(|| "conversation trace history archive hash is invalid".to_string())?;
        if archive_ref.len() > 256
            || content_hash.is_empty()
            || self.archived_bytes.is_none()
            || self.archived_completely != Some(true)
        {
            return Err("conversation trace history archive metadata is incomplete".to_string());
        }
        Ok(())
    }
}

impl ConversationTurnTrace {
    /// Appends one idempotent command-Session audit event using the trace sequence domain.
    ///
    /// The candidate is validated before `self` is changed, so a rejected transition cannot
    /// leave a partially invalid trace in memory or storage.
    pub fn append_command_session_lifecycle(
        &mut self,
        lifecycle: ConversationCommandSessionLifecycle,
    ) -> Result<u64, String> {
        if let Some(existing) = self.items.iter().find(|item| {
            matches!(
                item,
                ConversationTurnTraceItem::CommandSessionLifecycle {
                    phase,
                    session_id,
                    ..
                } if *phase == lifecycle.phase && session_id == &lifecycle.session_id
            )
        }) {
            let expected = ConversationTurnTraceItem::CommandSessionLifecycle {
                sequence: existing.sequence(),
                phase: lifecycle.phase,
                session_id: lifecycle.session_id,
                call_id: lifecycle.call_id,
                status: lifecycle.status,
                exit_code: lifecycle.exit_code,
                latest_sequence: lifecycle.latest_sequence,
                output_truncated: lifecycle.output_truncated,
                archive: lifecycle.archive,
                created_at: lifecycle.created_at,
            };
            return (existing == &expected)
                .then_some(existing.sequence())
                .ok_or_else(|| {
                    "conversation trace command session lifecycle conflicts with existing audit"
                        .to_string()
                });
        }

        let sequence = self
            .items
            .last()
            .map(ConversationTurnTraceItem::sequence)
            .map_or(0, |current| current.saturating_add(1));
        let item = ConversationTurnTraceItem::CommandSessionLifecycle {
            sequence,
            phase: lifecycle.phase,
            session_id: lifecycle.session_id,
            call_id: lifecycle.call_id,
            status: lifecycle.status,
            exit_code: lifecycle.exit_code,
            latest_sequence: lifecycle.latest_sequence,
            output_truncated: lifecycle.output_truncated,
            archive: lifecycle.archive,
            created_at: lifecycle.created_at,
        };
        let mut candidate = self.clone();
        candidate.items.push(item.clone());
        candidate.validate()?;
        self.items.push(item);
        Ok(sequence)
    }
}

impl ConversationTraceSnapshot {
    /// Returns the closed prefix that is safe to append to model context.
    ///
    /// The durable audit trace may retain one final unresolved call while the process owns the
    /// run. Keeping that call out of this prefix prevents a half tool exchange from entering
    /// model context, while still allowing startup recovery to correlate a process-owned external
    /// side effect with its eventual terminal receipt.
    pub fn committed_prefix(&self) -> Self {
        let unresolved_call_index = self.items.iter().enumerate().rev().find_map(|(index, item)| {
            let ConversationTurnTraceItem::ToolCall { call_id, .. } = item else {
                return None;
            };
            let closed = self.items[index + 1..].iter().any(|candidate| {
                matches!(candidate, ConversationTurnTraceItem::ToolResult { call_id: result_id, .. } if result_id == call_id)
            });
            (!closed).then_some(index)
        });
        let items = unresolved_call_index
            .map_or_else(|| self.items.clone(), |index| self.items[..index].to_vec());
        let covered_sequence = items.last().map(ConversationTurnTraceItem::sequence);
        let model_context_items = self
            .model_context_items
            .iter()
            .filter(|item| covered_sequence.is_some_and(|sequence| item.sequence <= sequence))
            .cloned()
            .collect();
        Self {
            items,
            model_context_items,
            next_sequence: self.next_sequence,
            truncated: self.truncated,
        }
    }

    pub fn in_progress_trace(
        &self,
        run_id: &str,
        conversation_id: &str,
        assistant_message_id: &str,
    ) -> ConversationTurnTrace {
        let committed = self.committed_prefix();
        ConversationTurnTrace {
            schema_version: CONVERSATION_TURN_TRACE_SCHEMA_VERSION,
            run_id: run_id.to_string(),
            conversation_id: conversation_id.to_string(),
            assistant_message_id: assistant_message_id.to_string(),
            terminal_status: ConversationTurnTraceTerminalStatus::InProgress,
            terminal_error: None,
            truncated: committed.truncated,
            items: committed.items,
        }
    }

    /// Builds the append-only in-progress audit view, including a final unresolved ToolCall.
    ///
    /// This trace is for durable recovery only. Callers that assemble model context must continue
    /// to use [`Self::in_progress_trace`], which retains closed exchanges only.
    pub fn in_progress_audit_trace(
        &self,
        run_id: &str,
        conversation_id: &str,
        assistant_message_id: &str,
    ) -> ConversationTurnTrace {
        ConversationTurnTrace {
            schema_version: CONVERSATION_TURN_TRACE_SCHEMA_VERSION,
            run_id: run_id.to_string(),
            conversation_id: conversation_id.to_string(),
            assistant_message_id: assistant_message_id.to_string(),
            terminal_status: ConversationTurnTraceTerminalStatus::InProgress,
            terminal_error: None,
            truncated: self.truncated,
            items: self.items.clone(),
        }
    }
}
