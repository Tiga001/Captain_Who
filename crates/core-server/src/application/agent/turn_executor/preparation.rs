impl AgentService {
    pub(crate) fn execute_turn(
        &self,
        start: AgentTurnStart,
        notifications: CoreServerNotificationSender,
    ) -> Result<AgentConversationTurnOutput, AgentServiceError> {
        match start {
            AgentTurnStart::HumanRoot(start) => {
                self.start_human_root_turn(start.into_input(), notifications)
            }
            AgentTurnStart::AgentWake(start) => {
                self.start_trusted_agent_wake_turn(start, notifications)
            }
        }
    }

    fn start_trusted_agent_wake_turn(
        &self,
        mut start: TrustedAgentWakeTurnStart,
        notifications: CoreServerNotificationSender,
    ) -> Result<AgentConversationTurnOutput, AgentServiceError> {
        let _admission = self
            .conversation_admission
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let factory = ChildAgentFactory::new(Arc::clone(&self.storage));
        let spawn = factory
            .resolve_trusted_claimed_wake(
                start.agent_id(),
                start.wake_id(),
                start.source_message_id(),
                start.claim_token(),
            )
            .map_err(|error| error.to_string())?;
        if spawn.collaboration_identity != *start.collaboration_identity()
            || spawn.agent.conversation_id != start.conversation_id()
        {
            return Err("可信 Wake 身份与持久化 Agent/Mailbox 事实不一致。"
                .to_string()
                .into());
        }
        let conversation_id = spawn.agent.conversation_id.clone();
        let global_permit = start
            .take_global_permit()
            .ok_or_else(|| "可信 Wake 缺少进程级 Agent Turn 并发许可。".to_string())?;
        if self.is_project_deleting(spawn.agent.project_id.as_deref())
            || self.is_conversation_deleting(Some(&conversation_id))
        {
            return Err("项目或会话正在移除，无法开始子 Agent 运行。"
                .to_string()
                .into());
        }
        if self
            .provider_transitions
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .contains_key(&conversation_id)
        {
            return Err("子 Agent 会话正在切换模型，无法执行 Wake。"
                .to_string()
                .into());
        }

        let (previous_conversation, expected_revision) =
            self.storage.load_conversation_for_turn(&conversation_id)?;
        let previous_conversation =
            previous_conversation.ok_or_else(|| "子 Agent Conversation 不存在。".to_string())?;
        let expected_revision = expected_revision
            .ok_or_else(|| "子 Agent Conversation 缺少 Turn admission revision。".to_string())?;
        let run_id = next_run_id();
        let assistant_message_id = create_id("message");
        self.reserve_conversation_turn(&conversation_id, &run_id, &assistant_message_id)?;
        self.register_turn_concurrency_permit(&run_id, global_permit)?;
        let cancellation_token = AgentCancellationToken::new();
        self.register_cancellation(&run_id, cancellation_token.clone());
        let prepared = match prepare_agent_wake_turn(
            &self.storage,
            &self.skills,
            &spawn,
            assistant_message_id.clone(),
            &run_id,
            previous_conversation.clone(),
            expected_revision,
            mycopilot_core::TrustedAgentWakeTurnAdmission {
                agent_id: start.agent_id().to_string(),
                wake_id: start.wake_id().to_string(),
                claim_token: start.claim_token().to_string(),
                source_agent_message_id: start.source_message_id().to_string(),
            },
        ) {
            Ok(prepared) => prepared,
            Err(error) => {
                self.release_turn_concurrency_permit(&run_id);
                self.release_conversation_turn_if_current(&conversation_id, &run_id);
                self.unregister_cancellation_if_current(&run_id, &cancellation_token);
                // If atomic admission already committed, deleting the assistant/trace would also
                // delete its turn-start receipt and make the old task appear undelivered to a
                // later follow-up. Dispatcher re-reads the Wake: a still-Claimed Wake is a
                // definitely-not-admitted failure, while Running carries the immutable exact Turn
                // identity and is terminalized together with its result Outbox.
                return Err(error.to_string().into());
            }
        };

        self.launch_prepared_initial_turn(
            prepared,
            cancellation_token,
            notifications,
            PreparedTurnRollback::AgentWake,
        )
    }

    pub(super) fn rollback_prepared_initial_turn(
        &self,
        conversation_id: &str,
        assistant_message_id: &str,
        provisional_run_id: Option<&str>,
        rollback: &PreparedTurnRollback,
        cause: impl std::fmt::Display,
    ) -> AgentServiceError {
        let cause = cause.to_string();
        let result = match rollback {
            PreparedTurnRollback::Human {
                user_message_id,
                previous,
                previous_world_state_was_empty,
            } => self.storage.rollback_conversation_turn_preparation(
                conversation_id,
                user_message_id,
                assistant_message_id,
                provisional_run_id,
                previous.as_ref(),
                *previous_world_state_was_empty,
            ),
            PreparedTurnRollback::AgentWake => Ok(()),
            PreparedTurnRollback::Automation { automation_run_id } => {
                self.settle_automation_start_failure(automation_run_id, &cause)
            }
            PreparedTurnRollback::Rewrite { request_id } => self
                .settle_prepared_rewrite_failure(
                    conversation_id,
                    assistant_message_id,
                    provisional_run_id,
                    request_id,
                    &cause,
                )
                .map(|_| ()),
        };
        match result {
            Ok(()) => cause.into(),
            Err(rollback_error) => {
                format!("{cause}；同时无法回滚 provisional Conversation Turn：{rollback_error}")
                    .into()
            }
        }
    }

    /// Terminalizes a Turn whose automation admission transaction already committed.
    ///
    /// Deleting its provisional messages would erase the exactly-once receipt and allow restart
    /// recovery to create a duplicate user message. The automation watcher subsequently observes
    /// this durable failed Trace and atomically settles the owning `automation_runs` row.
    pub(crate) fn settle_automation_start_failure(
        &self,
        automation_run_id: &str,
        cause: &str,
    ) -> Result<(), String> {
        let durable_run = self
            .storage
            .get_automation_run(automation_run_id)?
            .ok_or_else(|| "admitted automation run disappeared".to_string())?;
        let agent_run_id = durable_run.agent_run_id.as_deref().ok_or_else(|| {
            "admitted automation run is missing its Agent run identity".to_string()
        })?;
        let conversation_id = durable_run.conversation_id.as_deref().ok_or_else(|| {
            "admitted automation run is missing its Conversation identity".to_string()
        })?;
        let user_message_id = durable_run.user_message_id.as_deref().ok_or_else(|| {
            "admitted automation run is missing its user message identity".to_string()
        })?;
        let assistant_message_id =
            durable_run.assistant_message_id.as_deref().ok_or_else(|| {
                "admitted automation run is missing its assistant message identity".to_string()
            })?;
        let reverse_bound = self
            .storage
            .get_automation_run_by_agent_run_id(agent_run_id)?
            .ok_or_else(|| "admitted Agent run has no Automation owner".to_string())?;
        if reverse_bound.id != durable_run.id
            || reverse_bound.config_revision != durable_run.config_revision
            || reverse_bound.conversation_id != durable_run.conversation_id
            || reverse_bound.user_message_id != durable_run.user_message_id
            || reverse_bound.assistant_message_id != durable_run.assistant_message_id
        {
            return Err(
                "automation start-failure reverse identity does not match its durable admission"
                    .to_string(),
            );
        }

        let conversation = self
            .storage
            .load_conversation(conversation_id)?
            .ok_or_else(|| "admitted automation Conversation disappeared".to_string())?;
        let exact_user = conversation
            .messages
            .iter()
            .filter(|message| message.id == user_message_id)
            .collect::<Vec<_>>();
        let exact_assistant = conversation
            .messages
            .iter()
            .filter(|message| message.id == assistant_message_id)
            .collect::<Vec<_>>();
        if exact_user.len() != 1
            || exact_user[0].role != "user"
            || exact_assistant.len() != 1
            || exact_assistant[0].role != "assistant"
        {
            return Err(
                "automation start-failure identity does not match its durable message pair"
                    .to_string(),
            );
        }
        let current_trace = self
            .storage
            .get_conversation_turn_trace(assistant_message_id)?
            .ok_or_else(|| "automation trace disappeared before failure settlement".to_string())?;
        if current_trace.conversation_id != conversation_id
            || current_trace.run_id != agent_run_id
            || !current_trace.items.is_empty()
        {
            return Err(
                "automation start-failure identity does not match its exact empty trace"
                    .to_string(),
            );
        }
        match current_trace.terminal_status {
            mycopilot_core::ConversationTurnTraceTerminalStatus::Failed => {
                if durable_run.status
                    != mycopilot_core::storage::automation_repository::StoredAutomationRunStatus::Failed
                    && !matches!(
                        durable_run.status,
                        mycopilot_core::storage::automation_repository::StoredAutomationRunStatus::Running
                            | mycopilot_core::storage::automation_repository::StoredAutomationRunStatus::WaitingForApproval
                    )
                {
                    return Err(
                        "failed Automation trace is bound to an incompatible run state"
                            .to_string(),
                    );
                }
                // The first caller may have committed and lost its return value. Treat the exact
                // failed identity as success so scheduler recovery never tries to create or alter
                // a second Turn.
                self.notify_durable_turn_observers(assistant_message_id);
                return Ok(());
            }
            mycopilot_core::ConversationTurnTraceTerminalStatus::InProgress => {
                if !matches!(
                    durable_run.status,
                    mycopilot_core::storage::automation_repository::StoredAutomationRunStatus::Running
                        | mycopilot_core::storage::automation_repository::StoredAutomationRunStatus::WaitingForApproval
                ) {
                    return Err(
                        "in-progress Automation trace is bound to an incompatible run state"
                            .to_string(),
                    );
                }
            }
            _ => {
                return Err(
                    "automation start-failure settlement cannot overwrite a terminal Turn"
                        .to_string(),
                );
            }
        }
        let created_at = self
            .storage
            .get_assistant_message_created_at(conversation_id, assistant_message_id)?
            .ok_or_else(|| {
                "automation assistant disappeared before failure settlement".to_string()
            })?;
        let completed_at = now_ms().max(created_at);
        let trace = failed_conversation_trace_without_items(
            agent_run_id,
            conversation_id,
            assistant_message_id,
            cause,
        );
        let mut settled = false;
        for delay_ms in TERMINAL_PERSISTENCE_RETRY_DELAYS_MS {
            match self.persist_automation_start_failure(
                automation_run_id,
                conversation_id,
                assistant_message_id,
                cause,
                &trace,
                created_at,
                completed_at,
            ) {
                Ok(()) => {
                    settled = true;
                    break;
                }
                Err(_) => std::thread::sleep(Duration::from_millis(delay_ms)),
            }
        }
        if !settled {
            self.persist_automation_start_failure(
                automation_run_id,
                conversation_id,
                assistant_message_id,
                cause,
                &trace,
                created_at,
                completed_at,
            )?;
        }
        self.notify_durable_turn_observers(assistant_message_id);
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn persist_automation_start_failure(
        &self,
        _automation_run_id: &str,
        conversation_id: &str,
        assistant_message_id: &str,
        cause: &str,
        trace: &ConversationTurnTrace,
        created_at: i64,
        completed_at: i64,
    ) -> Result<(), String> {
        #[cfg(test)]
        if crate::application::agent::take_automation_start_failure_settlement_failure(
            _automation_run_id,
        ) {
            return Err("injected Automation start-failure trace settlement failure".to_string());
        }
        self.storage.finalize_chat_message_with_conversation_trace(
            conversation_id,
            assistant_message_id,
            cause,
            Some("error"),
            "failed",
            trace,
            created_at,
            completed_at,
        )
    }

    /// Once the immutable rewrite receipt exists, a pre-runtime fault is an accepted Turn whose
    /// only safe resolution is an exact failed terminal. Returning the stored response keeps the
    /// first RPC response and every request-id replay identical; callers then reload the active
    /// projection and observe the failed replacement instead of resurrecting the hidden source.
    pub(super) fn settle_prepared_rewrite_failure(
        &self,
        conversation_id: &str,
        assistant_message_id: &str,
        provisional_run_id: Option<&str>,
        request_id: &str,
        cause: &str,
    ) -> Result<Option<AgentConversationTurnOutput>, String> {
        let Some(rewrite) = self.storage.get_conversation_turn_rewrite(request_id)? else {
            return Ok(None);
        };
        if rewrite.conversation_id != conversation_id
            || rewrite.replacement_assistant_message_id != assistant_message_id
            || provisional_run_id != Some(rewrite.run_id.as_str())
        {
            return Err(
                "rewrite failure settlement identity does not match its receipt".to_string(),
            );
        }
        let current_trace = self
            .storage
            .get_conversation_turn_trace(assistant_message_id)?
            .ok_or_else(|| "rewrite trace disappeared before failure settlement".to_string())?;
        if current_trace.conversation_id != conversation_id
            || current_trace.run_id != rewrite.run_id
            || current_trace.terminal_status
                != mycopilot_core::ConversationTurnTraceTerminalStatus::InProgress
            || !current_trace.items.is_empty()
        {
            return Err(
                "rewrite failure settlement requires the exact empty in-progress trace".to_string(),
            );
        }
        let created_at = self
            .storage
            .get_assistant_message_created_at(conversation_id, assistant_message_id)?
            .ok_or_else(|| "rewrite assistant disappeared before failure settlement".to_string())?;
        let completed_at = now_ms().max(created_at);
        let trace = failed_conversation_trace_without_items(
            &rewrite.run_id,
            conversation_id,
            assistant_message_id,
            cause,
        );
        let mut settled = false;
        for delay_ms in TERMINAL_PERSISTENCE_RETRY_DELAYS_MS {
            match self.finalize_turn_with_human_root_notification(
                &rewrite.run_id,
                conversation_id,
                assistant_message_id,
                AgentRunStatus::Failed,
                cause,
                Some("error"),
                "failed",
                &trace,
                None,
                created_at,
                completed_at,
                None,
                None,
            ) {
                Ok(()) => {
                    settled = true;
                    break;
                }
                Err(_) => std::thread::sleep(Duration::from_millis(delay_ms)),
            }
        }
        if !settled {
            self.finalize_turn_with_human_root_notification(
                &rewrite.run_id,
                conversation_id,
                assistant_message_id,
                AgentRunStatus::Failed,
                cause,
                Some("error"),
                "failed",
                &trace,
                None,
                created_at,
                completed_at,
                None,
                None,
            )?;
        }
        Ok(Some(active_rewrite_turn_output(&self.storage, &rewrite)?))
    }
}
