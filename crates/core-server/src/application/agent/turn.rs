use super::*;
use sha2::{Digest, Sha256};

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
        input: AgentConversationTurnInput,
        notifications: CoreServerNotificationSender,
    ) -> Result<AgentConversationTurnOutput, AgentServiceError> {
        self.start_human_root_turn_internal(input, None, None, notifications)
    }

    pub fn rewrite_conversation_turn(
        &self,
        mut input: AgentConversationTurnRewriteInput,
        notifications: CoreServerNotificationSender,
    ) -> Result<AgentConversationTurnOutput, AgentServiceError> {
        let request_id = strict_rewrite_identity(&input.request_id, "requestId")?;
        let source_user_message_id =
            strict_rewrite_identity(&input.source_user_message_id, "sourceUserMessageId")?;
        let source_assistant_message_id = strict_rewrite_identity(
            &input.source_assistant_message_id,
            "sourceAssistantMessageId",
        )?;
        let conversation_id = input
            .turn
            .conversation_id
            .as_deref()
            .ok_or_else(|| "编辑重发必须指定 conversationId。".to_string())?;
        let conversation_id = strict_rewrite_identity(conversation_id, "conversationId")?;
        let user_message_id = input
            .turn
            .user_message_id
            .as_deref()
            .ok_or_else(|| "编辑重发必须指定新的 userMessageId。".to_string())?;
        let user_message_id = strict_rewrite_identity(user_message_id, "userMessageId")?;
        let assistant_message_id = input
            .turn
            .assistant_message_id
            .as_deref()
            .ok_or_else(|| "编辑重发必须指定新的 assistantMessageId。".to_string())?;
        let assistant_message_id =
            strict_rewrite_identity(assistant_message_id, "assistantMessageId")?;
        if source_user_message_id == source_assistant_message_id
            || user_message_id == assistant_message_id
            || source_user_message_id == user_message_id
            || source_assistant_message_id == assistant_message_id
        {
            return Err("编辑重发的新旧消息 ID 必须互不冲突。".to_string().into());
        }
        self.authorize_user_conversation_write(&conversation_id)?;

        let request_bytes =
            serde_json::to_vec(&input).map_err(|error| format!("无法验证编辑重发请求：{error}"))?;
        let request_fingerprint = format!("sha256:{:x}", Sha256::digest(&request_bytes));
        if let Some(existing) = self.storage.get_conversation_turn_rewrite(&request_id)? {
            if existing.request_fingerprint != request_fingerprint
                || existing.conversation_id != conversation_id
                || existing.source_user_message_id != source_user_message_id
                || existing.source_assistant_message_id != source_assistant_message_id
                || existing.replacement_user_message_id != user_message_id
                || existing.replacement_assistant_message_id != assistant_message_id
            {
                return Err(
                    "edit_turn_request_conflict: requestId 已用于其他编辑重发请求。"
                        .to_string()
                        .into(),
                );
            }
            return active_rewrite_turn_output(&self.storage, &existing)
                .map_err(AgentServiceError::from);
        }

        // Source attachment IDs remain bound to the immutable old user message. The replacement
        // receives deterministic IDs so an RPC replay produces the same rows and file paths
        // without moving or overwriting source facts.
        for (index, attachment) in input.turn.attachments.iter_mut().enumerate() {
            let mut digest = Sha256::new();
            digest.update(b"conversation-turn-rewrite-attachment-v1\0");
            digest.update(request_id.as_bytes());
            digest.update((index as u64).to_be_bytes());
            digest.update(attachment.id.as_bytes());
            digest.update(attachment.name.as_bytes());
            digest.update(attachment.data.as_bytes());
            attachment.id = format!("attachment-rewrite-{:x}", digest.finalize());
        }

        self.start_human_root_turn_internal(
            input.turn,
            Some(HumanConversationTurnRewrite {
                request_id,
                request_fingerprint,
                source_user_message_id,
                source_assistant_message_id,
            }),
            None,
            notifications,
        )
    }

    pub(super) fn start_human_root_turn_internal(
        &self,
        input: AgentConversationTurnInput,
        rewrite: Option<HumanConversationTurnRewrite>,
        automation: Option<AutomationHumanRootAdmission>,
        notifications: CoreServerNotificationSender,
    ) -> Result<AgentConversationTurnOutput, AgentServiceError> {
        self.start_human_root_turn_with_response(input, rewrite, automation, None, notifications)
    }

    pub(super) fn start_human_root_turn_with_response(
        &self,
        mut input: AgentConversationTurnInput,
        rewrite: Option<HumanConversationTurnRewrite>,
        mut automation: Option<AutomationHumanRootAdmission>,
        response: Option<mycopilot_core::storage::human_interaction_repository::HumanInteractionAsyncTurnAdmission>,
        notifications: CoreServerNotificationSender,
    ) -> Result<AgentConversationTurnOutput, AgentServiceError> {
        let automation_execution_context = automation
            .as_ref()
            .map(|automation| {
                mycopilot_core::AgentAutomationExecutionContext::new(
                    automation.context.automation_id.clone(),
                    automation.context.automation_run_id.clone(),
                    automation.context.scheduled_for,
                    automation.context.last_run_at,
                    automation.context.trigger_kind.clone(),
                )
            })
            .transpose()
            .map_err(AgentServiceError::from)?;
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

        self.authorize_user_conversation_write(&conversation_id)?;

        // Conversation Turn admission and Provider-transition admission share one Host lock. The
        // process reservation is installed before prepare writes any message; the empty durable
        // in-progress trace committed by the executor is the cross-restart source of truth.
        let _admission = self
            .conversation_admission
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if let Some(rewrite) = &rewrite {
            if let Some(existing) = self
                .storage
                .get_conversation_turn_rewrite(&rewrite.request_id)?
            {
                if existing.request_fingerprint != rewrite.request_fingerprint
                    || existing.conversation_id != conversation_id
                    || existing.source_user_message_id != rewrite.source_user_message_id
                    || existing.source_assistant_message_id != rewrite.source_assistant_message_id
                    || existing.replacement_user_message_id != user_message_id
                    || existing.replacement_assistant_message_id != assistant_message_id
                {
                    return Err(
                        "edit_turn_request_conflict: requestId 已用于其他编辑重发请求。"
                            .to_string()
                            .into(),
                    );
                }
                return active_rewrite_turn_output(&self.storage, &existing)
                    .map_err(AgentServiceError::from);
            }
        }
        // Every genuinely new root Turn needs a current Host lease, including answers to old
        // asynchronous questions and edit/regenerate. Replay above, child/wake execution,
        // steering, and recovery of an existing Turn do not create new human work.
        // Hold this lock until durable admission; revocation cannot race the message/Trace commit.
        let execution_access_guard = self
            .execution_access
            .lock()
            .map_err(|_| ExecutionAccessDenied::unavailable().agent_error())?;
        execution_access_guard
            .check()
            .map_err(ExecutionAccessDenied::agent_error)?;
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
        self.ensure_no_manual_context_compaction(&conversation_id)?;
        let (previous_conversation, expected_revision) =
            self.storage.load_conversation_for_turn(&conversation_id)?;
        if rewrite.is_some()
            && previous_conversation
                .as_ref()
                .and_then(|conversation| conversation.model_id.as_deref())
                != Some(input.model_id.as_str())
        {
            return Err(
                "edit_turn_model_change_not_supported: 编辑重发必须继续使用当前会话模型；请先完成编辑，再通过新的普通 Turn 切换模型。"
                    .to_string()
                    .into(),
            );
        }
        self.ensure_provider_transition_ready_for_send(&conversation_id, &input.model_id)?;
        if previous_conversation.is_some()
            && self.storage.conversation_revision(&conversation_id)? != expected_revision
        {
            return Err("Conversation 在模型兼容预检期间发生变化，请重试本轮。"
                .to_string()
                .into());
        }
        if previous_conversation.as_ref().is_some_and(|conversation| {
            conversation.messages.iter().any(|message| {
                (message.id == user_message_id && response.is_none())
                    || message.id == assistant_message_id
            })
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
        let global_permit = match automation
            .as_mut()
            .and_then(|automation| automation.global_permit.take())
        {
            Some(permit) => permit,
            None => self
                .turn_concurrency_gate()
                .try_acquire()
                .map_err(AgentServiceError::from)?,
        };
        self.reserve_conversation_turn(&conversation_id, &run_id, &assistant_message_id)?;
        self.register_turn_concurrency_permit(&run_id, global_permit)?;
        let cancellation_token = AgentCancellationToken::new();
        self.register_cancellation(&run_id, cancellation_token.clone());

        let automation_admission = automation.as_ref().map(|automation| {
            mycopilot_core::storage::automation_repository::AutomationRunAdmissionInput {
                automation_run_id: automation.context.automation_run_id.clone(),
                admission_token: automation.admission_token.clone(),
                config_revision: automation.config_revision,
                permission_mode: automation.permission_mode.clone(),
                agent_run_id: run_id.clone(),
                conversation_id: conversation_id.clone(),
                user_message_id: user_message_id.clone(),
                assistant_message_id: assistant_message_id.clone(),
                admitted_at: now_ms(),
            }
        });
        let execution_access_check = || {
            execution_access_guard
                .check()
                .map_err(ExecutionAccessDenied::agent_error)
        };
        let prepared_outcome = if let Some(response) = response.clone() {
            prepare_reserved_human_response_turn(
                &self.storage,
                &self.skills,
                input,
                &run_id,
                previous_conversation.clone(),
                expected_revision,
                response,
                &execution_access_check,
            )
            .map(|prepared| PreparedConversationTurnOutcome::Prepared(Box::new(prepared)))
        } else {
            match rewrite.clone() {
                Some(rewrite) => prepare_reserved_human_rewrite_turn(
                    &self.storage,
                    &self.skills,
                    input,
                    &run_id,
                    previous_conversation.clone(),
                    expected_revision,
                    rewrite,
                    &execution_access_check,
                ),
                None => prepare_reserved_human_turn(
                    &self.storage,
                    &self.skills,
                    input,
                    &run_id,
                    previous_conversation.clone(),
                    expected_revision,
                    automation_admission.as_ref().map(|admission| {
                        (
                            admission,
                            &execution_access_check as &dyn Fn() -> Result<(), AgentServiceError>,
                        )
                    }),
                    automation_execution_context,
                    &execution_access_check,
                )
                .map(|prepared| PreparedConversationTurnOutcome::Prepared(Box::new(prepared))),
            }
        };
        drop(execution_access_guard);
        let prepared = match prepared_outcome {
            Ok(PreparedConversationTurnOutcome::Prepared(prepared)) => *prepared,
            Ok(PreparedConversationTurnOutcome::Replayed(output)) => {
                self.release_turn_concurrency_permit(&run_id);
                self.release_conversation_turn_if_current(&conversation_id, &run_id);
                self.unregister_cancellation_if_current(&run_id, &cancellation_token);
                return Ok(*output);
            }
            Err(error) => {
                if response.is_some() {
                    let settlement = self
                        .storage
                        .settle_async_human_interaction_start_failure(&run_id);
                    self.release_turn_concurrency_permit(&run_id);
                    self.release_conversation_turn_if_current(&conversation_id, &run_id);
                    self.unregister_cancellation_if_current(&run_id, &cancellation_token);
                    return Err(match settlement {
                        Ok(_) => error,
                        Err(settlement_error) => format!("{error}; human response preparation settlement failed: {settlement_error}").into(),
                    });
                }
                if let Some(rewrite) = &rewrite {
                    let cause = error.to_string();
                    return match self.settle_prepared_rewrite_failure(
                        &conversation_id,
                        &assistant_message_id,
                        Some(&run_id),
                        &rewrite.request_id,
                        &cause,
                    ) {
                        Ok(output) => {
                            self.release_turn_concurrency_permit(&run_id);
                            self.release_conversation_turn_if_current(&conversation_id, &run_id);
                            self.unregister_cancellation_if_current(&run_id, &cancellation_token);
                            output.ok_or(error)
                        }
                        Err(settlement_error) => Err(format!(
                            "{cause}；同时无法终态化已接受的编辑重发 Turn：{settlement_error}"
                        )
                        .into()),
                    };
                }
                if let Some(automation) = &automation {
                    if error.data().and_then(|data| data["code"].as_str())
                        == Some("permission_disabled")
                    {
                        // Atomic storage admission committed only the task/run block; it wrote no
                        // provisional Conversation facts. Preserve the structured code so the
                        // scheduler classifies this as a repairable permission block, and avoid a
                        // generic rollback that could obscure that durable outcome.
                        self.release_turn_concurrency_permit(&run_id);
                        self.release_conversation_turn_if_current(&conversation_id, &run_id);
                        self.unregister_cancellation_if_current(&run_id, &cancellation_token);
                        return Err(error);
                    }
                    let admitted = match self
                        .storage
                        .get_automation_run(&automation.context.automation_run_id)
                    {
                        Ok(candidate) => candidate.is_some_and(|candidate| {
                            candidate.agent_run_id.as_deref() == Some(run_id.as_str())
                        }),
                        Err(inspect_error) => {
                            // Once the admission transaction may have committed, an inspection
                            // failure must never fall through to ordinary HumanRoot rollback: that
                            // could erase the exactly-once message receipt. Durable recovery can
                            // safely decide the run/trace relationship on the next pass.
                            self.release_turn_concurrency_permit(&run_id);
                            self.release_conversation_turn_if_current(&conversation_id, &run_id);
                            self.unregister_cancellation_if_current(&run_id, &cancellation_token);
                            return Err(format!(
                                "{error}; could not inspect Automation admission after preparation failure: {inspect_error}"
                            )
                            .into());
                        }
                    };
                    if admitted {
                        let cause = error.to_string();
                        let settlement = self.settle_automation_start_failure(
                            &automation.context.automation_run_id,
                            &cause,
                        );
                        self.release_turn_concurrency_permit(&run_id);
                        self.release_conversation_turn_if_current(&conversation_id, &run_id);
                        self.unregister_cancellation_if_current(&run_id, &cancellation_token);
                        return Err(match settlement {
                            Ok(()) => cause.into(),
                            Err(settlement_error) => format!(
                                "{cause}; failed to settle admitted automation Turn: {settlement_error}"
                            )
                            .into(),
                        });
                    }
                }
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

        let rollback = if response.is_some() {
            PreparedTurnRollback::HumanResponse
        } else if let Some(rewrite) = &rewrite {
            PreparedTurnRollback::Rewrite {
                request_id: rewrite.request_id.clone(),
            }
        } else if let Some(automation) = automation {
            PreparedTurnRollback::Automation {
                automation_run_id: automation.context.automation_run_id,
            }
        } else {
            PreparedTurnRollback::Human {
                user_message_id,
                previous: previous_conversation,
                previous_world_state_was_empty,
            }
        };
        self.launch_prepared_initial_turn(prepared, cancellation_token, notifications, rollback)
    }
}

fn strict_rewrite_identity(value: &str, field: &str) -> Result<String, AgentServiceError> {
    let trimmed = value.trim();
    if trimmed.is_empty() || trimmed != value || value.len() > 256 {
        return Err(format!("编辑重发的 {field} 无效。").into());
    }
    Ok(value.to_string())
}
