impl AgentService {
    #[cfg(test)]
    pub fn cancel_run(&self, run_id: &str) -> bool {
        self.cancel_run_checked(run_id).unwrap_or(false)
    }

    /// Cancels one user-owned root Run and its exact active descendant Runs.
    ///
    /// Unlike the legacy boolean wrapper, this entry point preserves storage failures so an RPC
    /// caller cannot mistake a best-effort process interruption for a durably committed tree
    /// stop. When the durable fence fails, the local root Run is still interrupted to honour the
    /// user's immediate stop intent, but the error remains authoritative for the response.
    pub fn cancel_run_checked(&self, run_id: &str) -> Result<bool, AgentServiceError> {
        // Resolve the owner once. Runtime teardown removes several in-memory indexes, so a second
        // lookup after authorization could otherwise lose the Conversation and skip the durable
        // descendant stop while the same request is in flight.
        let conversation_id = self.conversation_id_for_run(run_id);
        if !self
            .authorize_resolved_user_run_write(conversation_id.as_deref())
            .map_err(AgentServiceError::from)?
        {
            return Ok(false);
        }
        // `agent.cancelRun` is the explicit user stop boundary. Capture the authoritative
        // conversation binding before cancelling the worker, because worker teardown removes the
        // ActiveRunControl. This is intentionally separate from `cancel_run_internal`: deletion,
        // shutdown, provider failure, and a cancelled `command_session` observation must not gain
        // this user-authorized process termination semantic by accident.
        // Commit the run-scoped durable fence before signalling the root token. Spawn/Followup
        // commit under the same SQLite write boundary, so either their Wake is swept here or they
        // observe this stop and refuse to create new work.
        let tree_stop = match conversation_id.as_deref() {
            Some(conversation_id) => {
                match self.begin_descendant_agent_tree_stop(run_id, conversation_id) {
                    Ok(tree_stop) => tree_stop,
                    Err(error) => {
                        // A failed fence cannot truthfully be reported as a successful tree stop.
                        // Still interrupt process-local root work as an immediate best effort; the
                        // durable error is returned to the caller so it can surface/retry safely.
                        self.cancel_exact_run_execution(run_id, Some(conversation_id));
                        return Err(error);
                    }
                }
            }
            None => None,
        };
        let cancellation = self.cancel_exact_run_execution(run_id, conversation_id.as_deref());
        let cancelled_agent_tree = if let Some(tree_stop) = tree_stop {
            let mut first = self.cancel_agent_tree_wake_batch(tree_stop.cancellation);
            // Repeat once after delivering runtime cancellation to accelerate convergence and to
            // refresh an exact Wake status which raced the first frozen snapshot. The durable
            // fence, rather than this repeat, owns correctness for later scheduling commits.
            let second = match self.reinforce_agent_tree_stop_for_run_checked(run_id) {
                Ok(second) => second,
                Err(error) => {
                    first.errors.push(error.to_string());
                    AgentTreeWakeCancellationProgress::default()
                }
            };
            first.errors.extend(second.errors);
            if first.lost_ownership || second.lost_ownership {
                first.errors.push(
                    "子 Agent 的持久化执行身份在停止期间连续变化，无法确认整棵任务树已经停止。"
                        .to_string(),
                );
            }
            if !first.errors.is_empty() {
                return Err(AgentServiceError::from(first.errors.join("；")));
            }
            first.affected || second.affected
        } else {
            false
        };
        Ok(cancellation.any_effect() || cancelled_agent_tree)
    }

    /// Propagates an explicit user stop from a human-driven root Turn to every currently queued
    /// or admitted descendant Turn. The durable repository owns the claim/admission boundary;
    /// this Host layer owns process-local cancellation tokens, pending approvals, and command
    /// Sessions. Agent identities and completed history remain reusable by a later explicit Turn.
    fn begin_descendant_agent_tree_stop(
        &self,
        root_run_id: &str,
        root_conversation_id: &str,
    ) -> Result<Option<mycopilot_core::AgentTreeRunStopCancellation>, AgentServiceError> {
        let root = match self
            .storage
            .get_agent_node_by_conversation(root_conversation_id)
        {
            Ok(Some(root)) if root.parent_agent_id.is_none() => root,
            Ok(_) => return Ok(None),
            Err(error) => return Err(AgentServiceError::from(error.to_string())),
        };
        self.storage
            .begin_agent_tree_run_stop_by_root_agent(&root.agent_id, root_run_id)
            .map_err(|error| AgentServiceError::from(error.to_string()))
    }

    pub(crate) fn agent_tree_run_is_stopped(&self, run_id: &str) -> Result<bool, String> {
        self.storage
            .get_agent_tree_run_stop(run_id)
            .map(|stop| stop.is_some())
            .map_err(|error| error.to_string())
    }

