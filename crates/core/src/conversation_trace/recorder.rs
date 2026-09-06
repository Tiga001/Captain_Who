impl ConversationTraceRecorder {
    pub(crate) fn from_checkpoint_with_model_context(
        items: Vec<ConversationTurnTraceItem>,
        model_context_items: Vec<ConversationModelContextItem>,
        next_sequence: u64,
        truncated: bool,
    ) -> Self {
        let inferred_next = items
            .iter()
            .map(ConversationTurnTraceItem::sequence)
            .max()
            .map(|sequence| sequence.saturating_add(1))
            .unwrap_or(0);
        Self {
            items,
            model_context_items,
            next_sequence: next_sequence.max(inferred_next),
            truncated,
            // A current approval checkpoint is committed with the same append-only Trace prefix.
            // Resuming it may append a ToolResult, but must never rewrite the frozen ToolCall
            // approval state after a restart.
            items_are_durable: true,
        }
    }

    fn from_durable_trace(
        items: Vec<ConversationTurnTraceItem>,
        next_sequence: u64,
        truncated: bool,
    ) -> Self {
        let inferred_next = items
            .iter()
            .map(ConversationTurnTraceItem::sequence)
            .max()
            .map(|sequence| sequence.saturating_add(1))
            .unwrap_or(0);
        Self {
            items,
            model_context_items: Vec::new(),
            next_sequence: next_sequence.max(inferred_next),
            truncated,
            items_are_durable: true,
        }
    }

    pub(crate) fn from_durable_snapshot(snapshot: ConversationTraceSnapshot) -> Self {
        let inferred_next = snapshot
            .items
            .iter()
            .map(ConversationTurnTraceItem::sequence)
            .max()
            .map(|sequence| sequence.saturating_add(1))
            .unwrap_or(0);
        Self {
            items: snapshot.items,
            model_context_items: snapshot.model_context_items,
            next_sequence: snapshot.next_sequence.max(inferred_next),
            truncated: snapshot.truncated,
            items_are_durable: true,
        }
    }

    fn unresolved_tool_call(&self) -> Option<AgentToolCall> {
        self.items.iter().rev().find_map(|item| {
            let ConversationTurnTraceItem::ToolCall {
                call_id,
                tool,
                operation,
                approval_status,
                ..
            } = item
            else {
                return None;
            };
            let has_result = self.items.iter().any(|candidate| {
                matches!(candidate, ConversationTurnTraceItem::ToolResult { call_id: result_id, .. } if result_id == call_id)
            });
            (!has_result).then(|| AgentToolCall {
                id: call_id.clone(),
                tool: tool.clone(),
                args: operation.clone(),
                approval_status: *approval_status,
                reason: operation
                    .get("reason")
                    .and_then(Value::as_str)
                    .map(str::to_string),
            })
        })
    }

    #[cfg(test)]
    pub(crate) fn checkpoint(
        &self,
    ) -> (
        Vec<ConversationTurnTraceItem>,
        Vec<ConversationModelContextItem>,
        u64,
        bool,
    ) {
        (
            self.items.clone(),
            self.model_context_items.clone(),
            self.next_sequence,
            self.truncated,
        )
    }

    pub(crate) fn checkpoint_snapshot(&self) -> ConversationTraceSnapshot {
        ConversationTraceSnapshot {
            items: self.items.clone(),
            model_context_items: self.model_context_items.clone(),
            next_sequence: self.next_sequence,
            truncated: self.truncated,
        }
    }

    pub(crate) fn mark_truncated(&mut self) {
        self.truncated = true;
    }

    pub(crate) fn snapshot(&self) -> ConversationTraceSnapshot {
        let (items, projected_truncated) = if self.items_are_durable {
            (self.items.clone(), false)
        } else {
            project_durable_trace_items(&self.items)
        };
        ConversationTraceSnapshot {
            items,
            model_context_items: self.model_context_items.clone(),
            next_sequence: self.next_sequence,
            truncated: self.truncated || projected_truncated,
        }
    }

    #[cfg(test)]
    pub(crate) fn committed_item_count(&self) -> usize {
        self.snapshot().committed_prefix().items.len()
    }

    pub(crate) fn record_narration(&mut self, content: &str) -> Result<Option<u64>, String> {
        let content = content.trim();
        if content.is_empty() {
            return Ok(None);
        }
        let (content, redacted) = sanitize_text(content);
        let sequence = self.take_sequence();
        self.items
            .push(ConversationTurnTraceItem::AssistantNarration {
                sequence,
                content: content.clone(),
                truncated: redacted,
            });
        self.record_model_message(
            sequence,
            0,
            &LlmMessage::text(crate::llm::LlmMessageRole::Assistant, content),
        )?;
        self.truncated |= redacted;
        Ok(Some(sequence))
    }

    pub(crate) fn record_context_compaction_started(
        &mut self,
        operation_id: &str,
    ) -> Result<u64, String> {
        if operation_id.trim().is_empty()
            || operation_id.len() > 2_048
            || operation_id.chars().any(char::is_control)
        {
            return Err("context compaction trace identity is invalid".to_string());
        }
        if let Some(sequence) = self.items.iter().find_map(|item| match item {
            ConversationTurnTraceItem::ContextCompactionLifecycle {
                sequence,
                phase: ConversationContextCompactionLifecyclePhase::Started,
                operation_id: existing,
                ..
            } if existing == operation_id => Some(*sequence),
            _ => None,
        }) {
            return Ok(sequence);
        }
        if matches!(
            self.items.last(),
            Some(ConversationTurnTraceItem::ToolCall { .. })
        ) {
            return Err("context compaction cannot split an unresolved tool exchange".to_string());
        }
        let sequence = self.take_sequence();
        self.items
            .push(ConversationTurnTraceItem::ContextCompactionLifecycle {
                sequence,
                phase: ConversationContextCompactionLifecyclePhase::Started,
                operation_id: operation_id.to_string(),
                outcome: None,
            });
        Ok(sequence)
    }

    pub(crate) fn record_context_compaction_finished(
        &mut self,
        operation_id: &str,
        outcome: AgentContextCompactionEventOutcome,
    ) -> Result<u64, String> {
        let Some(start_sequence) = self.items.iter().find_map(|item| match item {
            ConversationTurnTraceItem::ContextCompactionLifecycle {
                sequence,
                phase: ConversationContextCompactionLifecyclePhase::Started,
                operation_id: existing,
                ..
            } if existing == operation_id => Some(*sequence),
            _ => None,
        }) else {
            return Err("context compaction finish is missing its durable start".to_string());
        };
        if let Some(existing_outcome) = self.items.iter().find_map(|item| match item {
            ConversationTurnTraceItem::ContextCompactionLifecycle {
                phase: ConversationContextCompactionLifecyclePhase::Finished,
                operation_id: existing,
                outcome,
                ..
            } if existing == operation_id => *outcome,
            _ => None,
        }) {
            return (existing_outcome == outcome)
                .then_some(start_sequence)
                .ok_or_else(|| "context compaction outcome conflicts with its trace".to_string());
        }
        let sequence = self.take_sequence();
        self.items
            .push(ConversationTurnTraceItem::ContextCompactionLifecycle {
                sequence,
                phase: ConversationContextCompactionLifecyclePhase::Finished,
                operation_id: operation_id.to_string(),
                outcome: Some(outcome),
            });
        Ok(start_sequence)
    }

    pub(crate) fn record_runtime_error(
        &mut self,
        message: &str,
        recoverable: bool,
        code: Option<&str>,
    ) -> Result<u64, String> {
        if message.trim().is_empty() {
            return Err("Runtime error trace message cannot be empty".to_string());
        }
        if code.is_some_and(|code| {
            code.trim().is_empty() || code.len() > 128 || code.chars().any(char::is_control)
        }) {
            return Err("Runtime error trace code is invalid".to_string());
        }
        if matches!(
            self.items.last(),
            Some(ConversationTurnTraceItem::ToolCall { .. })
        ) {
            return Err("Runtime error cannot split an unresolved tool exchange".to_string());
        }
        let (message, message_truncated) = project_terminal_error(message);
        let (code, code_truncated) = code
            .map(sanitize_text)
            .map(|(value, truncated)| (Some(value), truncated))
            .unwrap_or((None, false));
        let truncated = message_truncated || code_truncated;
        let sequence = self.take_sequence();
        self.items.push(ConversationTurnTraceItem::RuntimeError {
            sequence,
            message,
            recoverable,
            code,
            truncated,
        });
        self.truncated |= truncated;
        Ok(sequence)
    }

    pub(crate) fn record_model_message(
        &mut self,
        sequence: u64,
        ordinal: u32,
        message: &LlmMessage,
    ) -> Result<(), String> {
        if self
            .model_context_items
            .iter()
            .any(|item| item.sequence == sequence && item.ordinal == ordinal)
        {
            return Ok(());
        }
        let (item, truncated) = model_context_item_from_message(sequence, ordinal, message)?;
        self.model_context_items.push(item);
        self.model_context_items
            .sort_by_key(|item| (item.sequence, item.ordinal));
        self.truncated |= truncated;
        Ok(())
    }

    /// Records one split durable Assistant Tool Call with the exact identity from its original
    /// provider turn.
    ///
    /// The durable model log intentionally stores one Assistant message per Tool Call. For a
    /// grouped provider turn, deriving identity from that split message would renumber calls and
    /// lose the provider-owned call ID. Runtime therefore supplies the frozen mapping explicitly.
    pub(crate) fn record_model_tool_call_message(
        &mut self,
        sequence: u64,
        ordinal: u32,
        message: &LlmMessage,
        provider_identity: AgentProviderToolCallIdentity,
    ) -> Result<(), String> {
        if self
            .model_context_items
            .iter()
            .any(|item| item.sequence == sequence && item.ordinal == ordinal)
        {
            return Ok(());
        }
        let provider_identities = BTreeMap::from([(
            provider_identity.runtime_call_id.clone(),
            provider_identity.clone(),
        )]);
        let (item, truncated) = model_context_item_from_message_with_identities(
            sequence,
            ordinal,
            message,
            Some(&provider_identities),
        )?;
        if item.role != "assistant"
            || item.tool_calls.len() != 1
            || provider_identity.runtime_call_id != item.tool_calls[0].id
        {
            return Err(
                "durable Assistant Tool Call identity does not match its model projection"
                    .to_string(),
            );
        }
        self.model_context_items.push(item);
        self.model_context_items
            .sort_by_key(|item| (item.sequence, item.ordinal));
        self.truncated |= truncated;
        Ok(())
    }

    /// Applies the same durable approval-barrier projection as Context. Raw queued external calls
    /// never enter the persisted model log, while the live provider turn remains in memory.
    pub(crate) fn omit_model_tool_calls(&mut self, omitted_call_ids: &BTreeSet<String>) {
        if omitted_call_ids.is_empty() {
            return;
        }
        for item in &mut self.model_context_items {
            if item.role == "assistant" {
                item.tool_calls
                    .retain(|call| !omitted_call_ids.contains(&call.id));
            }
        }
    }

    /// Admits one Host-owned state fact into the audit and model journals atomically.
    /// Retrying the exact durable identity is a no-op; it never creates a second observation.
    pub fn record_backend_state(
        &mut self,
        expected_sequence: u64,
        event_id: &str,
        content: &str,
        created_at: i64,
        placement: ConversationBackendStatePlacement,
    ) -> Result<Option<String>, String> {
        validate_backend_state(event_id, content, created_at)?;
        if let Some(existing) = self.items.iter().find(|item| matches!(
            item, ConversationTurnTraceItem::BackendState { event_id: existing_id, .. }
                if existing_id == event_id
        )) {
            let identical = matches!(existing, ConversationTurnTraceItem::BackendState {
                sequence, content: existing_content, created_at: existing_time,
                placement: existing_placement, ..
            } if *sequence == expected_sequence && existing_content == content
                && *existing_time == created_at && *existing_placement == placement);
            let model_is_present = self.model_context_items.iter().any(|item| {
                item.sequence == expected_sequence && item.ordinal == 0 && item.role == "user"
                    && item.content == content && item.tool_calls.is_empty()
                    && item.tool_call_id.is_none() && !item.is_error
            });
            return if identical && model_is_present { Ok(None) } else {
                Err("Backend state identity conflicts with its durable journals".to_string())
            };
        }
        if self.next_sequence != expected_sequence {
            return Err("Backend state expected trace sequence does not match".to_string());
        }
        if self.unresolved_tool_call().is_some() {
            return Err("Backend state cannot split an unresolved tool exchange".to_string());
        }
        if self.model_context_items.iter().any(|item| item.sequence == expected_sequence) {
            return Err("Backend state model sequence is already occupied".to_string());
        }
        let (model_item, truncated) = model_context_item_from_message(
            expected_sequence, 0, &LlmMessage::text(crate::llm::LlmMessageRole::User, content),
        )?;
        if truncated || model_item.content != content {
            return Err("Backend state model projection must preserve its exact JSON".to_string());
        }
        self.items.push(ConversationTurnTraceItem::BackendState {
            sequence: expected_sequence,
            event_id: event_id.to_string(),
            content: content.to_string(),
            created_at,
            placement,
        });
        self.model_context_items.push(model_item);
        self.next_sequence = self.next_sequence.saturating_add(1);
        Ok(Some(content.to_string()))
    }

    pub(crate) fn record_user_guidance(
        &mut self,
        guidance_id: &str,
        client_message_id: &str,
        content: &str,
        attachments: &[AgentInputAttachment],
        created_at: i64,
    ) -> Option<u64> {
        let content = content.trim();
        if content.is_empty()
            || matches!(
                self.items.last(),
                Some(ConversationTurnTraceItem::ToolCall { .. })
            )
        {
            return None;
        }
        let (content, content_redacted) = sanitize_text(content);
        let (attachments, attachment_redacted) = trace_attachments_from_input(attachments);
        let sequence = self.take_sequence();
        self.items.push(ConversationTurnTraceItem::UserGuidance {
            sequence,
            guidance_id: guidance_id.to_string(),
            client_message_id: client_message_id.to_string(),
            content,
            attachments,
            created_at,
            truncated: content_redacted || attachment_redacted,
        });
        self.truncated |= content_redacted || attachment_redacted;
        Some(sequence)
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn record_agent_mailbox_delivery(
        &mut self,
        expected_sequence: u64,
        receipt_id: &str,
        message_id: &str,
        sender_agent_id: &str,
        sender_task_name: &str,
        sender_task_path: &str,
        kind: crate::AgentMailboxKind,
        content: &str,
        created_at: i64,
    ) -> Result<Option<String>, String> {
        let content = content.trim();
        if receipt_id.trim().is_empty()
            || message_id.trim().is_empty()
            || sender_agent_id.trim().is_empty()
            || sender_task_name.trim().is_empty()
            || sender_task_path.trim().is_empty()
            || content.is_empty()
            || created_at < 0
        {
            return Err("Agent mailbox delivery identity is invalid".to_string());
        }
        let (model_content, model_truncated) = project_agent_mailbox_model_envelope(
            sender_agent_id,
            sender_task_name,
            sender_task_path,
            kind,
            content,
        )?;
        let (trace_content, trace_truncated) = project_agent_mailbox_envelope_with_budget(
            sender_agent_id,
            sender_task_name,
            sender_task_path,
            kind,
            content,
            DurableTraceProjectionLimits::USER_GUIDANCE_CHARS,
        )?;
        let truncated = model_truncated || trace_truncated;
        if let Some(existing) = self.items.iter().find(|item| {
            matches!(
                item,
                ConversationTurnTraceItem::AgentMailboxDelivery {
                    message_id: existing_message,
                    ..
                } if existing_message == message_id
            )
        }) {
            return match existing {
                ConversationTurnTraceItem::AgentMailboxDelivery {
                    sequence,
                    receipt_id: existing_receipt,
                    message_id: existing_message,
                    sender_agent_id: existing_sender,
                    sender_task_name: existing_task_name,
                    sender_task_path: existing_task_path,
                    kind: existing_kind,
                    content: existing_content,
                    created_at: existing_created_at,
                    truncated: existing_truncated,
                } if *sequence == expected_sequence
                    && existing_receipt == receipt_id
                    && existing_message == message_id
                    && existing_sender == sender_agent_id
                    && existing_task_name == sender_task_name
                    && existing_task_path == sender_task_path
                    && *existing_kind == kind
                    && existing_content == &trace_content
                    && *existing_created_at == created_at
                    && *existing_truncated == truncated =>
                {
                    Ok(None)
                }
                _ => Err("Agent mailbox delivery identity conflicts with the trace".to_string()),
            };
        }
        if self.next_sequence != expected_sequence {
            return Err(format!(
                "Agent mailbox delivery expected trace sequence {expected_sequence}, current is {}",
                self.next_sequence
            ));
        }
        if matches!(
            self.items.last(),
            Some(ConversationTurnTraceItem::ToolCall { .. })
        ) {
            return Err(
                "Agent mailbox delivery cannot split an unresolved tool exchange".to_string(),
            );
        }
        self.items
            .push(ConversationTurnTraceItem::AgentMailboxDelivery {
                sequence: expected_sequence,
                receipt_id: receipt_id.to_string(),
                message_id: message_id.to_string(),
                sender_agent_id: sender_agent_id.to_string(),
                sender_task_name: sender_task_name.to_string(),
                sender_task_path: sender_task_path.to_string(),
                kind,
                content: trace_content,
                created_at,
                truncated,
            });
        self.record_model_message(
            expected_sequence,
            0,
            &LlmMessage::text(crate::llm::LlmMessageRole::User, model_content.clone()),
        )?;
        self.next_sequence = self.next_sequence.saturating_add(1);
        self.truncated |= truncated;
        Ok(Some(model_content))
    }

    pub(crate) fn next_sequence(&self) -> u64 {
        self.next_sequence
    }

    #[cfg(test)]
    pub(crate) fn record_tool_call(&mut self, call: &AgentToolCall) -> Option<u64> {
        self.record_tool_call_with_identity(
            call,
            AgentToolIdentity::Builtin {
                tool_name: call.tool.clone(),
            },
        )
    }

    pub(crate) fn record_tool_call_with_identity(
        &mut self,
        call: &AgentToolCall,
        provenance: AgentToolIdentity,
    ) -> Option<u64> {
        if let Some(sequence) = self.items.iter().find_map(|item| {
            matches!(item, ConversationTurnTraceItem::ToolCall { call_id, .. } if call_id == &call.id)
                .then(|| item.sequence())
        }) {
            return Some(sequence);
        }
        let (operation, redacted) = sanitize_value(&call.args);
        let sequence = self.take_sequence();
        self.items.push(ConversationTurnTraceItem::ToolCall {
            sequence,
            call_id: call.id.clone(),
            tool: call.tool.clone(),
            provenance,
            operation,
            approval_status: call.approval_status,
            truncated: redacted,
        });
        self.truncated |= redacted;
        Some(sequence)
    }

    /// Returns the sequence of a ToolCall already frozen into the current trace.
    ///
    /// Approval and restart continuations must reuse the trusted provenance captured before the
    /// side-effect boundary. They are never allowed to infer a new identity from a tool name.
    pub(crate) fn require_recorded_tool_call(&self, call: &AgentToolCall) -> Result<u64, String> {
        let sequence = self
            .items
            .iter()
            .find_map(|item| match item {
                ConversationTurnTraceItem::ToolCall {
                    sequence,
                    call_id,
                    tool,
                    ..
                } if call_id == &call.id && tool == &call.tool => Some(*sequence),
                _ => None,
            })
            .ok_or_else(|| {
                "current continuation is missing its frozen ToolCall provenance".to_string()
            })?;
        let has_model_context = self.model_context_items.iter().any(|item| {
            item.sequence == sequence
                && item.role == "assistant"
                && item.tool_call_id.is_none()
                && item.tool_calls.len() == 1
                && item.tool_calls[0].id == call.id
                && item.tool_calls[0].name == call.tool
        });
        if !has_model_context {
            return Err(
                "current continuation is missing its immutable ToolCall model context".to_string(),
            );
        }
        Ok(sequence)
    }

    /// Approval enriches the state of the original model call; it does not replace the model's
    /// arguments with a second, presentation-oriented representation.
    pub(crate) fn enrich_tool_call(&mut self, action: &AgentProposedAction) {
        let (call_id, approval_status) = match action {
            AgentProposedAction::FileChange { file_change } => {
                (&file_change.id, file_change.approval_status)
            }
            AgentProposedAction::Command { command } => (&command.id, command.approval_status),
            AgentProposedAction::SkillMaterialization { materialization } => {
                (&materialization.id, materialization.approval_status)
            }
            AgentProposedAction::SkillScript { script } => (&script.id, script.approval_status),
            AgentProposedAction::OfficeOperation { office_operation } => {
                (&office_operation.id, office_operation.approval_status)
            }
            AgentProposedAction::SkillInstallation { installation } => {
                (&installation.id, installation.approval_status)
            }
            AgentProposedAction::ToolCall { call } => (&call.id, call.approval_status),
            AgentProposedAction::McpToolCall { approval } => {
                (&approval.identity.call_id, approval.call.approval_status)
            }
            AgentProposedAction::BuiltinCapabilityActivation { approval } => {
                (&approval.call_id, approval.approval_status)
            }
            AgentProposedAction::BuiltinMcpToolApproval { approval } => {
                (&approval.identity.call_id, approval.approval_status)
            }
            AgentProposedAction::BrowserRiskApproval { approval } => {
                (&approval.call_id, approval.approval_status)
            }
        };
        if let Some(ConversationTurnTraceItem::ToolCall {
            approval_status: current,
            ..
        }) = self.items.iter_mut().rev().find(
            |item| matches!(item, ConversationTurnTraceItem::ToolCall { call_id: candidate, .. } if candidate == call_id),
        ) {
            *current = approval_status;
        }
    }

    pub(crate) fn record_tool_result(
        &mut self,
        call: &AgentToolCall,
        result: &AgentToolResult,
    ) -> Option<u64> {
        self.record_tool_result_with_archive(call, result, Default::default())
    }

    pub(crate) fn pending_tool_result_sequence(&self, call_id: &str) -> Option<u64> {
        let has_call = self.items.iter().any(
            |item| matches!(item, ConversationTurnTraceItem::ToolCall { call_id: candidate, .. } if candidate == call_id),
        );
        let has_result = self.items.iter().any(
            |item| matches!(item, ConversationTurnTraceItem::ToolResult { call_id: candidate, .. } if candidate == call_id),
        );
        (has_call && !has_result).then_some(self.next_sequence)
    }

    pub(crate) fn record_tool_result_with_archive(
        &mut self,
        call: &AgentToolCall,
        result: &AgentToolResult,
        archive: ConversationHistoryArchiveTraceMetadata,
    ) -> Option<u64> {
        let call_index = self.items.iter().rposition(
            |item| matches!(item, ConversationTurnTraceItem::ToolCall { call_id, .. } if call_id == &call.id),
        )?;
        if self.items.iter().any(|item| {
            matches!(item, ConversationTurnTraceItem::ToolResult { call_id, .. } if call_id == &call.id)
        }) {
            return self.items.iter().find_map(|item| {
                matches!(item, ConversationTurnTraceItem::ToolResult { call_id, .. } if call_id == &call.id)
                    .then(|| item.sequence())
            });
        }
        if !self.items_are_durable {
            if let ConversationTurnTraceItem::ToolCall {
                approval_status, ..
            } = &mut self.items[call_index]
            {
                *approval_status = call.approval_status;
            }
        }

        let sequence = self.take_sequence();
        let mut item = if self.items_are_durable {
            projected_tool_result_trace_item(sequence, call, result)
        } else {
            checkpoint_tool_result_trace_item(sequence, call, result)
        };
        if let ConversationTurnTraceItem::ToolResult {
            truncated,
            archive: item_archive,
            ..
        } = &mut item
        {
            *item_archive = archive;
            if self.items_are_durable {
                // `projected_tool_result_trace_item` has already crossed the same bounded
                // history-projection boundary that `project_durable_trace_items` applies when a
                // live Runtime snapshot reaches terminal settlement. Preserve that fact in the
                // archive metadata as well as on the item itself. Otherwise a precommitted
                // wait_agent result whose generic payload exceeds the history limit differs from
                // the later Runtime terminal projection only by this bit, and the append-only
                // exact-prefix guard correctly rejects the terminal commit.
                item_archive.history_projection_truncated |= *truncated;
            }
        }
        self.truncated |= matches!(
            &item,
            ConversationTurnTraceItem::ToolResult {
                truncated: true,
                ..
            }
        );
        self.items.push(item);
        Some(sequence)
    }

    pub(crate) fn finish(
        &self,
        run_id: &str,
        conversation_id: &str,
        assistant_message_id: &str,
        terminal_status: ConversationTurnTraceTerminalStatus,
        terminal_error: Option<&str>,
    ) -> ConversationTurnTrace {
        let mut recorder = self.clone();
        recorder.close_unresolved(terminal_status, terminal_error);
        recorder.close_unresolved_context_compactions(terminal_status);
        let recorder_truncated = recorder.truncated;
        let (items, projected_truncated) = if recorder.items_are_durable {
            (recorder.items, false)
        } else {
            project_durable_trace_items(&recorder.items)
        };
        let (terminal_error, terminal_redacted) = terminal_error
            .map(project_terminal_error)
            .map(|(value, truncated)| (Some(value), truncated))
            .unwrap_or((None, false));
        ConversationTurnTrace {
            schema_version: CONVERSATION_TURN_TRACE_SCHEMA_VERSION,
            run_id: run_id.to_string(),
            conversation_id: conversation_id.to_string(),
            assistant_message_id: assistant_message_id.to_string(),
            terminal_status,
            terminal_error,
            truncated: recorder_truncated || projected_truncated || terminal_redacted,
            items,
        }
    }

    fn close_unresolved(
        &mut self,
        terminal_status: ConversationTurnTraceTerminalStatus,
        terminal_error: Option<&str>,
    ) {
        let Some((call_id, tool, operation, approval_status)) =
            self.items.iter().rev().find_map(|item| {
            if let ConversationTurnTraceItem::ToolCall {
                call_id,
                tool,
                operation,
                approval_status,
                ..
            } = item
            {
                let has_result = self.items.iter().any(|candidate| {
                    matches!(candidate, ConversationTurnTraceItem::ToolResult { call_id: result_id, .. } if result_id == call_id)
                });
                (!has_result).then(|| {
                    (
                        call_id.clone(),
                        tool.clone(),
                        operation.clone(),
                        *approval_status,
                    )
                })
            } else {
                None
            }
        }) else {
            return;
        };
        let status = if terminal_status == ConversationTurnTraceTerminalStatus::Cancelled {
            ConversationTraceToolResultStatus::Cancelled
        } else {
            ConversationTraceToolResultStatus::Failed
        };
        let fallback = if terminal_status == ConversationTurnTraceTerminalStatus::Cancelled {
            "Run cancelled before a verifiable tool result was available."
        } else {
            "Run ended before a verifiable tool result was available."
        };
        let (raw_error, raw_redacted) = sanitize_text(terminal_error.unwrap_or(fallback));
        let raw_observation = json!({
            "resultAvailable": false,
            "terminalStatus": terminal_status,
        });
        let (observation, error, projection_truncated) = if self.items_are_durable {
            let (observation, error, error_truncated) =
                project_tool_result(&tool, Some(&operation), &raw_observation, Some(&raw_error));
            (
                observation.value,
                error,
                observation.truncated || error_truncated,
            )
        } else {
            (raw_observation, Some(raw_error), false)
        };
        let sequence = self.take_sequence();
        let item = ConversationTurnTraceItem::ToolResult {
            sequence,
            call_id,
            tool,
            status,
            success: false,
            observation,
            approval_status,
            error,
            truncated: raw_redacted || projection_truncated,
            archive: Default::default(),
        };
        self.truncated |= matches!(
            item,
            ConversationTurnTraceItem::ToolResult {
                truncated: true,
                ..
            }
        );
        self.items.push(item);
    }

    fn close_unresolved_context_compactions(
        &mut self,
        terminal_status: ConversationTurnTraceTerminalStatus,
    ) {
        if !terminal_status.is_terminal() {
            return;
        }
        let mut operations = Vec::<(String, bool)>::new();
        for item in &self.items {
            let ConversationTurnTraceItem::ContextCompactionLifecycle {
                phase,
                operation_id,
                ..
            } = item
            else {
                continue;
            };
            match phase {
                ConversationContextCompactionLifecyclePhase::Started => {
                    operations.push((operation_id.clone(), false));
                }
                ConversationContextCompactionLifecyclePhase::Finished => {
                    if let Some((_, finished)) = operations
                        .iter_mut()
                        .find(|(candidate, _)| candidate == operation_id)
                    {
                        *finished = true;
                    }
                }
            }
        }
        let outcome = match terminal_status {
            ConversationTurnTraceTerminalStatus::Cancelled => {
                AgentContextCompactionEventOutcome::Cancelled
            }
            ConversationTurnTraceTerminalStatus::Failed => {
                AgentContextCompactionEventOutcome::Failed
            }
            ConversationTurnTraceTerminalStatus::Completed => {
                AgentContextCompactionEventOutcome::Skipped
            }
            ConversationTurnTraceTerminalStatus::InProgress => return,
        };
        for (operation_id, finished) in operations {
            if finished {
                continue;
            }
            let sequence = self.take_sequence();
            self.items
                .push(ConversationTurnTraceItem::ContextCompactionLifecycle {
                    sequence,
                    phase: ConversationContextCompactionLifecyclePhase::Finished,
                    operation_id,
                    outcome: Some(outcome),
                });
        }
    }

    fn take_sequence(&mut self) -> u64 {
        let sequence = self.next_sequence;
        self.next_sequence = self.next_sequence.saturating_add(1);
        sequence
    }
}

pub(crate) fn trace_attachments_from_input(
    attachments: &[AgentInputAttachment],
) -> (Vec<ConversationTraceAttachment>, bool) {
    let mut redacted = false;
    let attachments = attachments
        .iter()
        .map(|attachment| {
            let (name, name_redacted) = sanitize_text(&attachment.name);
            let (mime_type, mime_redacted) =
                sanitize_optional_text(attachment.mime_type.as_deref());
            redacted |= name_redacted || mime_redacted || attachment.truncated.unwrap_or(false);
            ConversationTraceAttachment {
                id: attachment.id.clone(),
                kind: attachment.kind,
                name,
                mime_type,
                size_bytes: attachment.size_bytes,
            }
        })
        .collect();
    (attachments, redacted)
}

pub(crate) fn render_user_guidance_content(
    content: &str,
    attachments: &[ConversationTraceAttachment],
) -> String {
    if attachments.is_empty() {
        return content.to_string();
    }
    let attachment_list = attachments
        .iter()
        .map(|attachment| {
            format!(
                "- {} ({}, {} bytes, id: {})",
                attachment.name,
                match attachment.kind {
                    AgentInputAttachmentKind::File => "file",
                    AgentInputAttachmentKind::Image => "image",
                },
                attachment.size_bytes,
                attachment.id
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    format!("{content}\n\nAttachments supplied with this user guidance:\n{attachment_list}")
}
