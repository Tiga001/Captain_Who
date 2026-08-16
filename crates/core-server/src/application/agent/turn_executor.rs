use super::*;

const TERMINAL_PERSISTENCE_RETRY_DELAYS_MS: [u64; 3] = [10, 50, 200];

/// Retries one immutable terminal settlement without weakening the durable in-progress fence.
///
/// `persist` may stage logical-run Usage in memory before entering SQLite. A failed attempt must
/// therefore be rolled back before another attempt, otherwise additive Provider semantics would
/// count the same terminal segment twice. The final failed attempt intentionally remains staged:
/// the still-live Turn occupancy, permit, trace/context snapshots, and Usage state then describe
/// the same unresolved durable terminal boundary until restart reconciliation or diagnosis.
pub(super) async fn persist_terminal_with_bounded_retry<T, Persist, Rollback>(
    mut persist: Persist,
    mut rollback_before_retry: Rollback,
) -> Result<T, String>
where
    T: Send,
    Persist: FnMut() -> Result<T, String> + Send,
    Rollback: FnMut() + Send,
{
    for delay_ms in TERMINAL_PERSISTENCE_RETRY_DELAYS_MS {
        match persist() {
            Ok(value) => return Ok(value),
            Err(_) => {
                rollback_before_retry();
                tokio::time::sleep(Duration::from_millis(delay_ms)).await;
            }
        }
    }
    persist()
}

pub(super) fn restore_run_usage_state(
    service: &AgentService,
    run_id: &str,
    previous: &Option<AgentRunUsageState>,
) {
    let mut usage_contexts = service
        .usage_contexts
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    match previous {
        Some(previous) => {
            usage_contexts.insert(run_id.to_string(), previous.clone());
        }
        None => {
            usage_contexts.remove(run_id);
        }
    }
}

/// Public root turns and Host-authenticated Agent wakes enter the same application executor.
///
/// Neither this enum nor the wake request implements `Deserialize`: collaboration identity is a
/// Host fact reconstructed from the Agent graph, never a renderer/model supplied parameter.
// Round 2 exposes this Host-only branch to deterministic application tests. The first production
// caller is the Round 3 Dispatcher; keeping it crate-private is more important than fabricating a
// renderer route merely to satisfy dead-code analysis.
#[cfg_attr(not(test), allow(dead_code))]
#[allow(clippy::large_enum_variant)]
pub(crate) enum AgentTurnStart {
    HumanRoot(HumanRootTurnStart),
    AgentWake(TrustedAgentWakeTurnStart),
}

pub(crate) struct HumanRootTurnStart {
    input: AgentConversationTurnInput,
}

impl HumanRootTurnStart {
    pub(crate) fn new(input: AgentConversationTurnInput) -> Self {
        Self { input }
    }

    fn into_input(self) -> AgentConversationTurnInput {
        self.input
    }
}

/// A wake identity which can only be constructed by trusted application code after claiming and
/// re-reading the durable wake, Agent node, and Mailbox projection.
#[derive(Clone, Debug)]
pub(crate) struct TrustedAgentWakeTurnStart {
    wake_id: String,
    agent_id: String,
    conversation_id: String,
    source_message_id: String,
    claim_token: String,
    collaboration_identity: AgentCollaborationIdentity,
    global_permit: Option<crate::application::agent_dispatcher::AgentTurnConcurrencyPermit>,
}

impl TrustedAgentWakeTurnStart {
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn new(
        wake_id: String,
        agent_id: String,
        conversation_id: String,
        source_message_id: String,
        claim_token: String,
        collaboration_identity: AgentCollaborationIdentity,
    ) -> Result<Self, AgentServiceError> {
        for (field, value) in [
            ("wake_id", wake_id.as_str()),
            ("agent_id", agent_id.as_str()),
            ("conversation_id", conversation_id.as_str()),
            ("source_message_id", source_message_id.as_str()),
            ("claim_token", claim_token.as_str()),
        ] {
            if value.trim().is_empty() {
                return Err(format!("trusted Agent wake {field} cannot be empty").into());
            }
        }
        if collaboration_identity.agent_id != agent_id
            || collaboration_identity.conversation_id != conversation_id
            || collaboration_identity.source_agent_message_id != source_message_id
        {
            return Err(
                "trusted Agent wake identity does not match its collaboration snapshot"
                    .to_string()
                    .into(),
            );
        }
        collaboration_identity
            .validate()
            .map_err(|error| AgentServiceError::from(error.to_string()))?;
        Ok(Self {
            wake_id,
            agent_id,
            conversation_id,
            source_message_id,
            claim_token,
            collaboration_identity,
            global_permit: None,
        })
    }

