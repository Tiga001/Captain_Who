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

    pub fn continue_conversation_turn(
        &self,
        input: AgentConversationTurnContinueInput,
        notifications: CoreServerNotificationSender,
    ) -> Result<AgentConversationTurnOutput, AgentServiceError> {
        let assistant_message_id = input.assistant_message_id.clone();
        self.continue_conversation_turn_inner(input, notifications)
            .map_err(|error| {
                if !matches!(
                    self.storage
                        .get_conversation_turn_trace(&assistant_message_id),
                    Ok(None)
                ) {
                    return error;
                }
                let mut data = error
                    .data()
                    .cloned()
                    .or_else(|| {
                        error
                            .skill_activation()
                            .and_then(|data| serde_json::to_value(data).ok())
                    })
                    .unwrap_or_else(|| serde_json::json!({}));
                if let Some(data) = data.as_object_mut() {
                    data.insert("continuationAdmissionRejected".into(), Value::Bool(true));
                    data.entry("code")
                        .or_insert_with(|| Value::String("TURN_CONTINUATION_REJECTED".into()));
                }
                AgentServiceError::structured(error.to_string(), data)
            })
    }

    fn continue_conversation_turn_inner(
        &self,
        input: AgentConversationTurnContinueInput,
        notifications: CoreServerNotificationSender,
    ) -> Result<AgentConversationTurnOutput, AgentServiceError> {
        for (value, field) in [
            (&input.request_id, "requestId"),
            (&input.conversation_id, "conversationId"),
            (
                &input.source_assistant_message_id,
                "sourceAssistantMessageId",
            ),
            (&input.assistant_message_id, "assistantMessageId"),
        ] {
            if value.trim().is_empty()
                || value.trim() != value
                || value.len() > 256
                || value.chars().any(char::is_control)
            {
                return Err(format!("继续任务的 {field} 无效。").into());
            }
        }
        if input.source_assistant_message_id == input.assistant_message_id {
            return Err("继续任务必须创建新的回复消息。".to_string().into());
        }
        let fingerprint =
            serde_json::to_vec(&input).map_err(|error| format!("无法验证继续任务请求：{error}"))?;
        let continuation = HumanConversationTurnContinuation {
            request_id: input.request_id,
            request_fingerprint: format!("{:x}", Sha256::digest(fingerprint)),
            source_assistant_message_id: input.source_assistant_message_id,
        };
        self.start_root_turn_with_continuation(
            AgentConversationTurnInput {
                conversation_id: Some(input.conversation_id),
                project_id: None,
                model_id: input.model_id,
                context_window_indicator_enabled: input.context_window_indicator_enabled,
                // Host-only preparation placeholder. This never becomes a user message or model input.
                content: "Continue the interrupted task.".into(),
                attachments: Vec::new(),
                folder_references: Vec::new(),
                skills: input.skills,
                title: None,
                user_message_id: None,
                assistant_message_id: Some(input.assistant_message_id),
                max_tokens: None,
                temperature: None,
                prompt_preferences: None,
                permissions: input.permissions,
            },
            None,
            None,
            None,
            None,
            Some(continuation),
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
        if self
            .storage
            .workflow_execution_delivery_origins(&conversation_id)?
            .iter()
            .any(|(message_id, _)| message_id == &source_user_message_id)
        {
            return Err("工作流来信不能编辑为用户消息。".to_string().into());
        }

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
        input: AgentConversationTurnInput,
        rewrite: Option<HumanConversationTurnRewrite>,
        automation: Option<AutomationHumanRootAdmission>,
        response: Option<mycopilot_core::storage::human_interaction_repository::HumanInteractionAsyncTurnAdmission>,
        notifications: CoreServerNotificationSender,
    ) -> Result<AgentConversationTurnOutput, AgentServiceError> {
        self.start_root_turn_with_workflow(
            input,
            rewrite,
            automation,
            response,
            None,
            notifications,
        )
    }

    pub(super) fn start_root_turn_with_workflow(
        &self,
        input: AgentConversationTurnInput,
        rewrite: Option<HumanConversationTurnRewrite>,
        automation: Option<AutomationHumanRootAdmission>,
        response: Option<mycopilot_core::storage::human_interaction_repository::HumanInteractionAsyncTurnAdmission>,
        workflow: Option<mycopilot_core::workflow_execution::Input>,
        notifications: CoreServerNotificationSender,
    ) -> Result<AgentConversationTurnOutput, AgentServiceError> {
        self.start_root_turn_with_continuation(
            input,
            rewrite,
            automation,
            response,
            workflow,
            None,
            notifications,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn start_root_turn_with_continuation(
        &self,
        mut input: AgentConversationTurnInput,
        rewrite: Option<HumanConversationTurnRewrite>,
        mut automation: Option<AutomationHumanRootAdmission>,
        response: Option<mycopilot_core::storage::human_interaction_repository::HumanInteractionAsyncTurnAdmission>,
        workflow: Option<mycopilot_core::workflow_execution::Input>,
        continuation: Option<HumanConversationTurnContinuation>,
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
        let mut user_message_id = normalized_optional(input.user_message_id.as_deref())
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
        if workflow.is_some() && self.workflow_dispatch_stopped.load(Ordering::Acquire) {
            return Err("Workflow delivery is shutting down.".to_string().into());
        }
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
        if let Some(continuation) = &continuation {
            if let Some(output) = replay_continued_turn(
                &self.storage,
                &conversation_id,
                &assistant_message_id,
                continuation,
            )? {
                return Ok(output);
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
        if let Some(continuation) = &continuation {
            let conversation = previous_conversation
                .as_ref()
                .ok_or_else(|| "继续任务的会话不存在。".to_string())?;
            let source = conversation
                .messages
                .last()
                .filter(|message| {
                    message.role == "assistant"
                        && message.id == continuation.source_assistant_message_id
                })
                .ok_or_else(|| "只能继续当前对话最新一次由你停止的任务。".to_string())?;
            let trace = self
                .storage
                .get_conversation_turn_trace(&source.id)?
                .ok_or_else(|| "停止的任务缺少执行记录，无法继续。".to_string())?;
            if trace.terminal_status
                != mycopilot_core::ConversationTurnTraceTerminalStatus::Cancelled
                || !trace.user_interrupted()
            {
                return Err("只能继续由你主动停止且已完成停止的任务。"
                    .to_string()
                    .into());
            }
            user_message_id = conversation
                .messages
                .iter()
                .rev()
                .find(|message| message.role == "user")
                .map(|message| message.id.clone())
                .ok_or_else(|| "停止的任务没有原始用户消息。".to_string())?;
            input.user_message_id = Some(user_message_id.clone());
        }
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
                (message.id == user_message_id && response.is_none() && continuation.is_none())
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
        self.storage
            .workflow_execution_bind_run(&conversation_id, &run_id)?;
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
        let prepared_outcome = if let Some(continuation) = continuation.clone() {
            prepare_reserved_continuation_turn(
                &self.storage,
                &self.skills,
                input,
                &run_id,
                previous_conversation.clone(),
                expected_revision,
                continuation,
                &execution_access_check,
            )
        } else if let Some(workflow) = workflow.clone() {
            let claim =
                self.storage
                    .workflow_execution_bind_input(&workflow.id, &run_id, &user_message_id);
            match claim {
                Ok(true) => prepare_reserved_workflow_turn(
                    &self.storage,
                    &self.skills,
                    input,
                    &run_id,
                    previous_conversation.clone(),
                    expected_revision,
                    workflow,
                    &execution_access_check,
                )
                .map(|prepared| PreparedConversationTurnOutcome::Prepared(Box::new(prepared))),
                Ok(false) => Err("workflow input is no longer pending".to_string().into()),
                Err(error) => Err(error.into()),
            }
        } else if let Some(response) = response.clone() {
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
        let prepared_outcome = prepared_outcome.and_then(|outcome| {
            if workflow.is_none()
                && response.is_none()
                && automation.is_none()
                && matches!(&outcome, PreparedConversationTurnOutcome::Prepared(_))
            {
                // A newly admitted explicit user turn lifts Stop before its worker can sample.
                // Workflow deliveries and pre-existing asynchronous answers never lift it.
                self.storage
                    .workflow_execution_resume_conversation(&conversation_id)?;
            }
            Ok(outcome)
        });
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
                if let Some(workflow) = &workflow {
                    let _ = self
                        .storage
                        .workflow_execution_fail_input(&workflow.id, &error.to_string());
                }
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
                if continuation.is_some() {
                    return Err(
                        match self.settle_prepared_continuation_failure(
                            &conversation_id,
                            &assistant_message_id,
                            Some(&run_id),
                            &error.to_string(),
                        ) {
                            Ok(()) => error,
                            Err(settlement_error) => format!(
                                "{error}；同时无法终态化已接受的继续任务：{settlement_error}"
                            )
                            .into(),
                        },
                    );
                }
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

        let rollback = if continuation.is_some() {
            PreparedTurnRollback::Continuation
        } else if response.is_some() {
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
