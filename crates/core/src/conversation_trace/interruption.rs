/// Exact Host-authenticated user-stop event. It is appended once at terminal commit, rather than
/// synthesized during context reconstruction, so every later request retains identical bytes.
impl ConversationTurnTrace {
    pub fn user_interrupted(&self) -> bool {
        self.terminal_status == ConversationTurnTraceTerminalStatus::Cancelled
            && self.items.iter().any(|item| {
                let ConversationTurnTraceItem::BackendState {
                    event_id, content, ..
                } = item
                else {
                    return false;
                };
                if event_id != "user-stop" {
                    return false;
                }
                serde_json::from_str::<Value>(content).is_ok_and(|value| {
                    value["type"] == "user_turn_interrupted" && value["reason"] == "user_requested"
                })
            })
    }

    pub(crate) fn append_user_interruption(
        &mut self,
        model_context_items: &mut Vec<ConversationModelContextItem>,
        stopped_at: i64,
    ) -> Result<(), String> {
        if self.terminal_status != ConversationTurnTraceTerminalStatus::Cancelled {
            return Err("only a cancelled Turn can record a user interruption".into());
        }
        if self.user_interrupted() {
            return self.validate_complete_model_context(model_context_items);
        }
        let mut recorder =
            ConversationTraceRecorder::from_durable_snapshot(ConversationTraceSnapshot {
                items: self.items.clone(),
                model_context_items: model_context_items.clone(),
                next_sequence: self
                    .items
                    .last()
                    .map(|item| item.sequence().saturating_add(1))
                    .unwrap_or(0),
                truncated: self.truncated,
            });
        let content = json!({
            "type": "user_turn_interrupted",
            "reason": "user_requested",
            "message": "The user stopped this turn. Completed actions remain in effect. Interrupted operations may have partially executed; verify their actual state before continuing or retrying them. This records an interruption, not authorization to resume."
        }).to_string();
        recorder.record_backend_state(
            recorder.next_sequence(),
            "user-stop",
            &content,
            stopped_at,
            ConversationBackendStatePlacement::AfterMessage,
        )?;
        let snapshot = recorder.snapshot();
        self.items = snapshot.items;
        *model_context_items = snapshot.model_context_items;
        self.validate()?;
        self.validate_complete_model_context(model_context_items)
    }
}