    pub(crate) fn with_global_permit(
        mut self,
        permit: crate::application::agent_dispatcher::AgentTurnConcurrencyPermit,
    ) -> Self {
        self.global_permit = Some(permit);
        self
    }

    fn take_global_permit(
        &mut self,
    ) -> Option<crate::application::agent_dispatcher::AgentTurnConcurrencyPermit> {
        self.global_permit.take()
    }

    pub(crate) fn wake_id(&self) -> &str {
        &self.wake_id
    }

    pub(crate) fn agent_id(&self) -> &str {
        &self.agent_id
    }

    pub(crate) fn conversation_id(&self) -> &str {
        &self.conversation_id
    }

    pub(crate) fn source_message_id(&self) -> &str {
        &self.source_message_id
    }

    pub(crate) fn claim_token(&self) -> &str {
        &self.claim_token
    }

    pub(crate) fn collaboration_identity(&self) -> &AgentCollaborationIdentity {
        &self.collaboration_identity
    }
}

pub(super) struct PreparedRuntimeTurnSegment {
    pub(super) run_id: String,
    pub(super) conversation_id: String,
    pub(super) assistant_message_id: String,
    pub(super) assistant_created_at: i64,
    pub(super) agent_input: AgentChatInput,
    pub(super) skill_resources: Option<Arc<mycopilot_core::skills::SkillResourceSession>>,
    pub(super) mcp_tools: Option<McpToolRuntime>,
    pub(super) context_window_tool_projection: RunContextToolProjection,
    pub(super) cancellation_token: AgentCancellationToken,
    pub(super) steer_input: AgentSteerInputQueue,
    /// An approval continuation keeps its predecessor nonterminal until either the continuation
    /// finishes or a durable successor approval takes over recovery. Successor storage uses this
    /// identity to atomically insert the recovery anchor and terminalize the predecessor before
    /// either row becomes visible to process-local readers.
    pub(super) pending_action_predecessor_settlement:
        Option<(PendingActionRecord, PendingActionStatus)>,
    /// The initial root segment owns freshly captured MCP approval payloads. A continuation has
    /// already crossed its prior approval boundary and keeps the existing invalidation semantics.
    pub(super) invalidate_mcp_payload_on_pending_store_failure: bool,
    pub(super) steering_close_error_context: &'static str,
}

pub(super) struct RuntimeTurnSegmentOutcome {
    pub(super) result: AgentResult<AgentChatOutput>,
    pub(super) terminal_event_gate: Arc<AgentTerminalEventGate>,
}

