impl AgentService {
    pub fn delete_project(&self, project_id: &str) -> Result<(), String> {
        let _admission = self.conversation_admission.lock().unwrap_or_else(|error|error.into_inner());
        for conversation in self.storage.load_conversations()?.iter().filter(|conversation| conversation.project_id.as_deref() == Some(project_id)) {
            self.ensure_no_manual_context_compaction(&conversation.id)?;
        }
        // Agent-bound projects are deletable: after this layer drains live execution and file
        // effects, StorageService removes every owned Agent tree in the same deletion transaction.
        // A graph-presence precheck here would bypass that authoritative cascade entirely.
        // Marking and effect registration use the same lock. Therefore every effect is either
        // already represented in `file_effects`, or observes the marker and never starts.
        {
            let mut lifecycle = self
                .deletion_lifecycle
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            if !lifecycle.projects.insert(project_id.to_string()) {
                return Err(format!(
                    "project `{project_id}` already has a deletion in progress"
                ));
            }
        }

        let automation_run_ids = match self
            .storage
            .list_nonterminal_automation_agent_run_ids_for_project(project_id)
        {
            Ok(run_ids) => run_ids.into_iter().collect::<HashSet<_>>(),
            Err(error) => {
                self.deletion_lifecycle
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .projects
                    .remove(project_id);
                return Err(error);
            }
        };
        let mut automation_wait_run_ids = automation_run_ids.iter().cloned().collect::<Vec<_>>();
        automation_wait_run_ids.sort();
        let mut run_ids = {
            let usage_contexts = self
                .usage_contexts
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            usage_contexts
                .iter()
                .filter(|(_, state)| state.context.project_id.as_deref() == Some(project_id))
                .map(|(run_id, _)| run_id.clone())
                .collect::<Vec<_>>()
        };
        let scope = FileEffectScope::Project(project_id.to_string());
        run_ids.extend(self.file_effects.active_run_ids(&scope));
        run_ids.extend(automation_run_ids.iter().cloned());
        run_ids.sort();
        run_ids.dedup();
        if let Err(error) = self.cancel_runs_for_destructive_mutation(&run_ids, &automation_run_ids)
        {
            self.deletion_lifecycle
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .projects
                .remove(project_id);
            return Err(error);
        }
        // A handed-off command is no longer owned by its Agent Run. Project deletion is an
        // explicit process-lifecycle boundary and therefore terminates those Sessions directly.
        self.command_sessions.terminate_project(project_id);

        if !self
            .file_effects
            .wait_until_idle(&scope, FILE_EFFECT_DRAIN_TIMEOUT)
        {
            self.deletion_lifecycle
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .projects
                .remove(project_id);
            return Err(format!(
                "project `{project_id}` was not deleted because an active file-producing action did not finish execution and durable settlement within {} ms; cancellation remains requested and deletion may be retried",
                FILE_EFFECT_DRAIN_TIMEOUT.as_millis()
            ));
        }

        let unsettled_effects = self.file_effects.unsettled_effect_ids(&scope);
        if !unsettled_effects.is_empty() {
            self.deletion_lifecycle
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .projects
                .remove(project_id);
            return Err(format!(
                "project `{project_id}` was not deleted because file-producing actions have effects without a confirmed durable terminal receipt: {}",
                unsettled_effects.join(", ")
            ));
        }
        if !self.wait_until_runs_inactive(&automation_wait_run_ids, FILE_EFFECT_DRAIN_TIMEOUT) {
            self.deletion_lifecycle
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .projects
                .remove(project_id);
            return Err(format!(
                "project `{project_id}` was not deleted because cancelled agent runs did not reach a safe terminal boundary within {} ms; deletion may be retried",
                FILE_EFFECT_DRAIN_TIMEOUT.as_millis()
            ));
        }

        #[cfg(test)]
        let result = if let Some(error) = take_project_deletion_failure(project_id) {
            Err(error)
        } else {
            self.storage.delete_project(project_id)
        };
        #[cfg(not(test))]
        let result = self.storage.delete_project(project_id);
        if result.is_ok() {
            self.invalidate_all_conversation_context_states();
            {
                let mut pending_actions = self
                    .pending_actions
                    .lock()
                    .unwrap_or_else(|error| error.into_inner());
                pending_actions.retain(|_, record| {
                    !agent_input_belongs_to_project(&record.agent_input, project_id)
                });
            }
            let mut usage_contexts = self
                .usage_contexts
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            usage_contexts
                .retain(|_, state| state.context.project_id.as_deref() != Some(project_id));
        }
        if result.is_err() {
            self.deletion_lifecycle
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .projects
                .remove(project_id);
        }
        result
    }

