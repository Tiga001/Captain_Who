/// A scoped append journal. Only operations used at Runtime publication boundaries are exposed;
/// existing arbitrary items cannot be edited through this capability. Dropping without commit
/// restores the original prefix, even while unwinding after a panic.
pub(crate) struct ConversationTraceAppendTransaction<'a> {
    recorder: &'a mut ConversationTraceRecorder,
    item_count: usize,
    model_boundary: Option<(u64, u32)>,
    next_sequence: u64,
    truncated: bool,
    changed_calls: Vec<(usize, ConversationTurnTraceItem)>,
    committed: bool,
}

impl ConversationTraceRecorder {
    pub(crate) fn begin_append(&mut self) -> ConversationTraceAppendTransaction<'_> {
        ConversationTraceAppendTransaction {
            item_count: self.items.len(),
            model_boundary: self
                .model_context_items
                .last()
                .map(|item| (item.sequence, item.ordinal)),
            next_sequence: self.next_sequence,
            truncated: self.truncated,
            changed_calls: Vec::new(),
            committed: false,
            recorder: self,
        }
    }
}

impl ConversationTraceAppendTransaction<'_> {
    pub(crate) fn commit(mut self) {
        self.committed = true;
    }

    pub(crate) fn next_sequence(&self) -> u64 {
        self.recorder.next_sequence()
    }

    pub(crate) fn mark_truncated(&mut self) {
        self.recorder.mark_truncated();
    }

    pub(crate) fn record_tool_call_with_identity(
        &mut self,
        call: &AgentToolCall,
        provenance: AgentToolIdentity,
    ) -> Option<u64> {
        self.recorder
            .record_tool_call_with_identity(call, provenance)
    }

    pub(crate) fn record_tool_result_with_archive(
        &mut self,
        call: &AgentToolCall,
        result: &AgentToolResult,
        archive: ConversationHistoryArchiveTraceMetadata,
    ) -> Option<u64> {
        if !self.recorder.items_are_durable {
            if let Some(index) = self.recorder.items.iter().rposition(|item| {
                matches!(item, ConversationTurnTraceItem::ToolCall { call_id, .. } if call_id == &call.id)
            }) {
                if index < self.item_count && !self.changed_calls.iter().any(|(saved, _)| *saved == index) {
                    self.changed_calls.push((index, self.recorder.items[index].clone()));
                }
            }
        }
        self.recorder
            .record_tool_result_with_archive(call, result, archive)
    }

    fn check_model_append(&self, sequence: u64, ordinal: u32) -> Result<(), String> {
        let identity = (sequence, ordinal);
        if self
            .model_boundary
            .is_some_and(|boundary| identity <= boundary)
            && !self
                .recorder
                .model_context_items
                .iter()
                .any(|item| (item.sequence, item.ordinal) == identity)
        {
            return Err(
                "atomic Trace publication cannot insert into an existing model prefix".into(),
            );
        }
        Ok(())
    }

    pub(crate) fn record_model_message(
        &mut self,
        sequence: u64,
        ordinal: u32,
        message: &LlmMessage,
    ) -> Result<(), String> {
        self.check_model_append(sequence, ordinal)?;
        self.recorder
            .record_model_message(sequence, ordinal, message)
    }

    pub(crate) fn record_model_tool_call_message(
        &mut self,
        sequence: u64,
        ordinal: u32,
        message: &LlmMessage,
        provider_identity: AgentProviderToolCallIdentity,
    ) -> Result<(), String> {
        self.check_model_append(sequence, ordinal)?;
        self.recorder
            .record_model_tool_call_message(sequence, ordinal, message, provider_identity)
    }

    pub(crate) fn record_context_material(
        &mut self,
        event_id: &str,
        material_kind: ConversationContextMaterialKind,
        content: &str,
        images: &[ConversationContextImageRef],
        created_at: i64,
    ) -> Result<u64, String> {
        self.check_model_append(self.recorder.next_sequence, 0)?;
        self.recorder
            .record_context_material(event_id, material_kind, content, images, created_at)
    }
}

impl Drop for ConversationTraceAppendTransaction<'_> {
    fn drop(&mut self) {
        if self.committed {
            return;
        }
        self.recorder.items.truncate(self.item_count);
        for (index, item) in self.changed_calls.drain(..) {
            self.recorder.items[index] = item;
        }
        self.recorder.model_context_items.retain(|item| {
            self.model_boundary
                .is_some_and(|boundary| (item.sequence, item.ordinal) <= boundary)
        });
        self.recorder.next_sequence = self.next_sequence;
        self.recorder.truncated = self.truncated;
    }
}

#[cfg(test)]
#[path = "append_transaction_tests.rs"]
mod append_transaction_tests;
