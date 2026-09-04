const TERMINAL_PERSISTENCE_RETRY_DELAYS_MS: [u64; 3] = [10, 50, 200];

#[cfg(test)]
type BeforePendingActionStoreHook = Arc<dyn Fn() + Send + Sync>;

#[cfg(test)]
type BeforeApprovalPublicationArbitrationHook = Arc<dyn Fn() + Send + Sync>;

#[cfg(test)]
type BeforeWaitingPublicationArbitrationHook = Arc<dyn Fn() + Send + Sync>;

#[cfg(test)]
static BEFORE_PENDING_ACTION_STORE_HOOKS: Mutex<Vec<(String, BeforePendingActionStoreHook)>> =
    Mutex::new(Vec::new());

#[cfg(test)]
static BEFORE_APPROVAL_PUBLICATION_ARBITRATION_HOOKS: Mutex<
    Vec<(String, BeforeApprovalPublicationArbitrationHook)>,
> = Mutex::new(Vec::new());

#[cfg(test)]
static BEFORE_WAITING_PUBLICATION_ARBITRATION_HOOKS: Mutex<
    Vec<(String, BeforeWaitingPublicationArbitrationHook)>,
> = Mutex::new(Vec::new());

#[cfg(test)]
pub(super) fn install_before_pending_action_store_hook(
    action_id: &str,
    hook: BeforePendingActionStoreHook,
) {
    BEFORE_PENDING_ACTION_STORE_HOOKS
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .push((action_id.to_string(), hook));
}

#[cfg(test)]
pub(super) fn install_before_approval_publication_arbitration_hook(
    action_id: &str,
    hook: BeforeApprovalPublicationArbitrationHook,
) {
    BEFORE_APPROVAL_PUBLICATION_ARBITRATION_HOOKS
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .push((action_id.to_string(), hook));
}

#[cfg(test)]
pub(super) fn install_before_waiting_publication_arbitration_hook(
    run_id: &str,
    hook: BeforeWaitingPublicationArbitrationHook,
) {
    BEFORE_WAITING_PUBLICATION_ARBITRATION_HOOKS
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .push((run_id.to_string(), hook));
}

#[cfg(test)]
fn run_before_pending_action_store_hook(action_id: &str) {
    let hook = {
        let mut hooks = BEFORE_PENDING_ACTION_STORE_HOOKS
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        hooks
            .iter()
            .position(|(candidate, _)| candidate == action_id)
            .map(|index| hooks.swap_remove(index).1)
    };
    if let Some(hook) = hook {
        hook();
    }
}

#[cfg(test)]
fn run_before_approval_publication_arbitration_hook(action_id: &str) {
    let hook = {
        let mut hooks = BEFORE_APPROVAL_PUBLICATION_ARBITRATION_HOOKS
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        hooks
            .iter()
            .position(|(candidate, _)| candidate == action_id)
            .map(|index| hooks.swap_remove(index).1)
    };
    if let Some(hook) = hook {
        hook();
    }
}

#[cfg(test)]
fn run_before_waiting_publication_arbitration_hook(run_id: &str) {
    let hook = {
        let mut hooks = BEFORE_WAITING_PUBLICATION_ARBITRATION_HOOKS
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        hooks
            .iter()
            .position(|(candidate, _)| candidate == run_id)
            .map(|index| hooks.swap_remove(index).1)
    };
    if let Some(hook) = hook {
        hook();
    }
}

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
    pub(super) automation_report_sink: Option<Arc<dyn AutomationReportSink>>,
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
    /// Root-local collaboration event sequence captured when the committed final response stream
    /// started. Activity committed after this boundary remains visible in Agent Center but does
    /// not become part of the frozen parent response Timeline.
    pub(super) final_response_collaboration_cutoff: Option<u64>,
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
    Automation {
        automation_run_id: String,
    },
    AgentWake,
}
