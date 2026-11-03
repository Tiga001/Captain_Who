impl AgentService {
    pub fn delete_project(&self, project_id: &str) -> Result<(), String> {
        let initial_conversation_ids = self
            .storage
            .load_conversation_metas()?
            .into_iter()
            .filter(|conversation| conversation.project_id.as_deref() == Some(project_id))
            .map(|conversation| conversation.id)
            .collect::<Vec<_>>();
        for conversation_id in &initial_conversation_ids {
            self.ensure_no_manual_context_compaction(conversation_id)?;
        }
        // The marker is ordered against final Turn admission, but no writer/registry lock is
        // retained while waiting for this project's preparation, mutation or file owners.
        let mut mutation_guard = self.storage.coordinate_runtime_control(|| {
            ProjectMutationGuard::acquire(
                Arc::clone(&self.deletion_lifecycle),
                Arc::clone(&self.preparing),
                project_id,
            )
        })?;
        if !mutation_guard.wait_local_mutations(FILE_EFFECT_DRAIN_TIMEOUT) {
            return Err(format!(
                "project `{project_id}` was not deleted because an existing conversation mutation did not finish within {} ms; deletion may be retried",
                FILE_EFFECT_DRAIN_TIMEOUT.as_millis()
            ));
        }
        let mut deleted_conversation_ids = self.storage.coordinate_project_conversations_control(
            project_id,
            |conversation_ids| {
                mutation_guard.include_conversations(conversation_ids.clone())?;
                Ok(conversation_ids.into_iter().collect::<HashSet<_>>())
            },
        )?;
        for conversation_id in &deleted_conversation_ids.iter().cloned().collect::<Vec<_>>() {
            self.ensure_no_manual_context_compaction(conversation_id)?;
            if let Some(agent) = self
                .storage
                .get_agent_node_by_conversation(conversation_id)
                .map_err(|error| error.to_string())?
            {
                if agent.project_id.as_deref() != Some(project_id) {
                    return Err(
                        "Project deletion found an Agent outside its authoritative project scope."
                            .into(),
                    );
                }
                if agent.parent_agent_id.is_none() {
                    let tree = self
                        .resolve_conversation_deletion_scope(conversation_id, Some(project_id))?;
                    deleted_conversation_ids.extend(tree);
                    self.stop_agent_tree_for_deletion(conversation_id)?;
                }
            }
        }
        let mut automation_run_ids = self
            .storage
            .list_nonterminal_automation_agent_run_ids_for_project(project_id)?
            .into_iter()
            .collect::<HashSet<_>>();
        for conversation_id in &deleted_conversation_ids {
            automation_run_ids.extend(
                self.storage
                    .list_nonterminal_automation_agent_run_ids_for_conversation(conversation_id)?,
            );
        }
        let mut run_ids = {
            let usage_contexts = self
                .usage_contexts
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            usage_contexts
                .iter()
                .filter(|(_, state)| {
                    state.context.project_id.as_deref() == Some(project_id)
                        || deleted_conversation_ids.contains(&state.context.conversation_id)
                })
                .map(|(run_id, _)| run_id.clone())
                .collect::<Vec<_>>()
        };
        let scope = FileEffectScope::Project(project_id.to_string());
        let mut scopes = vec![scope];
        let mut conversation_ids = deleted_conversation_ids.iter().cloned().collect::<Vec<_>>();
        conversation_ids.sort();
        scopes.extend(
            conversation_ids
                .iter()
                .cloned()
                .map(FileEffectScope::Conversation),
        );
        for scope in &scopes {
            run_ids.extend(self.file_effects.active_run_ids(scope));
        }
        run_ids.extend(automation_run_ids.iter().cloned());
        run_ids.sort();
        run_ids.dedup();
        self.cancel_runs_for_destructive_mutation(&run_ids, &automation_run_ids)?;
        // A handed-off command is no longer owned by its Agent Run. Project deletion is an
        // explicit process-lifecycle boundary and therefore terminates those Sessions directly.
        self.command_sessions.terminate_project(project_id);
        for conversation_id in &conversation_ids {
            self.command_sessions
                .terminate_conversation(conversation_id);
        }

        if !mutation_guard
            .preparation
            .wait_idle(FILE_EFFECT_DRAIN_TIMEOUT)
        {
            return Err(format!(
                "project `{project_id}` was not deleted because cancelled preparation did not finish within {} ms; deletion may be retried",
                FILE_EFFECT_DRAIN_TIMEOUT.as_millis()
            ));
        }

        let drain_started = Instant::now();
        if !scopes.iter().all(|scope| {
            self.file_effects.wait_until_idle(
                scope,
                FILE_EFFECT_DRAIN_TIMEOUT.saturating_sub(drain_started.elapsed()),
            )
        }) {
            return Err(format!(
                "project `{project_id}` was not deleted because an active file-producing action did not finish execution and durable settlement within {} ms; cancellation remains requested and deletion may be retried",
                FILE_EFFECT_DRAIN_TIMEOUT.as_millis()
            ));
        }

        let unsettled_effects = scopes
            .iter()
            .flat_map(|scope| self.file_effects.unsettled_effect_ids(scope))
            .collect::<Vec<_>>();
        if !unsettled_effects.is_empty() {
            return Err(format!(
                "project `{project_id}` was not deleted because file-producing actions have effects without a confirmed durable terminal receipt: {}",
                unsettled_effects.join(", ")
            ));
        }
        if !self.wait_until_runs_inactive(&run_ids, FILE_EFFECT_DRAIN_TIMEOUT) {
            return Err(format!(
                "project `{project_id}` was not deleted because cancelled agent runs did not reach a safe terminal boundary within {} ms; deletion may be retried",
                FILE_EFFECT_DRAIN_TIMEOUT.as_millis()
            ));
        }

        #[cfg(test)]
        let result = if let Some(error) = take_project_deletion_failure(project_id) {
            Err(error)
        } else {
            self.storage
                .delete_project_checked_members(project_id, &conversation_ids)
        };
        #[cfg(not(test))]
        let result = self
            .storage
            .delete_project_checked_members(project_id, &conversation_ids);
        if result.is_ok() {
            self.invalidate_all_conversation_context_states();
            {
                let mut pending_actions = self
                    .pending_actions
                    .lock()
                    .unwrap_or_else(|error| error.into_inner());
                pending_actions.retain(|_, record| {
                    !agent_input_belongs_to_project(&record.agent_input, project_id)
                        && record
                            .snapshot
                            .conversation_id
                            .as_ref()
                            .is_none_or(|id| !deleted_conversation_ids.contains(id))
                });
            }
            let mut usage_contexts = self
                .usage_contexts
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            usage_contexts.retain(|_, state| {
                state.context.project_id.as_deref() != Some(project_id)
                    && !deleted_conversation_ids.contains(&state.context.conversation_id)
            });
            drop(usage_contexts);
            self.observer_streams
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .retain(|conversation_id, state| {
                    !deleted_conversation_ids.contains(conversation_id)
                        && !deleted_conversation_ids.contains(&state.root_conversation_id)
                });
            mutation_guard.retain_project_tombstone();
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

    /// An approval's frozen validation/decision owns a scoped activity even before its actual
    /// file/process effect is known. Deletion drains this guard; it never counts as an unreceipted
    /// file change because callers must register their real effects separately.
    pub(super) fn register_lifecycle_activity(
        &self,
        agent_input: &AgentChatInput,
        run_id: &str,
    ) -> Result<FileEffectGuard, String> {
        self.register_file_effect(agent_input, run_id, &format!("lifecycle:{run_id}"))
            .map_err(|error| error.to_string())
    }

    fn resolve_conversation_deletion_scope(
        &self,
        conversation_id: &str,
        project_id: Option<&str>,
    ) -> Result<HashSet<String>, String> {
        let mut conversation_ids = HashSet::from([conversation_id.to_string()]);
        let Some(root) = self
            .storage
            .get_agent_node_by_conversation(conversation_id)
            .map_err(|error| error.to_string())?
        else {
            return Ok(conversation_ids);
        };
        if root.parent_agent_id.is_some() || root.project_id.as_deref() != project_id {
            return Err("Conversation deletion does not own the Agent root/project scope.".into());
        }
        for node in self
            .storage
            .list_agent_tree(&root.agent_id)
            .map_err(|error| error.to_string())?
        {
            if node.root_agent_id != root.agent_id
                || node.root_conversation_id != conversation_id
                || node.project_id.as_deref() != project_id
            {
                return Err(
                    "Agent tree deletion escaped its authoritative root/project scope.".into(),
                );
            }
            conversation_ids.insert(node.conversation_id);
        }
        Ok(conversation_ids)
    }

    /// Deletion owns cancellation of the entire cascade, but it must not manufacture the user
    /// interrupted marker or route through the public Stop RPC. Keep the existing durable tree
    /// stop when a root Run is active, and sweep exact descendant Wakes when only children remain.
    fn stop_agent_tree_for_deletion(&self, conversation_id: &str) -> Result<(), String> {
        let Some(root) = self
            .storage
            .get_agent_node_by_conversation(conversation_id)
            .map_err(|error| error.to_string())?
        else {
            return Ok(());
        };
        if root.parent_agent_id.is_some() {
            return Err(
                "Only an authoritative root Conversation may delete its Agent tree.".into(),
            );
        }
        self.storage
            .workflow_execution_pause_conversation(conversation_id)?;
        self.workflow_readiness_changed(Some(conversation_id));
        let stop = self
            .storage
            .get_in_progress_conversation_turn_identity(conversation_id)?
            .map(|identity| {
                self.storage
                    .begin_agent_tree_run_stop_by_root_agent(&root.agent_id, &identity.run_id)
            })
            .transpose()
            .map_err(|error| error.to_string())?
            .flatten();
        let batch = match stop {
            Some(stop) => stop.cancellation,
            None => self
                .storage
                .cancel_agent_tree_wakes_by_root_agent(&root.agent_id)
                .map_err(|error| error.to_string())?,
        };
        let mut progress = self.cancel_agent_tree_wake_batch(batch);
        if progress.lost_ownership {
            progress.errors.push("Agent tree ownership changed during deletion cancellation; retry after settlement.".into());
        }
        if progress.errors.is_empty() {
            Ok(())
        } else {
            Err(progress.errors.join("；"))
        }
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
        self.ensure_no_manual_context_compaction(conversation_id)?;
        self.authorize_user_conversation_write(conversation_id)
            .map_err(|error| error.to_string())?;
        // Root Agent conversations follow the same graph-aware storage transaction as project
        // deletion. Authorization above still prevents direct deletion of a child conversation.
        let project_id = self
            .storage
            .load_conversation_meta(conversation_id)?
            .and_then(|conversation| conversation.project_id);
        let mut mutation_guard = self.storage.coordinate_conversation_control(
            conversation_id,
            project_id.as_deref(),
            || {
                ConversationMutationGuard::acquire(
                    Arc::clone(&self.deletion_lifecycle),
                    Arc::clone(&self.preparing),
                    vec![conversation_id.to_string()],
                    project_id.as_deref(),
                )
            },
        )?;
        self.stop_agent_tree_for_deletion(conversation_id)?;
        let deleted_conversation_ids =
            self.resolve_conversation_deletion_scope(conversation_id, project_id.as_deref())?;
        self.storage.coordinate_runtime_control(|| {
            mutation_guard.extend_conversations(deleted_conversation_ids.iter().cloned().collect())
        })?;
        for affected in &deleted_conversation_ids {
            self.ensure_no_manual_context_compaction(affected)?;
        }

        let mut automation_run_ids = HashSet::new();
        for affected in &deleted_conversation_ids {
            automation_run_ids.extend(
                self.storage
                    .list_nonterminal_automation_agent_run_ids_for_conversation(affected)?,
            );
        }

        let mut run_ids = {
            let usage_contexts = self
                .usage_contexts
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            usage_contexts
                .iter()
                .filter(|(_, state)| {
                    deleted_conversation_ids.contains(&state.context.conversation_id)
                })
                .map(|(run_id, _)| run_id.clone())
                .collect::<Vec<_>>()
        };
        let scopes = deleted_conversation_ids
            .iter()
            .cloned()
            .map(FileEffectScope::Conversation)
            .collect::<Vec<_>>();
        for scope in &scopes {
            run_ids.extend(self.file_effects.active_run_ids(scope));
        }
        run_ids.extend(automation_run_ids.iter().cloned());
        run_ids.sort();
        run_ids.dedup();
        self.cancel_runs_for_destructive_mutation(&run_ids, &automation_run_ids)?;
        // Agent cancellation deliberately does not reach handed-off Sessions; conversation
        // deletion does, and waits below for their terminal file-effect settlement.
        for affected in &deleted_conversation_ids {
            self.predictions.invalidate_chat(affected);
            self.command_sessions.terminate_conversation(affected);
        }

        if !mutation_guard
            .preparation
            .wait_idle(FILE_EFFECT_DRAIN_TIMEOUT)
        {
            return Err(format!(
                "conversation `{conversation_id}` was not deleted because cancelled preparation did not finish within {} ms; deletion may be retried",
                FILE_EFFECT_DRAIN_TIMEOUT.as_millis()
            ));
        }

        let drain_started = Instant::now();
        if !scopes.iter().all(|scope| {
            self.file_effects.wait_until_idle(
                scope,
                FILE_EFFECT_DRAIN_TIMEOUT.saturating_sub(drain_started.elapsed()),
            )
        }) {
            return Err(format!(
                "conversation `{conversation_id}` was not deleted because an active file-producing action did not finish execution and durable settlement within {} ms; cancellation remains requested and deletion may be retried",
                FILE_EFFECT_DRAIN_TIMEOUT.as_millis()
            ));
        }

        let unsettled_effects = scopes
            .iter()
            .flat_map(|scope| self.file_effects.unsettled_effect_ids(scope))
            .collect::<Vec<_>>();
        if !unsettled_effects.is_empty() {
            return Err(format!(
                "conversation `{conversation_id}` was not deleted because file-producing actions have effects without a confirmed durable terminal receipt: {}",
                unsettled_effects.join(", ")
            ));
        }
        if !self.wait_until_runs_inactive(&run_ids, FILE_EFFECT_DRAIN_TIMEOUT) {
            return Err(format!(
                "conversation `{conversation_id}` was not deleted because cancelled agent runs did not reach a safe terminal boundary within {} ms; deletion may be retried",
                FILE_EFFECT_DRAIN_TIMEOUT.as_millis()
            ));
        }

        let result = self.storage.delete_conversation(conversation_id);
        if result.is_ok() {
            for affected in &deleted_conversation_ids {
                self.invalidate_conversation_context_state(affected);
            }
            self.pending_actions
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .retain(|_, record| {
                    record
                        .snapshot
                        .conversation_id
                        .as_ref()
                        .is_none_or(|id| !deleted_conversation_ids.contains(id))
                });
            self.usage_contexts
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .retain(|_, state| {
                    !deleted_conversation_ids.contains(&state.context.conversation_id)
                });
            let mut traces = self
                .trace_snapshots
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            for run_id in &run_ids {
                traces.remove(run_id);
            }
            drop(traces);
            self.observer_streams
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .retain(|stream_conversation_id, state| {
                    !deleted_conversation_ids.contains(stream_conversation_id)
                        && !deleted_conversation_ids.contains(&state.root_conversation_id)
                });
            // Conversation identifiers are immutable for this process lifetime. Retaining the
            // tombstone prevents a stale worker from recreating effects after durable deletion;
            // the project-local mutation registration is still released by the guard.
            mutation_guard.retain_conversation_tombstone();
        }
        result
    }

    pub fn delete_chat_messages(
        &self,
        conversation_id: &str,
        message_ids: &[String],
    ) -> Result<(), String> {
        self.ensure_no_manual_context_compaction(conversation_id)?;
        self.authorize_user_conversation_write(conversation_id)
            .map_err(|error| error.to_string())?;
        if message_ids.is_empty() {
            return Ok(());
        }

        let project_id = self
            .storage
            .load_conversation_meta(conversation_id)?
            .and_then(|conversation| conversation.project_id);
        let mutation_guard = self.storage.coordinate_conversation_control(
            conversation_id,
            project_id.as_deref(),
            || {
                ConversationMutationGuard::acquire(
                    Arc::clone(&self.deletion_lifecycle),
                    Arc::clone(&self.preparing),
                    vec![conversation_id.to_string()],
                    project_id.as_deref(),
                )
            },
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

        if !mutation_guard
            .preparation
            .wait_idle(FILE_EFFECT_DRAIN_TIMEOUT)
        {
            return Err(format!(
                "messages were not deleted because cancelled preparation did not finish within {} ms; deletion may be retried",
                FILE_EFFECT_DRAIN_TIMEOUT.as_millis()
            ));
        }

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
        self.observer_streams
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .retain(|stream_conversation_id, state| {
                !retired_run_ids.contains(&state.snapshot.run_id)
                    && (stream_conversation_id != conversation_id
                        || !message_id_set.contains(&state.snapshot.assistant_message_id))
            });
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

#[cfg(test)]
mod preparing_deletion_tests {
    use super::*;
    use mycopilot_core::storage::models::ChatConversationRecord;

    fn service(path: &Path) -> AgentService {
        AgentService::new(Arc::new(
            StorageService::open_with_model_credentials(
                path,
                Arc::new(mycopilot_core::image_generation::InMemoryCredentialStore::default()),
            )
            .unwrap(),
        ))
    }

    fn seed(service: &AgentService, id: &str) {
        service
            .storage
            .save_conversation(ChatConversationRecord {
                id: id.into(),
                title: id.into(),
                project_id: None,
                model_id: None,
                messages: Vec::new(),
                created_at: 1,
                updated_at: 1,
                pinned_at: None,
                archived_at: None,
                unread_at: None,
            })
            .unwrap();
    }

    #[test]
    fn deleting_one_preparing_conversation_allows_another_to_progress() {
        let directory = tempfile::tempdir().unwrap();
        let service = service(&directory.path().join("delete.sqlite"));
        seed(&service, "a");
        seed(&service, "b");
        let cancellation = AgentCancellationToken::new();
        let preparing = service
            .preparing
            .register(
                "a",
                None,
                "run-a",
                "assistant-a",
                cancellation.clone(),
                None,
            )
            .unwrap();
        let deleting = service.clone();
        let (send, receive) = std::sync::mpsc::sync_channel(1);
        let worker =
            std::thread::spawn(move || send.send(deleting.delete_conversation("a")).unwrap());
        let started = Instant::now();
        while !service.is_conversation_deleting(Some("a"))
            && started.elapsed() < Duration::from_secs(2)
        {
            std::thread::sleep(Duration::from_millis(1));
        }
        assert!(service.is_conversation_deleting(Some("a")));
        assert!(cancellation.is_cancelled());
        assert!(receive.recv_timeout(Duration::from_millis(10)).is_err());
        let unrelated = service
            .preparing
            .register(
                "b",
                None,
                "run-b",
                "assistant-b",
                AgentCancellationToken::new(),
                None,
            )
            .unwrap();
        assert!(unrelated.validate().is_ok());
        drop(preparing);
        receive
            .recv_timeout(Duration::from_secs(2))
            .unwrap()
            .unwrap();
        worker.join().unwrap();
        assert!(service
            .storage
            .load_conversation_meta("a")
            .unwrap()
            .is_none());
        assert!(service
            .storage
            .load_conversation_meta("b")
            .unwrap()
            .is_some());
        assert!(unrelated.validate().is_ok());
    }

    #[test]
    fn preparation_drain_timeout_keeps_data_and_never_revives_cancelled_generation() {
        let directory = tempfile::tempdir().unwrap();
        let service = service(&directory.path().join("delete.sqlite"));
        seed(&service, "a");
        let old = service
            .preparing
            .register(
                "a",
                None,
                "run-old",
                "assistant-old",
                AgentCancellationToken::new(),
                None,
            )
            .unwrap();
        let error = service.delete_conversation("a").unwrap_err();
        assert!(
            error.contains("cancelled preparation did not finish"),
            "{error}"
        );
        assert!(service
            .storage
            .load_conversation_meta("a")
            .unwrap()
            .is_some());
        assert!(!service.is_conversation_deleting(Some("a")));
        assert!(old.validate().is_err());
        assert!(service
            .preparing
            .register(
                "a",
                None,
                "run-new",
                "assistant-new",
                AgentCancellationToken::new(),
                None
            )
            .is_err());
        drop(old);
        let fresh = service
            .preparing
            .register(
                "a",
                None,
                "run-new",
                "assistant-new",
                AgentCancellationToken::new(),
                None,
            )
            .unwrap();
        assert!(fresh.validate().is_ok());
    }

    fn effect_input(conversation_id: &str, project_id: Option<&str>) -> AgentChatInput {
        let mut input: AgentChatInput = serde_json::from_value(serde_json::json!({
            "apiUrl": "https://example.test/v1/chat/completions",
            "apiToken": "test-token",
            "model": "test-model",
            "modelCapabilities": { "imageInput": false },
            "messages": []
        }))
        .unwrap();
        input.context = Some(AgentRunContext {
            conversation_id: Some(conversation_id.into()),
            project_id: project_id.map(ToString::to_string),
            workspace: None,
            attachment_library: None,
            permissions: AgentPermissions::default(),
            collaboration_identity: None,
        });
        input
    }

    #[test]
    fn conversation_delete_drains_its_effect_without_blocking_same_project_work() {
        let directory = tempfile::tempdir().unwrap();
        let service = service(&directory.path().join("delete-effects.sqlite"));
        service
            .storage
            .save_project(
                mycopilot_core::storage::models::ProjectRecord::without_folders(
                    "project", "Project", 1,
                ),
            )
            .unwrap();
        for id in ["a", "b"] {
            seed(&service, id);
            let mut metadata = service.storage.load_conversation_meta(id).unwrap().unwrap();
            metadata.project_id = Some("project".into());
            service.storage.save_conversation_meta(metadata).unwrap();
        }
        let input = effect_input("a", Some("project"));
        let mut effect = service
            .register_file_effect(&input, "run-a", "effect-a")
            .unwrap();
        effect.mark_effects_started();
        let deleting = service.clone();
        let (send, receive) = std::sync::mpsc::sync_channel(1);
        let worker =
            std::thread::spawn(move || send.send(deleting.delete_conversation("a")).unwrap());
        let started = Instant::now();
        while !service.is_conversation_deleting(Some("a"))
            && started.elapsed() < Duration::from_secs(2)
        {
            std::thread::sleep(Duration::from_millis(1));
        }
        assert!(service.is_conversation_deleting(Some("a")));
        assert!(receive.try_recv().is_err());
        assert!(service
            .register_file_effect(&input, "late-a", "late-effect")
            .is_err());
        let other = service
            .register_file_effect(&effect_input("b", Some("project")), "run-b", "effect-b")
            .unwrap();
        // The writer remains usable while A drains, including for B in the same project.
        service
            .storage
            .coordinate_runtime_control(|| Ok(()))
            .unwrap();
        effect.mark_durably_settled();
        drop(effect);
        receive
            .recv_timeout(Duration::from_secs(2))
            .unwrap()
            .unwrap();
        worker.join().unwrap();
        assert!(service
            .storage
            .load_conversation_meta("a")
            .unwrap()
            .is_none());
        assert!(service
            .storage
            .load_conversation_meta("b")
            .unwrap()
            .is_some());
        assert!(service
            .register_file_effect(&input, "stale-a", "stale-effect")
            .is_err());
        drop(other);
    }

    #[test]
    fn unsettled_effect_prevents_deletion_after_its_worker_exits() {
        let directory = tempfile::tempdir().unwrap();
        let service = service(&directory.path().join("unsettled-effects.sqlite"));
        seed(&service, "a");
        let input = effect_input("a", None);
        let mut effect = service
            .register_file_effect(&input, "run-a", "effect-a")
            .unwrap();
        effect.mark_effects_started();
        drop(effect);
        let error = service.delete_conversation("a").unwrap_err();
        assert!(
            error.contains("without a confirmed durable terminal receipt"),
            "{error}"
        );
        assert!(service
            .storage
            .load_conversation_meta("a")
            .unwrap()
            .is_some());
        assert!(!service.is_conversation_deleting(Some("a")));
        // Reconciliation of the exact effect can settle the marker; deletion then succeeds.
        let mut recovered = service
            .register_file_effect(&input, "run-a", "effect-a")
            .unwrap();
        recovered.mark_durably_settled();
        drop(recovered);
        service.delete_conversation("a").unwrap();
    }

    #[test]
    fn project_delete_drains_moved_conversation_effects_frozen_in_old_project() {
        let directory = tempfile::tempdir().unwrap();
        let service = service(&directory.path().join("moved-effects.sqlite"));
        for project in ["old-project", "new-project"] {
            service
                .storage
                .save_project(
                    mycopilot_core::storage::models::ProjectRecord::without_folders(
                        project, project, 1,
                    ),
                )
                .unwrap();
        }
        seed(&service, "moved");
        let mut metadata = service
            .storage
            .load_conversation_meta("moved")
            .unwrap()
            .unwrap();
        metadata.project_id = Some("old-project".into());
        service
            .storage
            .save_conversation_meta(metadata.clone())
            .unwrap();
        let frozen = effect_input("moved", Some("old-project"));
        let mut effect = service
            .register_file_effect(&frozen, "run-moved", "effect-moved")
            .unwrap();
        effect.mark_effects_started();
        metadata.project_id = Some("new-project".into());
        metadata.updated_at += 1;
        service.storage.save_conversation_meta(metadata).unwrap();
        let deleting = service.clone();
        let (send, receive) = std::sync::mpsc::sync_channel(1);
        let worker =
            std::thread::spawn(move || send.send(deleting.delete_project("new-project")).unwrap());
        let started = Instant::now();
        while !service.is_conversation_deleting(Some("moved"))
            && started.elapsed() < Duration::from_secs(2)
        {
            std::thread::sleep(Duration::from_millis(1));
        }
        assert!(service.is_conversation_deleting(Some("moved")));
        assert!(
            receive.try_recv().is_err(),
            "the old-project effect must be drained"
        );
        assert!(service
            .register_file_effect(&frozen, "late-moved", "late-effect")
            .is_err());
        assert!(service
            .storage
            .load_conversation_meta("moved")
            .unwrap()
            .is_some());
        effect.mark_durably_settled();
        drop(effect);
        receive
            .recv_timeout(Duration::from_secs(2))
            .unwrap()
            .unwrap();
        worker.join().unwrap();
        assert!(service
            .storage
            .load_conversation_meta("moved")
            .unwrap()
            .is_none());
        assert!(service
            .storage
            .load_projects()
            .unwrap()
            .iter()
            .any(|project| project.id == "old-project"));
        assert!(service
            .register_file_effect(&frozen, "stale-moved", "stale-effect")
            .is_err());
    }
}