#[allow(clippy::large_enum_variant)]
pub(super) enum PreparedTurnRollback {
    Human {
        user_message_id: String,
        previous: Option<mycopilot_core::storage::models::ChatConversationRecord>,
        previous_world_state_was_empty: bool,
    },
    Rewrite {
        request_id: String,
    },
    AgentWake,
}

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
            match self.storage.finalize_chat_message_with_conversation_trace(
                conversation_id,
                assistant_message_id,
                cause,
                Some("error"),
                "failed",
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
            self.storage.finalize_chat_message_with_conversation_trace(
                conversation_id,
                assistant_message_id,
                cause,
                Some("error"),
                "failed",
                &trace,
                created_at,
                completed_at,
            )?;
        }
        Ok(Some(active_rewrite_turn_output(&self.storage, &rewrite)?))
    }

    /// Commits the durable Turn lease and launches one initial Runtime segment. Human-root and
    /// trusted child-Wake adapters both end here; only their exact preparation rollback differs.
    pub(super) fn launch_prepared_initial_turn(
        &self,
        prepared: PreparedConversationTurn,
        cancellation_token: AgentCancellationToken,
        notifications: CoreServerNotificationSender,
        rollback: PreparedTurnRollback,
    ) -> Result<AgentConversationTurnOutput, AgentServiceError> {
        let run_id = prepared.output.run_id.clone();
        let conversation_id = prepared.output.conversation_id.clone();
        let assistant_message_id = prepared.output.assistant_message_id.clone();
        let mcp_tools = self.capture_mcp_tool_runtime(&prepared.agent_input);
        self.invalidate_conversation_context_state(&conversation_id);
        let context_window_tool_projection = match self.context_window_tool_projection_with_mcp(
            &prepared.agent_input,
            prepared.skill_resources.as_ref().map(Arc::clone),
            mcp_tools.clone(),
        ) {
            Ok(projection) => RunContextToolProjection::new(projection),
            Err(error) => {
                if let PreparedTurnRollback::Rewrite { request_id } = &rollback {
                    let cause = error.to_string();
                    return match self.settle_prepared_rewrite_failure(
                        &conversation_id,
                        &assistant_message_id,
                        Some(&run_id),
                        request_id,
                        &cause,
                    ) {
                        Ok(output) => {
                            self.release_conversation_turn_if_current(&conversation_id, &run_id);
                            self.release_turn_concurrency_permit(&run_id);
                            self.unregister_cancellation_if_current(&run_id, &cancellation_token);
                            output.ok_or_else(|| AgentServiceError::from(cause))
                        }
                        Err(settlement_error) => Err(format!(
                            "{cause}；同时无法终态化已接受的编辑重发 Turn：{settlement_error}"
                        )
                        .into()),
                    };
                }
                self.release_conversation_turn_if_current(&conversation_id, &run_id);
                self.release_turn_concurrency_permit(&run_id);
                self.unregister_cancellation_if_current(&run_id, &cancellation_token);
                return Err(match rollback {
                    PreparedTurnRollback::AgentWake => error.into(),
                    rollback => self.rollback_prepared_initial_turn(
                        &conversation_id,
                        &assistant_message_id,
                        Some(&run_id),
                        &rollback,
                        error,
                    ),
                });
            }
        };
        self.register_usage_context(&run_id, prepared.usage_context.clone());
        if self.is_agent_input_scope_deleting(&prepared.agent_input) {
            const CAUSE: &str = "项目或会话正在移除，无法开始新的 agent 运行。";
            if let PreparedTurnRollback::Rewrite { request_id } = &rollback {
                return match self.settle_prepared_rewrite_failure(
                    &conversation_id,
                    &assistant_message_id,
                    Some(&run_id),
                    request_id,
                    CAUSE,
                ) {
                    Ok(output) => {
                        self.release_conversation_turn_if_current(&conversation_id, &run_id);
                        self.release_turn_concurrency_permit(&run_id);
                        self.discard_usage_context(&run_id);
                        self.unregister_cancellation_if_current(&run_id, &cancellation_token);
                        output.ok_or_else(|| AgentServiceError::from(CAUSE.to_string()))
                    }
                    Err(settlement_error) => Err(format!(
                        "{CAUSE}；同时无法终态化已接受的编辑重发 Turn：{settlement_error}"
                    )
                    .into()),
                };
            }
            self.release_conversation_turn_if_current(&conversation_id, &run_id);
            self.release_turn_concurrency_permit(&run_id);
            self.discard_usage_context(&run_id);
            self.unregister_cancellation_if_current(&run_id, &cancellation_token);
            return Err(match rollback {
                PreparedTurnRollback::AgentWake => CAUSE.to_string().into(),
                rollback => self.rollback_prepared_initial_turn(
                    &conversation_id,
                    &assistant_message_id,
                    Some(&run_id),
                    &rollback,
                    CAUSE,
                ),
            });
        }

        let steer_input = self.register_active_run_control(
            &run_id,
            &conversation_id,
            &assistant_message_id,
            prepared
                .agent_input
                .context
                .as_ref()
                .and_then(|context| context.project_id.as_deref()),
            prepared.agent_input.model_capabilities,
        );
        initialize_turn_diff_best_effort(
            &self.storage,
            &prepared.agent_input,
            &run_id,
            &conversation_id,
            &assistant_message_id,
        );

        let output = prepared.output.clone();
        let service = self.clone();
        let worker_run_id = run_id;
        let worker_conversation_id = conversation_id;
        let worker_assistant_message_id = assistant_message_id;
        let pending_agent_input = prepared.agent_input.clone();
        let segment = PreparedRuntimeTurnSegment {
            run_id: worker_run_id.clone(),
            conversation_id: worker_conversation_id.clone(),
            assistant_message_id: worker_assistant_message_id.clone(),
            assistant_created_at: output.assistant_message.created_at,
            agent_input: prepared.agent_input,
            skill_resources: prepared.skill_resources,
            mcp_tools,
            context_window_tool_projection,
            cancellation_token: cancellation_token.clone(),
            steer_input,
            pending_action_predecessor_settlement: None,
            invalidate_mcp_payload_on_pending_store_failure: true,
            steering_close_error_context: "无法关闭用户引导通道并持久化剩余引导",
        };

        tokio::spawn(async move {
            let RuntimeTurnSegmentOutcome {
                result,
                terminal_event_gate,
            } = service
                .run_prepared_turn_segment(segment, notifications.clone())
                .await;
            let keep_trace_snapshot = matches!(
                &result,
                Ok(output) if output.status == AgentRunStatus::WaitingForApproval
            );

            let input_is_deleting = service
                .deletion_lifecycle
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .contains_input(&pending_agent_input);
            if input_is_deleting {
                terminal_event_gate.discard();
                service
                    .release_conversation_turn_if_current(&worker_conversation_id, &worker_run_id);
                service.release_turn_concurrency_permit(&worker_run_id);
                service.discard_usage_context(&worker_run_id);
                service.discard_trace_snapshot(&worker_run_id);
                service.discard_exact_running_context_window_snapshot(&worker_run_id);
                service.unregister_cancellation_if_current(&worker_run_id, &cancellation_token);
                return;
            }

            let mut deletion_cleanup = false;
            let (durable_terminal, persistence_committed) = match result {
                Ok(mut agent_output) => {
                    let committed_durable_context = is_terminal_run_status(agent_output.status);
                    let previous_usage_state = service
                        .usage_contexts
                        .lock()
                        .unwrap_or_else(|error| error.into_inner())
                        .get(&worker_run_id)
                        .cloned();
                    let persisted = persist_terminal_with_bounded_retry(
                        || {
                            let deletion_lifecycle = service
                                .deletion_lifecycle
                                .lock()
                                .unwrap_or_else(|error| error.into_inner());
                            if deletion_lifecycle.contains_input(&pending_agent_input) {
                                return Err(
                                    "项目或会话正在移除，无法持久化 agent 终态。".to_string()
                                );
                            }
                            service.persist_final_assistant_output(
                                &worker_conversation_id,
                                &worker_assistant_message_id,
                                &mut agent_output,
                            )
                        },
                        || restore_run_usage_state(&service, &worker_run_id, &previous_usage_state),
                    )
                    .await;
                    let persistence_committed = persisted.is_ok();
                    let deletion_lifecycle = service
                        .deletion_lifecycle
                        .lock()
                        .unwrap_or_else(|error| error.into_inner());
                    let durable_terminal = if deletion_lifecycle
                        .contains_input(&pending_agent_input)
                    {
                        deletion_cleanup = true;
                        terminal_event_gate.discard();
                        service.discard_usage_context(&worker_run_id);
                        false
                    } else {
                        if persistence_committed {
                            service.notify_durable_turn_observers(&worker_assistant_message_id);
                        }
                        let durable_terminal = persistence_committed && committed_durable_context;
                        if durable_terminal {
                            service.emit_terminal_context_window_snapshot(
                                &notifications,
                                &pending_agent_input,
                                &worker_run_id,
                                &worker_conversation_id,
                                &worker_assistant_message_id,
                                if agent_output.status == AgentRunStatus::Cancelled {
                                    ""
                                } else {
                                    &agent_output.content
                                },
                            );
                        } else if let Err(error) = &persisted {
                            let _ =
                                notifications.send(agent_event_notification(AgentEvent::Error {
                                    run_id: Some(worker_run_id.clone()),
                                    trace_sequence: None,
                                    message: format!(
                                        "无法原子持久化 assistant 终态与会话轨迹：{error}"
                                    ),
                                    recoverable: true,
                                    code: Some("conversation_trace_persistence_failed".to_string()),
                                    details: None,
                                }));
                        }
                        if durable_terminal {
                            emit_terminal_events_after_persistence_for_turn(
                                &notifications,
                                &terminal_event_gate,
                                &agent_output,
                                pending_agent_input
                                    .context
                                    .as_ref()
                                    .and_then(|context| context.collaboration_identity.as_ref()),
                                &worker_assistant_message_id,
                            );
                        }
                        durable_terminal
                    };
                    (durable_terminal, persistence_committed)
                }
                Err(error) => {
                    let model_request_interruption = error.model_request_interruption();
                    let usage = error.usage().cloned();
                    let code = error.code().map(ToString::to_string);
                    let details = error.details().cloned();
                    let message = error.to_string();
                    let conversation_turn_trace =
                        error.conversation_turn_trace().cloned().unwrap_or_else(|| {
                            failed_conversation_trace_without_items(
                                &worker_run_id,
                                &worker_conversation_id,
                                &worker_assistant_message_id,
                                &message,
                            )
                        });
                    let previous_usage_state = service
                        .usage_contexts
                        .lock()
                        .unwrap_or_else(|lock_error| lock_error.into_inner())
                        .get(&worker_run_id)
                        .cloned();
                    let persisted = persist_terminal_with_bounded_retry(
                        || {
                            let deletion_lifecycle = service
                                .deletion_lifecycle
                                .lock()
                                .unwrap_or_else(|lock_error| lock_error.into_inner());
                            if deletion_lifecycle.contains_input(&pending_agent_input) {
                                return Err(
                                    "项目或会话正在移除，无法持久化 agent 失败终态。".to_string()
                                );
                            }
                            if model_request_interruption.is_some() {
                                service.persist_assistant_model_request_interruption(
                                    &worker_conversation_id,
                                    &worker_assistant_message_id,
                                    &message,
                                    usage.clone(),
                                    &conversation_turn_trace,
                                )
                            } else {
                                service.persist_assistant_error(
                                    &worker_conversation_id,
                                    &worker_assistant_message_id,
                                    &message,
                                    usage.clone(),
                                    &conversation_turn_trace,
                                )
                            }
                        },
                        || restore_run_usage_state(&service, &worker_run_id, &previous_usage_state),
                    )
                    .await;
                    let persistence_committed = persisted.is_ok();
                    let cumulative_usage = persisted.as_ref().ok().cloned().flatten();
                    let deletion_lifecycle = service
                        .deletion_lifecycle
                        .lock()
                        .unwrap_or_else(|lock_error| lock_error.into_inner());
                    let durable_terminal = if deletion_lifecycle
                        .contains_input(&pending_agent_input)
                    {
                        deletion_cleanup = true;
                        terminal_event_gate.discard();
                        service.discard_usage_context(&worker_run_id);
                        false
                    } else {
                        if persistence_committed {
                            service.notify_durable_turn_observers(&worker_assistant_message_id);
                            service.emit_terminal_context_window_snapshot(
                                &notifications,
                                &pending_agent_input,
                                &worker_run_id,
                                &worker_conversation_id,
                                &worker_assistant_message_id,
                                if model_request_interruption.is_some() {
                                    ""
                                } else {
                                    &message
                                },
                            );
                        } else if let Err(error) = &persisted {
                            terminal_event_gate.discard();
                            let _ =
                                notifications.send(agent_event_notification(AgentEvent::Error {
                                    run_id: Some(worker_run_id.clone()),
                                    trace_sequence: None,
                                    message: format!(
                                        "无法原子持久化 assistant 失败终态与会话轨迹：{error}"
                                    ),
                                    recoverable: true,
                                    code: Some("conversation_trace_persistence_failed".to_string()),
                                    details: None,
                                }));
                        }
                        if persistence_committed {
                            let collaboration_identity = pending_agent_input
                                .context
                                .as_ref()
                                .and_then(|context| context.collaboration_identity.as_ref());
                            let terminal_error_event =
                                if let Some(reason) = model_request_interruption {
                                    terminal_event_gate.discard();
                                    model_request_interruption_event(&worker_run_id, reason)
                                } else {
                                    terminal_event_gate
                                        .take_error_after_persistence()
                                        .unwrap_or_else(|| AgentEvent::Error {
                                            run_id: Some(worker_run_id.clone()),
                                            trace_sequence: None,
                                            message: message.clone(),
                                            recoverable: false,
                                            code,
                                            details,
                                        })
                                };
                            emit_agent_event_notifications(
                                &notifications,
                                collaboration_identity,
                                &worker_run_id,
                                &worker_assistant_message_id,
                                terminal_error_event,
                            );
                            emit_agent_event_notifications(
                                &notifications,
                                collaboration_identity,
                                &worker_run_id,
                                &worker_assistant_message_id,
                                AgentEvent::Done {
                                    run_id: worker_run_id.clone(),
                                    success: false,
                                    status: Some(AgentRunStatus::Failed),
                                    content: model_request_interruption
                                        .is_none()
                                        .then_some(message),
                                    usage: cumulative_usage,
                                    finish_reason: None,
                                    proposed_actions: Vec::new(),
                                },
                            );
                        }
                        persistence_committed
                    };
                    (durable_terminal, persistence_committed)
                }
            };

            if durable_terminal || deletion_cleanup {
                service
                    .release_conversation_turn_if_current(&worker_conversation_id, &worker_run_id);
                service.release_turn_concurrency_permit(&worker_run_id);
            }
            if deletion_cleanup || (durable_terminal && !keep_trace_snapshot) {
                service.discard_trace_snapshot(&worker_run_id);
                service.discard_exact_running_context_window_snapshot(&worker_run_id);
            }
            if durable_terminal
                || deletion_cleanup
                || (keep_trace_snapshot && persistence_committed)
            {
                service.unregister_cancellation_if_current(&worker_run_id, &cancellation_token);
            }
        });

        Ok(output)
    }

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
        let mut active_turns = self
            .active_conversation_turns
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if let Some(active) = active_turns.get(conversation_id) {
            return Err(format!(
                "当前会话已有进行中的 agent 运行（runId={}），请等待其完成。",
                active.run_id
            )
            .into());
        }
        if let Some(active) = self
            .storage
            .list_conversation_turn_traces(conversation_id)?
            .into_iter()
            .find(|trace| trace.terminal_status == ConversationTurnTraceTerminalStatus::InProgress)
        {
            return Err(format!(
                "当前会话已有持久化的进行中 agent 运行（runId={}），请先完成或恢复它。",
                active.run_id
            )
            .into());
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
            permits.insert(run_id, self.turn_concurrency_gate.adopt_recovered());
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

    /// Runs exactly one Runtime segment. Initial root/wake execution and approval continuation
    /// share this body; their distinct durable completion policies remain outside it.
    pub(super) async fn run_prepared_turn_segment(
        &self,
        segment: PreparedRuntimeTurnSegment,
        notifications: CoreServerNotificationSender,
    ) -> RuntimeTurnSegmentOutcome {
        let PreparedRuntimeTurnSegment {
            run_id,
            conversation_id,
            assistant_message_id,
            assistant_created_at,
            agent_input,
            skill_resources,
            mcp_tools,
            context_window_tool_projection,
            cancellation_token,
            steer_input,
            pending_action_predecessor_settlement,
            invalidate_mcp_payload_on_pending_store_failure,
            steering_close_error_context,
        } = segment;

        let emitter_notifications = notifications.clone();
        let emitter_service = self.clone();
        let emitter_conversation_id = conversation_id.clone();
        let emitter_assistant_message_id = assistant_message_id.clone();
        let emitter_agent_input = agent_input.clone();
        let emitter_collaboration_identity = agent_input
            .context
            .as_ref()
            .and_then(|context| context.collaboration_identity.clone());
        let emitter_run_id = run_id.clone();
        let terminal_event_gate = Arc::new(AgentTerminalEventGate::default());
        let emitter_terminal_event_gate = terminal_event_gate.clone();
        let pending_store_failure = Arc::new(Mutex::new(None::<String>));
        let emitter_pending_store_failure = Arc::clone(&pending_store_failure);
        let emitter: AgentEventEmitter = Arc::new(move |event| {
            if let AgentEvent::ApprovalRequired {
                run_id,
                action,
                checkpoint,
            } = &event
            {
                if let Err(error) = emitter_service.close_active_run_steering(
                    run_id,
                    AgentSteerRunRejectionCode::RunNotSteerable,
                    "The agent run is waiting for approval and no longer accepts guidance.",
                    &emitter_notifications,
                ) {
                    if invalidate_mcp_payload_on_pending_store_failure {
                        emitter_service.invalidate_mcp_pending_payload(action);
                    }
                    emitter_terminal_event_gate.discard();
                    *emitter_pending_store_failure
                        .lock()
                        .unwrap_or_else(|lock_error| lock_error.into_inner()) = Some(error);
                    return;
                }
                let mut checkpoint_input =
                    agent_input_with_run_checkpoint(&emitter_agent_input, checkpoint);
                if let Err(error) =
                    emitter_service.refresh_agent_input_attachment_library(&mut checkpoint_input)
                {
                    if invalidate_mcp_payload_on_pending_store_failure {
                        emitter_service.invalidate_mcp_pending_payload(action);
                    }
                    emitter_terminal_event_gate.discard();
                    *emitter_pending_store_failure
                        .lock()
                        .unwrap_or_else(|lock_error| lock_error.into_inner()) = Some(error);
                    return;
                }
                let pending_store = if let Some((predecessor, terminal_status)) =
                    pending_action_predecessor_settlement.as_ref()
                {
                    emitter_service.store_pending_action_with_predecessor_settlement(
                        run_id,
                        &emitter_conversation_id,
                        &emitter_assistant_message_id,
                        action.as_ref().clone(),
                        checkpoint_input,
                        predecessor,
                        *terminal_status,
                    )
                } else {
                    emitter_service.store_pending_action(
                        run_id,
                        &emitter_conversation_id,
                        &emitter_assistant_message_id,
                        action.as_ref().clone(),
                        checkpoint_input,
                    )
                };
                let should_publish = match pending_store {
                    Ok(should_publish) => should_publish,
                    Err(error) => {
                        if invalidate_mcp_payload_on_pending_store_failure {
                            emitter_service.invalidate_mcp_pending_payload(action);
                        }
                        emitter_terminal_event_gate.discard();
                        *emitter_pending_store_failure
                            .lock()
                            .unwrap_or_else(|lock_error| lock_error.into_inner()) = Some(error);
                        return;
                    }
                };
                if !should_publish {
                    return;
                }
            }
            if emitter_pending_store_failure
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .is_some()
            {
                return;
            }
            let event = emitter_service.project_cumulative_usage_onto_event(event);
            if let Some(event) = emitter_terminal_event_gate.route(event) {
                emit_agent_event_notifications(
                    &emitter_notifications,
                    emitter_collaboration_identity.as_ref(),
                    &emitter_run_id,
                    &emitter_assistant_message_id,
                    event,
                );
            }
        });

        let host_executor = self.host_action_executor(
            agent_input.clone(),
            run_id.clone(),
            Some(conversation_id.clone()),
            Some(assistant_message_id.clone()),
            skill_resources.clone(),
            notifications.clone(),
        );
        let trace_observer = self.trace_observer(
            &run_id,
            &conversation_id,
            &assistant_message_id,
            assistant_created_at,
            agent_input.clone(),
            context_window_tool_projection.clone(),
            notifications.clone(),
        );
        let context_compaction_services = self.context_compaction_services(
            &run_id,
            &conversation_id,
            &assistant_message_id,
            agent_input.clone(),
            context_window_tool_projection,
            notifications.clone(),
        );
        let model_request_observer =
            self.model_request_observer(&run_id, &conversation_id, &assistant_message_id);
        let context_window_observer = agent_input.context_window_indicator_enabled.then(|| {
            self.context_window_observer(
                &run_id,
                &conversation_id,
                &agent_input.model,
                notifications.clone(),
            )
        });
        let mut host_services = AgentRuntimeHostServices::new()
            .with_host_actions(host_executor, self.storage.clone())
            .with_command_session_executor(Arc::new(self.command_sessions.clone()))
            .with_office_engine(self.office_engine.clone())
            .with_trace_observer(trace_observer)
            .with_model_request_observer(model_request_observer)
            .with_context_compaction(context_compaction_services);
        if let Some(provider_continuation_vault) = self.provider_continuation_vault.as_ref() {
            host_services = host_services
                .with_provider_continuation_vault(Arc::clone(provider_continuation_vault));
        }
        if let Some(context_window_observer) = context_window_observer {
            host_services = host_services.with_context_window_observer(context_window_observer);
        }
        if let Some(image_generation_execution) = self.image_generation_execution.clone() {
            host_services =
                host_services.with_image_generation_execution(image_generation_execution);
        }
        if let Some(skill_installation_prepare) = self.skill_installation_prepare.clone() {
            host_services =
                host_services.with_skill_installation_prepare(skill_installation_prepare);
        }
        if let Some(skill_installation) = self.skill_installation.clone() {
            host_services = host_services.with_skill_installation_commit(skill_installation);
        }
        host_services = host_services.with_skill_activation_resolver(
            model_skill_activation_resolver(self.storage.clone(), self.skills.clone()),
        );
        host_services = host_services.with_steer_input(steer_input.clone());
        host_services = host_services.with_collaboration_inbox(Arc::new(
            PersistentAgentSamplingBoundaryInbox::new(Arc::clone(&self.storage)),
        ));
        let collaboration_harness =
            crate::application::agent_harness::AgentCollaborationHarnessAdapter::new(
                Arc::clone(&self.storage),
                self.clone(),
                self.collaboration_authorizer(),
                Arc::clone(&self.collaboration_dispatcher),
                notifications.clone(),
            );
        host_services =
            match collaboration_harness.attach_to_host_services(host_services, &conversation_id) {
                Ok(services) => services,
                Err(error) => {
                    return RuntimeTurnSegmentOutcome {
                        result: Err(error),
                        terminal_event_gate,
                    };
                }
            };
        if let Some(resources) = skill_resources {
            host_services = host_services.with_skill_resources(resources);
        }
        if let Some(resolver) = self.artifact_runtime.clone() {
            host_services = host_services.with_command_runtime_profile_resolver(resolver);
        }
        if let Some(mcp_tools) = mcp_tools {
            host_services = host_services.with_mcp_tools(mcp_tools);
        }

        let result = send_chat_with_host_services(
            agent_input,
            run_id.clone(),
            emitter,
            cancellation_token,
            host_services,
        )
        .await;
        let result = match pending_store_failure
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .take()
        {
            Some(error) => {
                terminal_event_gate.discard();
                Err(pending_action_persistence_error(error))
            }
            None => result,
        };
        let close_message = match &result {
            Ok(output) if output.status == AgentRunStatus::WaitingForApproval => {
                "The agent run is waiting for approval and no longer accepts guidance."
            }
            _ => "The agent run has finished and no longer accepts guidance.",
        };
        let result = match self.unregister_active_run_control(
            &run_id,
            &steer_input,
            AgentSteerRunRejectionCode::RunNotSteerable,
            close_message,
            &notifications,
        ) {
            Ok(()) => result,
            Err(error) => {
                terminal_event_gate.discard();
                Err(AgentError::new(format!(
                    "{steering_close_error_context}：{error}"
                )))
            }
        };

        RuntimeTurnSegmentOutcome {
            result,
            terminal_event_gate,
        }
    }
}