    pub(super) fn register_file_effect(
        &self,
        agent_input: &AgentChatInput,
        run_id: &str,
        effect_id: &str,
    ) -> AgentResult<FileEffectGuard> {
        let project_id = agent_input_project_id(agent_input);
        let conversation_id = agent_input_conversation_id(agent_input);
        let lifecycle = self
            .deletion_lifecycle
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if lifecycle.contains_input(agent_input) {
            return Err(AgentError::cancelled());
        }
        // Register while the lifecycle lock is held so neither deletion path can insert its
        // marker between the state check and the active-effect count increment.
        let guard = self
            .file_effects
            .register(project_id, conversation_id, run_id, effect_id);
        drop(lifecycle);
        Ok(guard)
    }

    #[cfg(test)]
    pub(super) fn unsettled_file_effect_ids_for_conversation(
        &self,
        conversation_id: &str,
    ) -> Vec<String> {
        self.file_effects
            .unsettled_effect_ids(&FileEffectScope::Conversation(conversation_id.to_string()))
    }

    pub fn delete_conversation(&self, conversation_id: &str) -> Result<(), String> {
        let _admission = self.conversation_admission.lock().unwrap_or_else(|error|error.into_inner());
        self.ensure_no_manual_context_compaction(conversation_id)?;
        self.authorize_user_conversation_write(conversation_id)
            .map_err(|error| error.to_string())?;
        // Root Agent conversations follow the same graph-aware storage transaction as project
        // deletion. Authorization above still prevents direct deletion of a child conversation.
        let project_id = self
            .storage
            .load_conversation(conversation_id)?
            .and_then(|conversation| conversation.project_id);
        let mut mutation_guard = ConversationMutationGuard::acquire(
            Arc::clone(&self.deletion_lifecycle),
            conversation_id,
            project_id.as_deref(),
        )?;

        let automation_run_ids = self
            .storage
            .list_nonterminal_automation_agent_run_ids_for_conversation(conversation_id)?
            .into_iter()
            .collect::<HashSet<_>>();
        let mut automation_wait_run_ids = automation_run_ids.iter().cloned().collect::<Vec<_>>();
        automation_wait_run_ids.sort();

        let mut run_ids = {
            let usage_contexts = self
                .usage_contexts
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            usage_contexts
                .iter()
                .filter(|(_, state)| state.context.conversation_id == conversation_id)
                .map(|(run_id, _)| run_id.clone())
                .collect::<Vec<_>>()
        };
        let scope = FileEffectScope::Conversation(conversation_id.to_string());
        run_ids.extend(self.file_effects.active_run_ids(&scope));
        run_ids.extend(automation_run_ids.iter().cloned());
        run_ids.sort();
        run_ids.dedup();
        self.cancel_runs_for_destructive_mutation(&run_ids, &automation_run_ids)?;
        // Agent cancellation deliberately does not reach handed-off Sessions; conversation
        // deletion does, and waits below for their terminal file-effect settlement.
        self.command_sessions
            .terminate_conversation(conversation_id);

        if !self
            .file_effects
            .wait_until_idle(&scope, FILE_EFFECT_DRAIN_TIMEOUT)
        {
            return Err(format!(
                "conversation `{conversation_id}` was not deleted because an active file-producing action did not finish execution and durable settlement within {} ms; cancellation remains requested and deletion may be retried",
                FILE_EFFECT_DRAIN_TIMEOUT.as_millis()
            ));
        }

        let unsettled_effects = self.file_effects.unsettled_effect_ids(&scope);
        if !unsettled_effects.is_empty() {
            return Err(format!(
                "conversation `{conversation_id}` was not deleted because file-producing actions have effects without a confirmed durable terminal receipt: {}",
                unsettled_effects.join(", ")
            ));
        }
        if !self.wait_until_runs_inactive(&automation_wait_run_ids, FILE_EFFECT_DRAIN_TIMEOUT) {
            return Err(format!(
                "conversation `{conversation_id}` was not deleted because cancelled agent runs did not reach a safe terminal boundary within {} ms; deletion may be retried",
                FILE_EFFECT_DRAIN_TIMEOUT.as_millis()
            ));
        }

        let result = self.storage.delete_conversation(conversation_id);
        if result.is_ok() {
            self.invalidate_conversation_context_state(conversation_id);
            self.pending_actions
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .retain(|_, record| {
                    record.snapshot.conversation_id.as_deref() != Some(conversation_id)
                });
            self.usage_contexts
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .retain(|_, state| state.context.conversation_id != conversation_id);
            let mut traces = self
                .trace_snapshots
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            for run_id in &run_ids {
                traces.remove(run_id);
            }
            // Conversation identifiers are immutable for this process lifetime. Retaining the
            // tombstone prevents a stale worker from recreating effects after durable deletion;
            // the temporary project marker is still released by the guard.
            mutation_guard.retain_conversation_tombstone();
        }
        result
    }