    /// Repeats the durable sweep after an authoritative collaboration Tool crosses its commit
    /// boundary. Only a tree already fenced by an explicit user root-stop can use this path, so a
    /// model-issued single-Agent interrupt never gains accidental cascade semantics.
    pub(crate) fn reinforce_agent_tree_stop_for_run(&self, run_id: &str) -> bool {
        match self.reinforce_agent_tree_stop_for_run_checked(run_id) {
            Ok(progress) if progress.errors.is_empty() && !progress.lost_ownership => {
                progress.affected
            }
            Ok(progress) => {
                let detail = if progress.errors.is_empty() {
                    "子 Agent 的持久化执行身份连续变化".to_string()
                } else {
                    progress.errors.join("; ")
                };
                eprintln!("failed to reinforce stopped Agent tree: {}", detail);
                false
            }
            Err(error) => {
                eprintln!("failed to reinforce stopped Agent tree: {error}");
                false
            }
        }
    }

    fn reinforce_agent_tree_stop_for_run_checked(
        &self,
        run_id: &str,
    ) -> Result<AgentTreeWakeCancellationProgress, AgentServiceError> {
        let reinforced = match self.storage.reinforce_agent_tree_run_stop(run_id) {
            Ok(Some(reinforced)) => reinforced,
            Ok(None) => return Ok(AgentTreeWakeCancellationProgress::default()),
            Err(error) => return Err(AgentServiceError::from(error.to_string())),
        };
        Ok(self.cancel_agent_tree_wake_batch(reinforced.cancellation))
    }

    fn cancel_agent_tree_wake_batch(
        &self,
        batch: mycopilot_core::AgentTreeWakeCancellationBatch,
    ) -> AgentTreeWakeCancellationProgress {
        let mut progress = AgentTreeWakeCancellationProgress {
            affected: !batch.cancelled_before_admission.is_empty(),
            lost_ownership: false,
            errors: Vec::new(),
        };
        for wake in batch.active_wakes {
            match self.cancel_agent_tree_active_wake(wake) {
                Ok(wake_progress) => {
                    progress.affected |= wake_progress.affected;
                    progress.lost_ownership |= wake_progress.lost_ownership;
                    progress.errors.extend(wake_progress.errors);
                }
                Err(error) => progress.errors.push(error.to_string()),
            }
        }

        if progress.affected || progress.lost_ownership {
            if let Some(dispatcher) = self
                .collaboration_dispatcher
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .as_ref()
            {
                dispatcher.notify_work_available();
            }
        }
        progress
    }

    fn cancel_agent_tree_active_wake(
        &self,
        mut wake: mycopilot_core::ActiveAgentTreeWake,
    ) -> Result<AgentTreeWakeCancellationProgress, AgentServiceError> {
        for attempt in 0..2 {
            let runtime_interrupt = self.interrupt_agent_wake_run(&wake.run_id);
            if matches!(&runtime_interrupt, Ok(outcome) if outcome.any_effect()) {
                return Ok(AgentTreeWakeCancellationProgress {
                    affected: true,
                    lost_ownership: false,
                    errors: Vec::new(),
                });
            }

            match self.storage.settle_tree_stopped_active_wake(&wake) {
                Ok(mycopilot_core::AgentTreeStoppedWakeSettlementOutcome::Interrupted(
                    settlement,
                )) => {
                    crate::application::agent_wait::shared_agent_wait_notifications()
                        .notify_caller(&settlement.result_message.recipient_agent_id);
                    self.release_conversation_turn_if_current(&wake.conversation_id, &wake.run_id);
                    self.release_turn_concurrency_permit(&wake.run_id);
                    return Ok(AgentTreeWakeCancellationProgress {
                        affected: true,
                        lost_ownership: false,
                        errors: Vec::new(),
                    });
                }
                Ok(mycopilot_core::AgentTreeStoppedWakeSettlementOutcome::AlreadyTerminal) => {
                    self.release_conversation_turn_if_current(&wake.conversation_id, &wake.run_id);
                    self.release_turn_concurrency_permit(&wake.run_id);
                    return Ok(AgentTreeWakeCancellationProgress {
                        affected: true,
                        lost_ownership: false,
                        errors: Vec::new(),
                    });
                }
                Ok(mycopilot_core::AgentTreeStoppedWakeSettlementOutcome::LostOwnership) => {
                    if attempt == 0 {
                        let current = self
                            .storage
                            .get_agent_wake(&wake.wake_id)
                            .map_err(|error| AgentServiceError::from(error.to_string()))?;
                        let Some(current) = current else {
                            return Ok(AgentTreeWakeCancellationProgress {
                                affected: false,
                                lost_ownership: true,
                                errors: Vec::new(),
                            });
                        };
                        if current.run_id.as_deref() != Some(wake.run_id.as_str())
                            || current.assistant_message_id.as_deref()
                                != Some(wake.assistant_message_id.as_str())
                            || current.agent_id != wake.agent_id
                        {
                            return Ok(AgentTreeWakeCancellationProgress {
                                affected: false,
                                lost_ownership: true,
                                errors: Vec::new(),
                            });
                        }
                        if current.status.is_terminal() {
                            return Ok(AgentTreeWakeCancellationProgress {
                                affected: true,
                                lost_ownership: false,
                                errors: Vec::new(),
                            });
                        }
                        let Some(claim_token) = current.claim_token else {
                            return Ok(AgentTreeWakeCancellationProgress {
                                affected: false,
                                lost_ownership: true,
                                errors: Vec::new(),
                            });
                        };
                        wake.status = current.status;
                        wake.claim_token = claim_token;
                        continue;
                    }
                    return Ok(AgentTreeWakeCancellationProgress {
                        affected: false,
                        lost_ownership: true,
                        errors: Vec::new(),
                    });
                }
                Ok(mycopilot_core::AgentTreeStoppedWakeSettlementOutcome::UnsafeActiveAction {
                    count,
                }) => {
                    let runtime_error = runtime_interrupt
                        .err()
                        .map(|error| format!("；运行时中断错误：{error}"))
                        .unwrap_or_default();
                    return Err(AgentServiceError::from(format!(
                        "子 Agent Run {} 仍有 {count} 条未确认的审批或外部执行，无法安全伪装为已中断{runtime_error}",
                        wake.run_id
                    )));
                }
                Err(error) => {
                    let runtime_error = runtime_interrupt
                        .err()
                        .map(|error| format!("；运行时中断错误：{error}"))
                        .unwrap_or_default();
                    return Err(AgentServiceError::from(format!(
                        "无法持久化结算已停止的子 Agent Run {}：{error}{runtime_error}",
                        wake.run_id
                    )));
                }
            }
        }
        unreachable!("tree-stop cancellation retry loop always returns")
    }