#[cfg(test)]
mod terminal_persistence_retry_tests {
    use super::*;

    #[tokio::test]
    async fn retries_restore_staged_state_and_apply_the_terminal_segment_once() {
        let attempts = Arc::new(Mutex::new(0_usize));
        let staged_segments = Arc::new(Mutex::new(0_usize));
        let persist_attempts = Arc::clone(&attempts);
        let persist_segments = Arc::clone(&staged_segments);
        let rollback_segments = Arc::clone(&staged_segments);

        let result = persist_terminal_with_bounded_retry(
            move || {
                *persist_segments
                    .lock()
                    .unwrap_or_else(|error| error.into_inner()) += 1;
                let mut attempts = persist_attempts
                    .lock()
                    .unwrap_or_else(|error| error.into_inner());
                *attempts += 1;
                if *attempts < 3 {
                    Err("injected terminal transaction failure".to_string())
                } else {
                    Ok("committed")
                }
            },
            move || {
                *rollback_segments
                    .lock()
                    .unwrap_or_else(|error| error.into_inner()) = 0;
            },
        )
        .await
        .unwrap();

        assert_eq!(result, "committed");
        assert_eq!(
            *attempts.lock().unwrap_or_else(|error| error.into_inner()),
            3
        );
        assert_eq!(
            *staged_segments
                .lock()
                .unwrap_or_else(|error| error.into_inner()),
            1,
            "retry must not apply the same additive Usage segment more than once"
        );
    }