    pub fn delete_chat_messages(
        &self,
        conversation_id: &str,
        message_ids: &[String],
    ) -> Result<(), String> {
        let _admission = self.conversation_admission.lock().unwrap_or_else(|error|error.into_inner());
        self.ensure_no_manual_context_compaction(conversation_id)?;
        self.authorize_user_conversation_write(conversation_id)
            .map_err(|error| error.to_string())?;
        if message_ids.is_empty() {
            return Ok(());
        }

        let project_id = self
            .storage
            .load_conversation(conversation_id)?
            .and_then(|conversation| conversation.project_id);
        let _mutation_guard = ConversationMutationGuard::acquire(
            Arc::clone(&self.deletion_lifecycle),
            conversation_id,
            project_id.as_deref(),
        )?;
        let message_id_set = message_ids.iter().cloned().collect::<HashSet<_>>();
        let automation_run_ids = self
            .storage
            .list_nonterminal_automation_agent_run_ids_for_messages(conversation_id, message_ids)?
            .into_iter()
            .collect::<HashSet<_>>();

        let (mut run_ids, mut retired_run_ids) = {
            let usage_contexts = self
                .usage_contexts
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            let run_ids = usage_contexts
                .iter()
                .filter(|(_, state)| state.context.conversation_id == conversation_id)
                .map(|(run_id, _)| run_id.clone())
                .collect::<Vec<_>>();
            let retired_run_ids = usage_contexts
                .iter()
                .filter(|(_, state)| {
                    state.context.conversation_id == conversation_id
                        && message_id_set.contains(&state.context.assistant_message_id)
                })
                .map(|(run_id, _)| run_id.clone())
                .collect::<HashSet<_>>();
            (run_ids, retired_run_ids)
        };
        // An explicitly approved process is registered before its worker is spawned. Include
        // message-owned pending records so cancellation and the process guard cover that narrow
        // approved-to-worker-start interval even when the ephemeral usage context is absent.
        run_ids.extend({
            let pending_actions = self
                .pending_actions
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            pending_actions
                .values()
                .filter(|record| {
                    record.snapshot.conversation_id.as_deref() == Some(conversation_id)
                        && record
                            .snapshot
                            .assistant_message_id
                            .as_ref()
                            .is_some_and(|message_id| message_id_set.contains(message_id))
                })
                .map(|record| record.snapshot.run_id.clone())
                .collect::<Vec<_>>()
        });
        let scope = FileEffectScope::Conversation(conversation_id.to_string());
        run_ids.extend(self.file_effects.active_run_ids(&scope));
        run_ids.extend(automation_run_ids.iter().cloned());
        run_ids.sort();
        run_ids.dedup();
        self.cancel_runs_for_destructive_mutation(&run_ids, &automation_run_ids)?;
        self.command_sessions
            .terminate_messages(conversation_id, &message_id_set);

        if !self
            .file_effects
            .wait_until_idle(&scope, FILE_EFFECT_DRAIN_TIMEOUT)
        {
            return Err(format!(
                "messages were not deleted because an active file-producing action did not finish execution and durable settlement within {} ms; cancellation remains requested and deletion may be retried",
                FILE_EFFECT_DRAIN_TIMEOUT.as_millis()
            ));
        }
        let unsettled_effects = self.file_effects.unsettled_effect_ids(&scope);
        if !unsettled_effects.is_empty() {
            return Err(format!(
                "messages were not deleted because file-producing actions have effects without a confirmed durable terminal receipt: {}",
                unsettled_effects.join(", ")
            ));
        }
        if !self.wait_until_runs_inactive(&run_ids, FILE_EFFECT_DRAIN_TIMEOUT) {
            return Err(format!(
                "messages were not deleted because cancelled agent runs did not reach a safe terminal boundary within {} ms; deletion may be retried",
                FILE_EFFECT_DRAIN_TIMEOUT.as_millis()
            ));
        }

        // Check resumable approvals only after draining. A successfully settled action can still
        // be momentarily `approved`/`executing` in memory after its atomic audit+target+trace
        // receipt committed; rejecting it before the drain can strand that continuation forever.
        // At this point the marker prevents new work, all runs are inactive, and the effect tracker
        // proved there is no unreceipted side effect. A genuinely `pending` action remains
        // user-resumable and must be cancelled/rejected before its owner message can be removed.
        let pending_blockers = {
            let pending_actions = self
                .pending_actions
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            let mut blockers = pending_actions
                .values()
                .filter(|record| {
                    record.snapshot.conversation_id.as_deref() == Some(conversation_id)
                        && record
                            .snapshot
                            .assistant_message_id
                            .as_ref()
                            .is_some_and(|message_id| message_id_set.contains(message_id))
                        && record.snapshot.status == PendingActionStatus::Pending
                })
                .map(|record| record.snapshot.action_id.clone())
                .collect::<Vec<_>>();
            blockers.sort();
            blockers.truncate(8);
            blockers
        };
        if !pending_blockers.is_empty() {
            return Err(format!(
                "messages were not deleted because they own pending actions; cancel or reject these actions first: {}",
                pending_blockers.join(", ")
            ));
        }

        self.storage
            .delete_chat_messages(conversation_id, message_ids)?;
        retired_run_ids.extend({
            let mut pending_actions = self
                .pending_actions
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            let run_ids = pending_actions
                .values()
                .filter(|record| {
                    record.snapshot.conversation_id.as_deref() == Some(conversation_id)
                        && record
                            .snapshot
                            .assistant_message_id
                            .as_ref()
                            .is_some_and(|message_id| message_id_set.contains(message_id))
                })
                .map(|record| record.snapshot.run_id.clone())
                .collect::<HashSet<_>>();
            pending_actions.retain(|_, record| {
                record.snapshot.conversation_id.as_deref() != Some(conversation_id)
                    || record
                        .snapshot
                        .assistant_message_id
                        .as_ref()
                        .is_none_or(|message_id| !message_id_set.contains(message_id))
            });
            run_ids
        });
        if !retired_run_ids.is_empty() {
            self.usage_contexts
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .retain(|run_id, _| !retired_run_ids.contains(run_id));
            self.trace_snapshots
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .retain(|run_id, _| !retired_run_ids.contains(run_id));
        }
        self.invalidate_conversation_context_state(conversation_id);
        Ok(())
    }

    fn wait_until_runs_inactive(&self, run_ids: &[String], timeout: Duration) -> bool {
        if run_ids.is_empty() {
            return true;
        }
        let deadline = std::time::Instant::now() + timeout;
        loop {
            let cancellation_active = {
                let cancellations = self
                    .cancellations
                    .lock()
                    .unwrap_or_else(|error| error.into_inner());
                run_ids
                    .iter()
                    .any(|run_id| cancellations.contains_key(run_id))
            };
            let process_active = run_ids
                .iter()
                .any(|run_id| self.process_runs.has_active_run(run_id));
            if !cancellation_active && !process_active {
                return true;
            }
            if std::time::Instant::now() >= deadline {
                return false;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    }
}
