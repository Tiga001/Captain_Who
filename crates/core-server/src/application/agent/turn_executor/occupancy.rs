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
            .get_in_progress_conversation_turn_identity(conversation_id)?
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
            .get_in_progress_conversation_turn_identity(conversation_id)?
            .is_some())
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
            .get_in_progress_conversation_turn_identity(conversation_id)?
            .ok_or_else(|| {
                format!(
                    "active conversation turn trace is missing for assistant {assistant_message_id}"
                )
            })?;
        if durable.conversation_id != conversation_id
            || durable.run_id != run_id
            || durable.assistant_message_id != assistant_message_id
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
        drop(active_turns);
        self.finish_active_run_steering(
            run_id,
            "The agent run has finished and no longer accepts guidance.",
        );
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

#[cfg(test)]
mod occupancy_tests {
    use super::*;
    use mycopilot_core::storage::models::{ChatConversationRecord, ChatMessageRecord};
    use std::sync::Barrier;

    fn fixture() -> (tempfile::TempDir, Arc<StorageService>, AgentService) {
        let directory = tempfile::tempdir().unwrap();
        let storage =
            Arc::new(StorageService::open(&directory.path().join("storage.sqlite")).unwrap());
        storage
            .save_conversation(ChatConversationRecord {
                id: "conversation".into(),
                project_id: None,
                model_id: None,
                title: "Occupancy".into(),
                messages: vec![ChatMessageRecord {
                    id: "assistant".into(),
                    role: "assistant".into(),
                    content: String::new(),
                    created_at: 1,
                    status: Some("pending".into()),
                    attachments: Vec::new(),
                    folder_references_json: None,
                    agent_run_json: None,
                    ui_state_json: None,
                    human_interaction_response: None,
                }],
                created_at: 1,
                updated_at: 1,
                pinned_at: None,
                archived_at: None,
                unread_at: None,
            })
            .unwrap();
        let service = AgentService::new_authorized_for_test(Arc::clone(&storage));
        (directory, storage, service)
    }

    #[test]
    fn concurrent_admission_has_one_winner_and_failed_preparation_can_release_it() {
        let (_directory, _storage, service) = fixture();
        let barrier = Barrier::new(2);
        let winners = std::thread::scope(|scope| {
            let starts = ["run-a", "run-b"].map(|run_id| {
                let service = &service;
                let barrier = &barrier;
                scope.spawn(move || {
                    barrier.wait();
                    service
                        .reserve_conversation_turn("conversation", run_id, "assistant")
                        .is_ok()
                        .then_some(run_id)
                })
            });
            starts
                .into_iter()
                .filter_map(|start| start.join().unwrap())
                .collect::<Vec<_>>()
        });
        assert_eq!(winners.len(), 1);
        assert!(service
            .has_conversation_turn_occupancy("conversation")
            .unwrap());
        service.release_conversation_turn_if_current("conversation", "stale-run");
        assert!(service
            .has_conversation_turn_occupancy("conversation")
            .unwrap());
        service.release_conversation_turn_if_current("conversation", winners[0]);
        assert!(!service
            .has_conversation_turn_occupancy("conversation")
            .unwrap());
        service
            .reserve_conversation_turn("conversation", "retry-run", "assistant")
            .unwrap();
    }

    #[test]
    fn durable_occupancy_survives_restart_without_a_resident_runtime_and_requires_exact_owner() {
        let (directory, storage, service) = fixture();
        let trace = ConversationTraceSnapshot::default().in_progress_trace(
            "durable-run",
            "conversation",
            "assistant",
        );
        storage
            .append_in_progress_conversation_turn_trace(&trace, 1, 1)
            .unwrap();
        drop(service);
        drop(storage);
        let storage =
            Arc::new(StorageService::open(&directory.path().join("storage.sqlite")).unwrap());
        let service =
            AgentService::try_new_deferred_startup_reconciliation(Arc::clone(&storage)).unwrap();
        service.active_conversation_turns.lock().unwrap().clear();

        assert!(service
            .has_conversation_turn_occupancy("conversation")
            .unwrap());
        assert!(service
            .reserve_conversation_turn("conversation", "other-run", "other-message")
            .is_err());
        assert!(service
            .ensure_conversation_turn_owner("conversation", "other-run", "assistant")
            .is_err());
        assert!(service
            .ensure_conversation_turn_owner("conversation", "durable-run", "other-message")
            .is_err());
        assert!(service.active_conversation_turns.lock().unwrap().is_empty());
        service
            .ensure_conversation_turn_owner("conversation", "durable-run", "assistant")
            .unwrap();
        service.release_conversation_turn_if_current("conversation", "durable-run");
        assert!(
            service
                .has_conversation_turn_occupancy("conversation")
                .unwrap(),
            "releasing the accelerator cannot release an uncommitted terminal Turn"
        );
    }

    #[test]
    fn terminal_turn_cannot_reacquire_approval_ownership() {
        let mut trace = ConversationTraceSnapshot::default().in_progress_trace(
            "durable-run",
            "conversation",
            "assistant",
        );
        for status in [
            ConversationTurnTraceTerminalStatus::Completed,
            ConversationTurnTraceTerminalStatus::Failed,
            ConversationTurnTraceTerminalStatus::Cancelled,
        ] {
            // Each fixture has one terminal transition; terminal-to-terminal changes are not
            // part of the production persistence contract.
            let (_directory, storage, service) = fixture();
            trace.terminal_status = status;
            storage
                .replace_conversation_turn_trace(&trace, 1, 2)
                .unwrap();
            assert!(!service
                .has_conversation_turn_occupancy("conversation")
                .unwrap());
            assert!(service
                .ensure_conversation_turn_owner("conversation", "durable-run", "assistant")
                .is_err());
        }
    }

    #[test]
    fn manual_compaction_keeps_occupancy_until_its_cancelled_worker_drains() {
        use mycopilot_core::storage::models::ManualContextCompactionOperation;

        let (_directory, storage, service) = fixture();
        let mut operation = ManualContextCompactionOperation {
            operation_id: "compaction".into(),
            request_id: "compaction-request".into(),
            conversation_id: "conversation".into(),
            status: "running".into(),
            phase: "preparing".into(),
            assistant_message_id: None,
            covered_through_message_id: None,
            model_id: None,
            summary_id: None,
            source_input_tokens: None,
            replacement_input_tokens: None,
            error: None,
            started_at: 1,
            updated_at: 1,
            completed_at: None,
        };
        storage.claim_manual_context_compaction(&operation).unwrap();
        assert!(service
            .has_conversation_turn_occupancy("conversation")
            .unwrap());
        assert!(service
            .reserve_conversation_turn("conversation", "new-run", "assistant")
            .is_err());
        service
            .manual_context_compaction_cancellations
            .lock()
            .unwrap()
            .insert(
                operation.operation_id.clone(),
                AgentCancellationToken::new(),
            );
        operation.status = "cancelled".into();
        operation.updated_at = 2;
        operation.completed_at = Some(2);
        storage
            .update_manual_context_compaction(&operation)
            .unwrap();
        assert!(service
            .has_conversation_turn_occupancy("conversation")
            .unwrap());
        service
            .manual_context_compaction_cancellations
            .lock()
            .unwrap()
            .clear();
        assert!(!service
            .has_conversation_turn_occupancy("conversation")
            .unwrap());
    }
}