    #[tokio::test]
    async fn exhausted_retry_keeps_the_last_staged_terminal_state_for_reconciliation() {
        let attempts = Arc::new(Mutex::new(0_usize));
        let staged_segments = Arc::new(Mutex::new(0_usize));
        let persist_attempts = Arc::clone(&attempts);
        let persist_segments = Arc::clone(&staged_segments);
        let rollback_segments = Arc::clone(&staged_segments);

        let error = persist_terminal_with_bounded_retry(
            move || -> Result<(), String> {
                *persist_segments
                    .lock()
                    .unwrap_or_else(|lock_error| lock_error.into_inner()) += 1;
                *persist_attempts
                    .lock()
                    .unwrap_or_else(|lock_error| lock_error.into_inner()) += 1;
                Err("persistent terminal transaction failure".to_string())
            },
            move || {
                *rollback_segments
                    .lock()
                    .unwrap_or_else(|lock_error| lock_error.into_inner()) = 0;
            },
        )
        .await
        .unwrap_err();

        assert_eq!(error, "persistent terminal transaction failure");
        assert_eq!(
            *attempts
                .lock()
                .unwrap_or_else(|lock_error| lock_error.into_inner()),
            TERMINAL_PERSISTENCE_RETRY_DELAYS_MS.len() + 1
        );
        assert_eq!(
            *staged_segments
                .lock()
                .unwrap_or_else(|lock_error| lock_error.into_inner()),
            1,
            "the final failed attempt remains represented while the durable Turn fence stays live"
        );
    }
}
