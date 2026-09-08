impl AgentService {
    pub(super) fn reserve_conversation_turn(
        &self,
        conversation_id: &str,
        run_id: &str,
        assistant_message_id: &str,
    ) -> Result<(), AgentServiceError> {
        let conversation_id = conversation_id.trim();
        if conversation_id.is_empty() {
            return Err(
                "conversation turn admission requires a conversation identity"
                    .to_string()
                    .into(),
            );
        }
        self.ensure_no_manual_context_compaction(conversation_id)?;
        let mut active_turns = self
            .active_conversation_turns
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if let Some(active) = active_turns.get(conversation_id) {
            return Err(AgentServiceError::structured(
                format!(
                    "当前会话已有进行中的 agent 运行（runId={}），请等待其完成。",
                    active.run_id
                ),
                serde_json::json!({
                    "domain": "agent_turn",
                    "code": "conversation_busy",
                    "retryable": true
                }),
            ));
        }
        if let Some(active) = self
            .storage
            .list_conversation_turn_traces(conversation_id)?
            .into_iter()
            .find(|trace| trace.terminal_status == ConversationTurnTraceTerminalStatus::InProgress)
        {
            return Err(AgentServiceError::structured(
                format!(
                    "当前会话已有持久化的进行中 agent 运行（runId={}），请先完成或恢复它。",
                    active.run_id
                ),
                serde_json::json!({
                    "domain": "agent_turn",
                    "code": "conversation_busy",
                    "retryable": true
                }),
            ));
        }
        active_turns.insert(
            conversation_id.to_string(),
            ActiveConversationTurn {
                run_id: run_id.to_string(),
                assistant_message_id: assistant_message_id.to_string(),
            },
        );
        Ok(())
    }

    /// Consults both the process accelerator and the durable trace lease. Callers such as model
    /// transition preflight must not infer Turn quiescence from the currently resident Runtime:
    /// Runtime may already be gone while approval or terminal persistence is still outstanding.
    pub(super) fn has_conversation_turn_occupancy(
        &self,
        conversation_id: &str,
    ) -> Result<bool, String> {
        if self
            .ensure_no_manual_context_compaction(conversation_id)
            .is_err()
        {
            return Ok(true);
        }
        if self
            .active_conversation_turns
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .contains_key(conversation_id)
        {
            return Ok(true);
        }
        Ok(self
            .storage
            .list_conversation_turn_traces(conversation_id)?
            .iter()
            .any(|trace| trace.terminal_status == ConversationTurnTraceTerminalStatus::InProgress))
    }

    /// Approval continuation resumes the existing logical Turn. It may rebuild the in-memory
    /// accelerator after a process restart, but only from an exact durable trace identity.
    pub(super) fn ensure_conversation_turn_owner(
        &self,
        conversation_id: &str,
        run_id: &str,
        assistant_message_id: &str,
    ) -> Result<(), String> {
        let mut active_turns = self
            .active_conversation_turns
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if let Some(active) = active_turns.get(conversation_id) {
            return if active.run_id == run_id && active.assistant_message_id == assistant_message_id
            {
                Ok(())
            } else {
                Err(format!(
                    "conversation {conversation_id} is owned by active run {}",
                    active.run_id
                ))
            };
        }
        let durable = self
            .storage
            .get_conversation_turn_trace(assistant_message_id)?
            .ok_or_else(|| {
                format!(
                    "active conversation turn trace is missing for assistant {assistant_message_id}"
                )
            })?;
        if durable.conversation_id != conversation_id
            || durable.run_id != run_id
            || durable.terminal_status != ConversationTurnTraceTerminalStatus::InProgress
        {
            return Err("approval continuation does not own the durable active turn".to_string());
        }
        active_turns.insert(
            conversation_id.to_string(),
            ActiveConversationTurn {
                run_id: run_id.to_string(),
                assistant_message_id: assistant_message_id.to_string(),
            },
        );
        Ok(())
    }

    pub(super) fn release_conversation_turn_if_current(&self, conversation_id: &str, run_id: &str) {
        self.collaboration_run_directories.lock()
            .unwrap_or_else(|error| error.into_inner())
            .remove(run_id);
        let mut active_turns = self
            .active_conversation_turns
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if active_turns
            .get(conversation_id)
            .is_some_and(|active| active.run_id == run_id)
        {
            active_turns.remove(conversation_id);
        }
    }

    pub(super) fn restore_durable_conversation_turn_occupancies(&self) -> Result<(), String> {
        let durable = self.storage.list_in_progress_conversation_turn_traces()?;
        let mut active_turns = self
            .active_conversation_turns
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let mut permits = self
            .active_turn_permits
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        active_turns.clear();
        permits.clear();
        for trace in durable {
            let run_id = trace.run_id.clone();
            if active_turns
                .insert(
                    trace.conversation_id.clone(),
                    ActiveConversationTurn {
                        run_id: run_id.clone(),
                        assistant_message_id: trace.assistant_message_id,
                    },
                )
                .is_some()
            {
                return Err(format!(
                    "multiple durable in-progress turns exist for conversation {}",
                    trace.conversation_id
                ));
            }
            // Startup may find more durable Turns than a newly lowered limit. Recovered permits
            // count every survivor, deliberately blocking new root and child admission until the
            // active count falls below the configured process limit.
            if !self
                .storage
                .is_sync_human_interaction_run_waiting(&run_id)
                .map_err(|e| e.to_string())?
            {
                permits.insert(run_id, self.turn_concurrency_gate.adopt_recovered());
            }
        }
        Ok(())
    }

    /// Returns the process-local accelerator used by the Agent dispatcher while it observes a
    /// durable Turn. Callers must inspect SQLite before obtaining this value and once again after
    /// obtaining it; `Notify` is deliberately not an execution or completion source of truth.
    pub(crate) fn durable_turn_notification(
        &self,
        assistant_message_id: &str,
    ) -> Arc<tokio::sync::Notify> {
        let mut notifications = self
            .durable_turn_notifications
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        Arc::clone(
            notifications
                .entry(assistant_message_id.to_string())
                .or_insert_with(|| Arc::new(tokio::sync::Notify::new())),
        )
    }

    /// Publishes only after the durable Conversation boundary has committed. Notifications may
    /// be coalesced or lost across restart; every observer therefore re-reads SQLite.
    pub(super) fn notify_durable_turn_observers(&self, assistant_message_id: &str) {
        let notification = self
            .durable_turn_notifications
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .get(assistant_message_id)
            .cloned();
        if let Some(notification) = notification {
            notification.notify_waiters();
        }
    }

    pub(crate) fn turn_concurrency_gate(
        &self,
    ) -> crate::application::agent_dispatcher::AgentTurnConcurrencyGate {
        self.turn_concurrency_gate.clone()
    }

    pub(super) fn register_turn_concurrency_permit(
        &self,
        run_id: &str,
        permit: crate::application::agent_dispatcher::AgentTurnConcurrencyPermit,
    ) -> Result<(), AgentServiceError> {
        let mut permits = self
            .active_turn_permits
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if permits.contains_key(run_id) {
            return Err(
                format!("Agent Turn {run_id} already owns a global concurrency permit").into(),
            );
        }
        permits.insert(run_id.to_string(), permit);
        Ok(())
    }

    pub(super) fn ensure_turn_concurrency_permit(
        &self,
        run_id: &str,
    ) -> Result<(), AgentServiceError> {
        if self
            .active_turn_permits
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .contains_key(run_id)
        {
            return Ok(());
        }
        let permit = self
            .turn_concurrency_gate
            .try_acquire()
            .map_err(AgentServiceError::from)?;
        self.register_turn_concurrency_permit(run_id, permit)
    }

    pub(super) fn release_turn_concurrency_permit(&self, run_id: &str) {
        self.active_turn_permits
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .remove(run_id);
    }

    pub(crate) fn retain_recovered_turn_concurrency_permit(
        &self,
        run_id: &str,
    ) -> Option<crate::application::agent_dispatcher::AgentTurnConcurrencyPermit> {
        self.active_turn_permits
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .get(run_id)
            .cloned()
    }

    /// Called only after conservative Wake recovery atomically committed the terminal trace and
    /// direct-parent result Outbox. Exact identity checks prevent a stale observer from releasing
    /// a newer Conversation Turn.
    pub(crate) fn retire_recovered_turn_after_settlement(
        &self,
        conversation_id: &str,
        run_id: &str,
        assistant_message_id: &str,
    ) {
        let should_release = self
            .active_conversation_turns
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .get(conversation_id)
            .is_some_and(|active| {
                active.run_id == run_id && active.assistant_message_id == assistant_message_id
            });
        if should_release {
            self.release_conversation_turn_if_current(conversation_id, run_id);
            self.release_turn_concurrency_permit(run_id);
        }
    }
}
