use super::*;

impl AgentService {
    /// Public transport adapter. Renderer input can only create a human/root Turn; trusted Agent
    /// wakes use the crate-private `AgentTurnStart::AgentWake` variant.
    pub fn start_conversation_turn(
        &self,
        input: AgentConversationTurnInput,
        notifications: CoreServerNotificationSender,
    ) -> Result<AgentConversationTurnOutput, AgentServiceError> {
        self.execute_turn(
            AgentTurnStart::HumanRoot(HumanRootTurnStart::new(input)),
            notifications,
        )
    }

    pub(super) fn start_human_root_turn(
        &self,
        mut input: AgentConversationTurnInput,
        notifications: CoreServerNotificationSender,
    ) -> Result<AgentConversationTurnOutput, AgentServiceError> {
        // Normalize all identities before admission. In particular, a new Conversation must be
        // visible to the same one-Turn reservation used by an existing Conversation.
        let conversation_id = normalized_optional(input.conversation_id.as_deref())
            .unwrap_or_else(|| create_id("conversation"));
        let user_message_id = normalized_optional(input.user_message_id.as_deref())
            .unwrap_or_else(|| create_id("message"));
        let assistant_message_id = normalized_optional(input.assistant_message_id.as_deref())
            .unwrap_or_else(|| create_id("message"));
        input.conversation_id = Some(conversation_id.clone());
        input.user_message_id = Some(user_message_id.clone());
        input.assistant_message_id = Some(assistant_message_id.clone());

        // Conversation Turn admission and Provider-transition admission share one Host lock. The
        // process reservation is installed before prepare writes any message; the empty durable
        // in-progress trace committed by the executor is the cross-restart source of truth.
        let _admission = self
            .conversation_admission
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if self.is_project_deleting(input.project_id.as_deref())
            || self.is_conversation_deleting(Some(&conversation_id))
        {
            return Err("项目或会话正在移除，无法开始新的 agent 运行。"
                .to_string()
                .into());
        }
        if let Some(agent) = self
            .storage
            .get_agent_node_by_conversation(&conversation_id)
            .map_err(|error| error.to_string())?
        {
            if agent.parent_agent_id.is_some() {
                return Err(
                    "用户只能启动根 Agent；子 Agent Conversation 是只读观察视图。"
                        .to_string()
                        .into(),
                );
            }
            if agent.lifecycle != mycopilot_core::AgentLifecycle::Active {
                return Err("根 Agent 当前不可用，不能开始新的运行。".to_string().into());
            }
        }
        if self
            .provider_transitions
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .contains_key(&conversation_id)
        {
            return Err("当前会话正在压缩历史并切换模型，请稍后再发送。"
                .to_string()
                .into());
        }
        if user_message_id == assistant_message_id {
            return Err("userMessageId 与 assistantMessageId 必须不同。"
                .to_string()
                .into());
        }
        let (previous_conversation, expected_revision) =
            self.storage.load_conversation_for_turn(&conversation_id)?;
        self.ensure_provider_transition_ready_for_send(&conversation_id, &input.model_id)?;
        if previous_conversation.is_some()
            && self.storage.conversation_revision(&conversation_id)? != expected_revision
        {
            return Err("Conversation 在模型兼容预检期间发生变化，请重试本轮。"
                .to_string()
                .into());
        }
        if previous_conversation.as_ref().is_some_and(|conversation| {
            conversation
                .messages
                .iter()
                .any(|message| message.id == user_message_id || message.id == assistant_message_id)
        }) {
            return Err("本轮消息 ID 已存在，不能覆盖既有 Conversation 历史。"
                .to_string()
                .into());
        }
        let previous_world_state_was_empty = self
            .storage
            .list_active_conversation_world_state_records(&conversation_id)?
            .is_empty();

        let run_id = next_run_id();
        let global_permit = self
            .turn_concurrency_gate()
            .try_acquire()
            .map_err(AgentServiceError::from)?;
        self.reserve_conversation_turn(&conversation_id, &run_id, &assistant_message_id)?;
        self.register_turn_concurrency_permit(&run_id, global_permit)?;
        let cancellation_token = AgentCancellationToken::new();
        self.register_cancellation(&run_id, cancellation_token.clone());

        let prepared = match prepare_reserved_human_turn(
            &self.storage,
            &self.skills,
            input,
            &run_id,
            previous_conversation.clone(),
            expected_revision,
        ) {
            Ok(prepared) => prepared,
            Err(error) => {
                self.release_turn_concurrency_permit(&run_id);
                self.release_conversation_turn_if_current(&conversation_id, &run_id);
                self.unregister_cancellation_if_current(&run_id, &cancellation_token);
                let rollback = PreparedTurnRollback::Human {
                    user_message_id: user_message_id.clone(),
                    previous: previous_conversation.clone(),
                    previous_world_state_was_empty,
                };
                return Err(self.rollback_prepared_initial_turn(
                    &conversation_id,
                    &assistant_message_id,
                    Some(&run_id),
                    &rollback,
                    error,
                ));
            }
        };

        self.launch_prepared_initial_turn(
            prepared,
            cancellation_token,
            notifications,
            PreparedTurnRollback::Human {
                user_message_id,
                previous: previous_conversation,
                previous_world_state_was_empty,
            },
        )
    }
}
