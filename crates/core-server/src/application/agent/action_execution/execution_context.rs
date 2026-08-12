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
