use super::*;

#[derive(Clone)]
pub(crate) struct AutoApprovedActionContext {
    pub(super) agent_input: AgentChatInput,
    pub(super) run_id: String,
    pub(super) conversation_id: Option<String>,
    pub(super) assistant_message_id: Option<String>,
    pub(super) skill_resources: Option<Arc<SkillResourceSession>>,
    pub(super) notifications: Option<CoreServerNotificationSender>,
}

impl AutoApprovedActionContext {
    pub(crate) fn new(
        agent_input: AgentChatInput,
        run_id: String,
        conversation_id: Option<String>,
        assistant_message_id: Option<String>,
        skill_resources: Option<Arc<SkillResourceSession>>,
    ) -> Self {
        Self {
            agent_input,
            run_id,
            conversation_id,
            assistant_message_id,
            skill_resources,
            notifications: None,
        }
    }

    pub(crate) fn with_notifications(
        mut self,
        notifications: CoreServerNotificationSender,
    ) -> Self {
        self.notifications = Some(notifications);
        self
    }
}

/// Bridges bounded process preview chunks to Renderer events without coupling command success to
/// the UI channel. The authoritative result remains the final paired ToolResult.
pub(crate) fn command_output_observer(
    run_id: &str,
    call_id: &str,
    notifications: &CoreServerNotificationSender,
) -> ProcessOutputObserver {
    let run_id = run_id.to_string();
    let call_id = call_id.to_string();
    let notifications = notifications.clone();
    let sequence = Arc::new(Mutex::new(0_u64));
    Arc::new(move |stream, output| {
        if output.is_empty() {
            return;
        }
        // Keep sequence assignment and enqueue atomic across stdout/stderr reader threads so the
        // receiver sees the same total order encoded in the event.
        let mut next_sequence = sequence.lock().unwrap_or_else(|error| error.into_inner());
        *next_sequence = next_sequence.saturating_add(1);
        let _ = notifications.send(agent_event_notification(AgentEvent::CommandOutput {
            run_id: run_id.clone(),
            call_id: call_id.clone(),
            sequence: *next_sequence,
            stream,
            output,
        }));
    })
}
