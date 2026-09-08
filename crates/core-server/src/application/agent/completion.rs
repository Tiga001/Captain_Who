use super::*;

#[cfg(test)]
type AfterTerminalPublicationHook = Arc<dyn Fn() + Send + Sync>;

#[cfg(test)]
static AFTER_TERMINAL_PUBLICATION_HOOKS: Mutex<Vec<(String, AfterTerminalPublicationHook)>> =
    Mutex::new(Vec::new());

#[cfg(test)]
pub(super) fn install_after_terminal_publication_hook(
    run_id: &str,
    hook: AfterTerminalPublicationHook,
) {
    AFTER_TERMINAL_PUBLICATION_HOOKS
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .push((run_id.to_string(), hook));
}

#[cfg(test)]
pub(super) fn run_after_terminal_publication_hook(run_id: &str) {
    let hook = {
        let mut hooks = AFTER_TERMINAL_PUBLICATION_HOOKS
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

#[derive(Default)]
pub(super) struct AgentTerminalEventGate {
    deferred: Mutex<Vec<AgentEvent>>,
}

impl AgentTerminalEventGate {
    pub(super) fn route(&self, event: AgentEvent) -> Option<AgentEvent> {
        if should_defer_until_terminal_commit(&event) {
            self.deferred
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .push(event);
            None
        } else {
            Some(event)
        }
    }

    pub(super) fn take_after_persistence(&self, output: &AgentChatOutput) -> Vec<AgentEvent> {
        let mut events = std::mem::take(
            &mut *self
                .deferred
                .lock()
                .unwrap_or_else(|error| error.into_inner()),
        );
        for event in &mut events {
            if let AgentEvent::Done { usage, .. } = event {
                *usage = output.usage.clone();
            }
        }
        if !events
            .iter()
            .any(|event| matches!(event, AgentEvent::Done { .. }))
        {
            events.push(terminal_done_event(output));
        }
        events
    }

    /// Releases the Runtime-owned terminal error only after its durable trace committed.
    ///
    /// Host failures do not pass through the Runtime emitter and therefore return `None`; their
    /// caller must emit an explicitly unanchored fallback instead of inventing a trace sequence.
    pub(super) fn take_error_after_persistence(&self) -> Option<AgentEvent> {
        std::mem::take(
            &mut *self
                .deferred
                .lock()
                .unwrap_or_else(|error| error.into_inner()),
        )
        .into_iter()
        .find(|event| {
            matches!(
                event,
                AgentEvent::Error {
                    recoverable: false,
                    ..
                }
            )
        })
    }

    pub(super) fn discard(&self) {
        self.deferred
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clear();
    }
}

pub(super) fn should_defer_until_terminal_commit(event: &AgentEvent) -> bool {
    match event {
        AgentEvent::State { state, .. } => is_terminal_run_status(state.status),
        AgentEvent::Done { status, .. } => !matches!(
            status,
            Some(AgentRunStatus::WaitingForApproval | AgentRunStatus::WaitingForUserInput)
        ),
        AgentEvent::Error { recoverable, .. } => !recoverable,
        _ => false,
    }
}

pub(super) fn pending_terminal_commit_is_publishable(
    assistant_persisted: bool,
    pending_transitioned: bool,
    terminal: bool,
) -> bool {
    assistant_persisted && pending_transitioned && terminal
}

pub(super) fn terminal_done_event(output: &AgentChatOutput) -> AgentEvent {
    AgentEvent::Done {
        run_id: output.run_id.clone(),
        success: output.status == AgentRunStatus::Completed,
        status: Some(output.status),
        content: (!output.content.is_empty()).then(|| output.content.clone()),
        usage: output.usage.clone(),
        finish_reason: output.finish_reason.clone(),
        proposed_actions: output.proposed_actions.clone(),
    }
}

pub(super) fn model_request_interruption_public_message(
    reason: AgentModelRequestInterruptionReason,
) -> &'static str {
    match reason {
        AgentModelRequestInterruptionReason::ServiceConnectionFailed => "模型服务连接失败",
        AgentModelRequestInterruptionReason::ServiceUnavailable => "模型服务暂时不可用",
        AgentModelRequestInterruptionReason::AuthenticationFailed => "模型服务鉴权失败",
        AgentModelRequestInterruptionReason::QuotaExhausted => "模型服务额度不足",
        AgentModelRequestInterruptionReason::ContextLimitExceeded => "上下文超过模型限制",
        AgentModelRequestInterruptionReason::RequestRejected => "模型请求被拒绝",
        AgentModelRequestInterruptionReason::ResponseInvalid => "模型响应无效",
        AgentModelRequestInterruptionReason::RequestFailed => "模型请求失败",
    }
}

pub(super) fn model_request_interruption_event(
    run_id: &str,
    reason: AgentModelRequestInterruptionReason,
) -> AgentEvent {
    AgentEvent::Error {
        run_id: Some(run_id.to_string()),
        trace_sequence: None,
        message: model_request_interruption_public_message(reason).to_string(),
        recoverable: false,
        code: Some("agent.model_request_interrupted".to_string()),
        details: Some(serde_json::json!({
            "type": "safe_model_request_interruption",
            "reason": reason.as_str(),
            "safeToContinue": true,
        })),
    }
}

pub(super) fn emit_terminal_events_after_persistence_for_turn(
    notifications: &CoreServerNotificationSender,
    gate: &AgentTerminalEventGate,
    output: &AgentChatOutput,
    collaboration_identity: Option<&AgentCollaborationIdentity>,
    assistant_message_id: &str,
) {
    for event in gate.take_after_persistence(output) {
        emit_agent_event_notifications(
            notifications,
            collaboration_identity,
            &output.run_id,
            assistant_message_id,
            event,
        );
    }
}

pub(super) fn emit_pending_transition_error(
    notifications: &CoreServerNotificationSender,
    run_id: &str,
    status: PendingActionStatus,
    error: &str,
) {
    let _ = notifications.send(agent_event_notification(AgentEvent::Error {
        run_id: Some(run_id.to_string()),
        trace_sequence: None,
        message: format!(
            "待审批操作无法可靠迁移为 `{}`；已抑制终态事件：{error}",
            pending_status_label(status)
        ),
        recoverable: true,
        code: Some("pending_action_transition_failed".to_string()),
        details: None,
    }));
}

pub(super) fn pending_action_persistence_error(error: String) -> AgentError {
    AgentError::structured(
        "pending_action_persistence_failed",
        format!("待审批操作无法持久化，运行已安全终止：{error}"),
        serde_json::json!({ "cause": error }),
    )
}