    /// Cancels one exact Agent Run, including an adopted command Session only when the caller has
    /// already resolved the Run's Conversation at an authorized explicit-stop boundary.
    ///
    /// Root user-stop and trusted child-Wake interruption share this primitive. Authorization,
    /// durable tree fencing, pending-approval settlement, and Wake bookkeeping remain with their
    /// respective callers.
    pub(super) fn cancel_exact_run_execution(
        &self,
        run_id: &str,
        conversation_id: Option<&str>,
    ) -> AgentRunCancellationOutcome {
        let mut outcome = self.cancel_run_internal(run_id);
        let interrupted_sessions = conversation_id.map_or(0, |conversation_id| {
            self.command_sessions
                .interrupt_origin_run(conversation_id, run_id)
        });
        outcome.record_resource_cleanup(interrupted_sessions > 0);
        outcome
    }

    pub(super) fn cancel_run_internal(&self, run_id: &str) -> AgentRunCancellationOutcome {
        self.retire_builtin_capability_run(run_id);
        let runtime_token_signalled = {
            let cancellations = self
                .cancellations
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            if let Some(token) = cancellations.get(run_id) {
                token.cancel();
                true
            } else {
                false
            }
        };
        // Internal cancellation only reaches Sessions which have not durably transferred
        // ownership. Explicit user cancellation adds its process boundary in `cancel_run` after
        // resolving the active conversation identity; all other cancellation causes keep adopted
        // Sessions alive.
        let cancelled_sessions = self.command_sessions.cancel_pre_handoff_for_run(run_id);
        let cancelled_processes = self.process_runs.cancel_run(run_id);
        let mut outcome = AgentRunCancellationOutcome::NoEffect;
        outcome.record_resource_cleanup(cancelled_sessions > 0 || cancelled_processes > 0);
        outcome.record_turn_termination(runtime_token_signalled);
        outcome
    }

    fn retire_builtin_capability_run(&self, run_id: &str) {
        if let Some(coordinator) = self.browser_risk_coordinator.as_ref() {
            // The coordinator atomically settles process-only risk waiters/tombstones and then
            // delegates all task-grant retirement to the shared runtime authority.
            coordinator.cancel_run(run_id);
        } else if let Some(runtime) = self.builtin_capabilities.as_ref() {
            let _ = runtime.revoke_run_grants(run_id);
        }
    }

    fn cancel_runs_for_destructive_mutation(
        &self,
        run_ids: &[String],
        automation_run_ids: &HashSet<String>,
    ) -> Result<(), String> {
        for run_id in run_ids {
            if automation_run_ids.contains(run_id) {
                self.cancel_automation_agent_run(run_id)?;
            } else {
                self.cancel_run_internal(run_id);
            }
        }
        Ok(())
    }
}
