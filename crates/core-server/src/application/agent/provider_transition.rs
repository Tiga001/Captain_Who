use super::*;
use crate::application::agent_support::{
    AgentProviderTransitionDecision, AgentProviderTransitionOperationError,
    AgentProviderTransitionOperationStatus, AgentProviderTransitionReason,
    AgentProviderTransitionRecovery,
};
use mycopilot_core::storage::models::{
    ChatConversationRecord, ModelConfigRecord, ModelSettingsSnapshot,
};
use mycopilot_core::storage::{
    ProviderTransitionCompatibleCommitOutcome, ProviderTransitionTerminalRecord,
};
use mycopilot_core::{
    estimate_provider_transition_compaction_source_tokens, ContextCompactionReceipt,
    ContextCompactionReceiptStatus, ContextCompactionSummary, ContextContinuitySnapshot,
    ProviderProfileConfig,
};
use mycopilot_protocol_rs::AGENT_PROVIDER_TRANSITION_NOTIFICATION_METHOD;
use serde_json::json;

const PROVIDER_TRANSITION_OPERATION_SCHEMA_VERSION: u32 = 1;
const PROVIDER_TRANSITION_OPERATION_PREFIX: &str = "provider-transition-";
const PROVIDER_TRANSITION_STATUS_LIMIT: usize = 50;

#[derive(Clone)]
struct ProviderTransitionTarget {
    model: ModelConfigRecord,
    profile: ProviderProfileConfig,
    protocol: ProviderProtocolKey,
    protocol_revision: String,
    api_style: mycopilot_core::AgentApiStyle,
    generator_input: AgentChatInput,
}

struct PreparedProviderTransition {
    output: AgentProviderTransitionPreflightOutput,
    conversation: ChatConversationRecord,
    conversation_revision: i64,
    target: ProviderTransitionTarget,
    source_model_display_name: Option<String>,
    target_model_display_name: Option<String>,
    prefix: Option<mycopilot_core::ContextCompactionPrefix>,
    summary_owner_assistant_message_id: Option<String>,
}

enum ProviderTransitionPreparation {
    Ready(Box<PreparedProviderTransition>),
    Blocked(AgentProviderTransitionPreflightOutput),
}

impl AgentService {
    pub(super) fn ensure_provider_transition_ready_for_send(
        &self,
        conversation_id: &str,
        target_model_id: &str,
    ) -> Result<(), AgentServiceError> {
        // The first turn creates its conversation as part of the normal durable handoff. With no
        // persisted history there is no Provider state to transition, so keep the established new
        // conversation path unchanged.
        if self.storage.load_conversation(conversation_id)?.is_none() {
            return Ok(());
        }
        match self.prepare_provider_transition(AgentProviderTransitionPreflightInput {
            conversation_id: conversation_id.to_string(),
            target_model_id: target_model_id.to_string(),
        })? {
            ProviderTransitionPreparation::Ready(prepared)
                if prepared.output.decision == AgentProviderTransitionDecision::Compatible =>
            {
                Ok(())
            }
            ProviderTransitionPreparation::Ready(prepared) => Err(AgentServiceError::structured(
                "切换 API 厂商前需要先压缩历史。",
                json!({
                    "type": "providerTransition",
                    "code": "provider_transition_required",
                    "recovery": "preflightAndCompact",
                    "preflight": prepared.output,
                }),
            )),
            ProviderTransitionPreparation::Blocked(output) => Err(AgentServiceError::structured(
                output
                    .message
                    .clone()
                    .unwrap_or_else(|| "当前会话暂时不能切换模型。".to_string()),
                json!({
                    "type": "providerTransition",
                    "code": "provider_transition_blocked",
                    "recovery": "retryPreflight",
                    "preflight": output,
                }),
            )),
        }
    }

    pub fn preflight_provider_transition(
        &self,
        input: AgentProviderTransitionPreflightInput,
    ) -> Result<AgentProviderTransitionPreflightOutput, AgentServiceError> {
        self.authorize_user_conversation_write(&input.conversation_id)?;
        match self.prepare_provider_transition(input)? {
            ProviderTransitionPreparation::Ready(prepared) => Ok(prepared.output),
            ProviderTransitionPreparation::Blocked(output) => Ok(output),
        }
    }

    pub fn start_provider_transition(
        &self,
        input: AgentProviderTransitionStartInput,
        notifications: CoreServerNotificationSender,
    ) -> Result<AgentProviderTransitionOperation, AgentServiceError> {
        self.authorize_user_conversation_write(&input.conversation_id)?;
        let transition_token = input.transition_token.trim();
        if transition_token.is_empty() {
            return Err("模型切换预检已失效，请重新选择模型后重试。"
                .to_string()
                .into());
        }
        let operation_id = provider_transition_operation_id(transition_token);
        let _admission = self
            .conversation_admission
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if let Some(operation) = self
            .provider_transition_operations
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .get(&operation_id)
            .filter(|operation| {
                operation.conversation_id == input.conversation_id
                    && operation.target_model_id == input.target_model_id
            })
            .cloned()
        {
            return Ok(operation);
        }
        if let Some(record) = self
            .storage
            .get_provider_transition_terminal_record(&operation_id)?
        {
            if record.conversation_id != input.conversation_id
                || record.target_model_id != input.target_model_id
            {
                return Err("模型切换操作与当前会话或目标模型不匹配。"
                    .to_string()
                    .into());
            }
            return Ok(provider_transition_operation_from_terminal_record(&record));
        }
        if let Some(receipt) = self
            .storage
            .list_provider_transition_receipts(
                input.conversation_id.trim(),
                Some(&operation_id),
                1,
            )?
            .into_iter()
            .next()
        {
            let operation = provider_transition_operation_from_receipt(&receipt);
            if operation.target_model_id == input.target_model_id {
                return Ok(operation);
            }
        }
        let prepared =
            match self.prepare_provider_transition(AgentProviderTransitionPreflightInput {
                conversation_id: input.conversation_id.clone(),
                target_model_id: input.target_model_id.clone(),
            })? {
                ProviderTransitionPreparation::Ready(prepared) => prepared,
                ProviderTransitionPreparation::Blocked(_) => {
                    return Err("当前会话暂时不能切换模型，请稍后重试。".to_string().into())
                }
            };
        if prepared.output.transition_token.as_deref() != Some(transition_token)
            || prepared.output.operation_id.as_deref() != Some(operation_id.as_str())
        {
            return Err("模型切换预检已失效，请重新选择模型后重试。"
                .to_string()
                .into());
        }

        let started_at = now_ms();
        let target_model_id = prepared.output.target_model_id.clone();
        if prepared.output.decision == AgentProviderTransitionDecision::Compatible {
            let minimum_updated_at =
                started_at.max(prepared.conversation.updated_at.saturating_add(1));
            let committed = self
                .storage
                .commit_compatible_provider_transition_terminal(
                    &operation_id,
                    &prepared.conversation.id,
                    &target_model_id,
                    prepared.source_model_display_name.as_deref(),
                    prepared.target_model_display_name.as_deref(),
                    started_at,
                    started_at,
                    minimum_updated_at,
                    prepared.conversation.model_id.as_deref(),
                    prepared.conversation.updated_at,
                    prepared.conversation_revision,
                    &prepared.target.protocol_revision,
                )?;
            let record = match committed {
                ProviderTransitionCompatibleCommitOutcome::Committed(record)
                | ProviderTransitionCompatibleCommitOutcome::AlreadyCommitted(record) => record,
                ProviderTransitionCompatibleCommitOutcome::Stale => {
                    return Err("模型切换预检已失效，请重新选择模型后重试。"
                        .to_string()
                        .into())
                }
            };
            self.invalidate_conversation_context_state(&prepared.conversation.id);
            let operation = provider_transition_operation_from_terminal_record(&record);
            self.provider_transition_operations
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .insert(operation_id, operation.clone());
            emit_provider_transition_notification(&notifications, &operation);
            return Ok(operation);
        }

        let prefix = prepared
            .prefix
            .as_ref()
            .ok_or_else(|| "模型切换缺少可压缩的安全历史边界。".to_string())?;
        let summary_owner_assistant_message_id = prepared
            .summary_owner_assistant_message_id
            .as_deref()
            .ok_or_else(|| "模型切换缺少可归属摘要的历史回复。".to_string())?;
        let source_input_tokens = estimate_provider_transition_compaction_source_tokens(
            prefix,
            &prepared.target.model.id,
            prepared.target.api_style,
        )
        .map_err(|_| "目标模型无法安全接收待压缩历史。".to_string())?;
        let target_replacement_tokens = source_input_tokens.saturating_div(4).max(1);
        let run_id = format!("{operation_id}-compaction");
        let receipt = ContextCompactionReceipt::begin_provider_transition(
            operation_id.clone(),
            run_id.clone(),
            prepared.conversation.id.clone(),
            summary_owner_assistant_message_id.to_string(),
            prepared.target.model.id.clone(),
            prepared.source_model_display_name.clone(),
            prepared.target_model_display_name.clone(),
            prepared.target.api_style,
            prefix,
            source_input_tokens,
            target_replacement_tokens,
            started_at,
        )
        .map_err(|_| "无法建立安全的历史压缩任务。".to_string())?;

        {
            let mut transitions = self
                .provider_transitions
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            if transitions.contains_key(&prepared.conversation.id) {
                return Err("当前会话正在切换模型，请稍后重试。".to_string().into());
            }
            transitions.insert(prepared.conversation.id.clone(), operation_id.clone());
        }
        if self
            .storage
            .record_context_compaction_receipt(&receipt, None)
            .is_err()
        {
            self.release_provider_transition_claim(&prepared.conversation.id, &operation_id);
            return Err("无法启动历史压缩，请稍后重试。".to_string().into());
        }

        let running = AgentProviderTransitionOperation {
            schema_version: PROVIDER_TRANSITION_OPERATION_SCHEMA_VERSION,
            operation_id: operation_id.clone(),
            conversation_id: prepared.conversation.id.clone(),
            target_model_id: target_model_id.clone(),
            source_model_display_name: prepared.source_model_display_name.clone(),
            target_model_display_name: prepared.target_model_display_name.clone(),
            status: AgentProviderTransitionOperationStatus::Running,
            started_at,
            completed_at: None,
            conversation_updated_at: None,
            model_id: None,
            summary_id: None,
            covered_through_message_id: Some(prefix.covered_through.message_id().to_string()),
            error: None,
        };
        self.provider_transition_operations
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .insert(operation_id.clone(), running.clone());
        emit_provider_transition_notification(&notifications, &running);

        let service = self.clone();
        let prefix = prefix.clone();
        let target = prepared.target;
        let conversation = prepared.conversation;
        let conversation_revision = prepared.conversation_revision;
        tokio::spawn(async move {
            service
                .run_provider_transition_compaction(
                    operation_id,
                    run_id,
                    target_model_id,
                    conversation,
                    conversation_revision,
                    target,
                    prefix,
                    receipt,
                    notifications,
                )
                .await;
        });

        Ok(running)
    }

    pub fn get_provider_transition_status(
        &self,
        input: AgentProviderTransitionGetStatusInput,
    ) -> Result<AgentProviderTransitionGetStatusOutput, AgentServiceError> {
        let conversation_id = input.conversation_id.trim();
        if conversation_id.is_empty() {
            return Err("conversationId 不能为空。".to_string().into());
        }
        self.authorize_user_conversation_write(conversation_id)?;
        let _conversation = self
            .storage
            .load_conversation(conversation_id)?
            .ok_or_else(|| format!("未找到对话：{conversation_id}"))?;
        let operation_id = input
            .operation_id
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty());
        if operation_id
            .is_some_and(|value| !value.starts_with(PROVIDER_TRANSITION_OPERATION_PREFIX))
        {
            return Err("Provider transition operationId 无效。".to_string().into());
        }
        let receipts = self.storage.list_provider_transition_receipts(
            conversation_id,
            operation_id,
            PROVIDER_TRANSITION_STATUS_LIMIT,
        )?;
        let mut operations = receipts
            .into_iter()
            .filter(|receipt| {
                receipt
                    .operation_id
                    .starts_with(PROVIDER_TRANSITION_OPERATION_PREFIX)
            })
            .map(|receipt| provider_transition_operation_from_receipt(&receipt))
            .collect::<Vec<_>>();
        let terminal_records = if let Some(operation_id) = operation_id {
            self.storage
                .get_provider_transition_terminal_record(operation_id)?
                .filter(|record| record.conversation_id == conversation_id)
                .into_iter()
                .collect::<Vec<_>>()
        } else {
            self.storage.list_provider_transition_terminal_records(
                conversation_id,
                PROVIDER_TRANSITION_STATUS_LIMIT,
            )?
        };
        operations.extend(
            terminal_records
                .iter()
                .map(provider_transition_operation_from_terminal_record),
        );
        let cached = self
            .provider_transition_operations
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        for operation in cached.values().filter(|operation| {
            operation.conversation_id == conversation_id
                && operation_id.is_none_or(|expected| expected == operation.operation_id)
        }) {
            if let Some(existing) = operations
                .iter_mut()
                .find(|existing| existing.operation_id == operation.operation_id)
            {
                *existing = operation.clone();
            } else {
                operations.push(operation.clone());
            }
        }
        operations.sort_by(|left, right| {
            right
                .started_at
                .cmp(&left.started_at)
                .then_with(|| right.operation_id.cmp(&left.operation_id))
        });
        operations.truncate(PROVIDER_TRANSITION_STATUS_LIMIT);
        Ok(AgentProviderTransitionGetStatusOutput { operations })
    }

    fn prepare_provider_transition(
        &self,
        input: AgentProviderTransitionPreflightInput,
    ) -> Result<ProviderTransitionPreparation, AgentServiceError> {
        let conversation_id = input.conversation_id.trim();
        let target_model_id = input.target_model_id.trim();
        if conversation_id.is_empty() || target_model_id.is_empty() {
            return Err("conversationId 和 targetModelId 不能为空。"
                .to_string()
                .into());
        }
        let (conversation, conversation_revision) =
            self.storage.load_conversation_for_turn(conversation_id)?;
        let conversation = conversation.ok_or_else(|| format!("未找到对话：{conversation_id}"))?;
        let conversation_revision = conversation_revision
            .ok_or_else(|| "模型切换预检缺少 Conversation revision。".to_string())?;
        if let Some(agent) = self
            .storage
            .get_agent_node_by_conversation(conversation_id)
            .map_err(|error| error.to_string())?
        {
            if agent.parent_agent_id.is_some()
                || agent.lifecycle != mycopilot_core::AgentLifecycle::Active
            {
                return Ok(ProviderTransitionPreparation::Blocked(
                    provider_transition_blocked_output(
                        conversation_id,
                        target_model_id,
                        AgentProviderTransitionReason::UnsupportedTarget,
                        if agent.parent_agent_id.is_some() {
                            "子 Agent Conversation 是只读观察视图，不能由用户切换模型。"
                        } else {
                            "根 Agent 当前不可用，不能切换模型。"
                        },
                    ),
                ));
            }
        }
        if self
            .provider_transitions
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .contains_key(conversation_id)
        {
            return Ok(ProviderTransitionPreparation::Blocked(
                provider_transition_blocked_output(
                    conversation_id,
                    target_model_id,
                    AgentProviderTransitionReason::ActiveRun,
                    "当前会话正在切换模型。",
                ),
            ));
        }
        if self.has_conversation_turn_occupancy(conversation_id)? {
            return Ok(ProviderTransitionPreparation::Blocked(
                provider_transition_blocked_output(
                    conversation_id,
                    target_model_id,
                    AgentProviderTransitionReason::ActiveRun,
                    "请等待当前回复完成后再切换模型。",
                ),
            ));
        }
        if self
            .pending_actions
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .values()
            .any(|action| {
                action.snapshot.conversation_id.as_deref() == Some(conversation_id)
                    && matches!(
                        action.snapshot.status,
                        PendingActionStatus::Pending
                            | PendingActionStatus::Approved
                            | PendingActionStatus::Executing
                    )
            })
        {
            return Ok(ProviderTransitionPreparation::Blocked(
                provider_transition_blocked_output(
                    conversation_id,
                    target_model_id,
                    AgentProviderTransitionReason::PendingApproval,
                    "请先处理当前待审批操作。",
                ),
            ));
        }
        if conversation.model_id.is_none() && !conversation.messages.is_empty() {
            return Ok(ProviderTransitionPreparation::Blocked(
                provider_transition_blocked_output(
                    conversation_id,
                    target_model_id,
                    AgentProviderTransitionReason::UnsupportedTarget,
                    "当前会话历史缺少已冻结的模型身份，无法安全切换。",
                ),
            ));
        }

        let settings_snapshot = self
            .storage
            .load_model_settings_snapshot()?
            .ok_or_else(|| "请先配置模型。".to_string())?;
        let target = match resolve_provider_transition_target(
            &self.storage,
            &conversation,
            &settings_snapshot,
            target_model_id,
        ) {
            Ok(target) => target,
            Err(_) => {
                return Ok(ProviderTransitionPreparation::Blocked(
                    provider_transition_blocked_output(
                        conversation_id,
                        target_model_id,
                        AgentProviderTransitionReason::UnsupportedTarget,
                        "目标模型的 API 兼容配置不可用。",
                    ),
                ));
            }
        };
        let source_model_display_name = conversation.model_id.as_deref().map(|source_model_id| {
            settings_snapshot
                .settings
                .models
                .iter()
                .find(|model| model.id == source_model_id)
                .map(|model| model.display_name.trim())
                .filter(|display_name| !display_name.is_empty())
                .map(str::to_string)
                .unwrap_or_else(|| source_model_id.to_string())
        });
        let target_model_display_name = source_model_display_name.as_ref().map(|_| {
            if target.model.display_name.trim().is_empty() {
                target.model.id.clone()
            } else {
                target.model.display_name.trim().to_string()
            }
        });

        let (preview_input, traces) = self
            .persisted_conversation_context_state(&target.generator_input, conversation_id)
            .map_err(AgentServiceError::from)?;
        if traces
            .iter()
            .any(|trace| !trace.terminal_status.is_terminal())
        {
            return Ok(ProviderTransitionPreparation::Blocked(
                provider_transition_blocked_output(
                    conversation_id,
                    target_model_id,
                    AgentProviderTransitionReason::ActiveRun,
                    "请等待当前回复完成后再切换模型。",
                ),
            ));
        }

        let fork_requires_context_adaptation = self
            .storage
            .conversation_requires_context_adaptation(conversation_id)?;
        let requires_compaction = if fork_requires_context_adaptation {
            // A timeline fork may intentionally retain only the durable, Provider-neutral raw
            // journal after the source ciphertext was released. Never let a superficially
            // compatible target bypass the one required tool-free adaptation compaction.
            true
        } else {
            let host_services = self.context_window_provider_host_services();
            let compatibility = create_conversation_context_state_with_host_services(
                preview_input.clone(),
                conversation_id,
                &host_services,
            );
            match compatibility {
                Ok(_) => false,
                Err(error) if provider_transition_error_requires_compaction(error.code()) => {
                    // The transition request is deliberately tool-free and receives only the
                    // provider-neutral durable journal rendered as untrusted text. It does not
                    // need to decrypt or replay the old opaque continuation. Prefix construction
                    // and the atomic coverage commit below remain the authoritative safety checks.
                    true
                }
                Err(_) => {
                    return Ok(ProviderTransitionPreparation::Blocked(
                        provider_transition_blocked_output(
                            conversation_id,
                            target_model_id,
                            AgentProviderTransitionReason::UnsupportedTarget,
                            "目标模型无法安全使用当前会话。",
                        ),
                    ));
                }
            }
        };

        let summary_owner_assistant_message_id = preview_input
            .messages
            .iter()
            .rev()
            .find(|message| message.role == "assistant")
            .and_then(|message| message.message_id.clone());
        let prefix = if requires_compaction {
            let covered_through_message_id = preview_input
                .messages
                .last()
                .and_then(|message| message.message_id.as_deref())
                .ok_or_else(|| "当前不兼容历史缺少可压缩的安全消息边界。".to_string())?;
            Some(
                self.storage
                    .prepare_context_compaction_prefix(
                        conversation_id,
                        &ContextJournalCursor::message(covered_through_message_id),
                    )
                    .map_err(AgentServiceError::from)?,
            )
        } else {
            None
        };
        if requires_compaction && summary_owner_assistant_message_id.is_none() {
            return Ok(ProviderTransitionPreparation::Blocked(
                provider_transition_blocked_output(
                    conversation_id,
                    target_model_id,
                    AgentProviderTransitionReason::UnsupportedTarget,
                    "当前历史缺少可安全归属的回复边界。",
                ),
            ));
        }

        let source_fingerprint = match prefix.as_ref() {
            Some(prefix) => prefix.source_revision.clone(),
            None => provider_transition_context_fingerprint(
                &conversation,
                preview_input.context_compaction_summary.as_ref(),
            )?,
        };
        let durable_failed_attempt_count = self
            .storage
            .provider_transition_failed_attempt_count(conversation_id, target_model_id)?;
        let cached_failed_attempt_count = self
            .provider_transition_operations
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .values()
            .filter(|operation| {
                operation.conversation_id == conversation_id
                    && operation.target_model_id == target_model_id
                    && operation.status == AgentProviderTransitionOperationStatus::Failed
            })
            .count() as u64;
        // Counts, unlike wall-clock timestamps or opaque operation hashes, form an unambiguous
        // monotonic retry generation even when multiple failures complete in the same millisecond.
        // A normal failure is present in both durable storage and the in-memory cache, so summing
        // would double-count it and could regenerate an older token after an application restart.
        // The maximum also covers a cache-only terminal failure until startup reconciliation makes
        // the durable receipt authoritative.
        let failed_attempt_generation =
            durable_failed_attempt_count.max(cached_failed_attempt_count);
        let transition_token = provider_transition_token(
            conversation_id,
            conversation.model_id.as_deref(),
            conversation.updated_at,
            conversation_revision,
            target_model_id,
            &target.protocol,
            &source_fingerprint,
            failed_attempt_generation,
        )?;
        let operation_id = provider_transition_operation_id(&transition_token);
        let reason = if requires_compaction {
            provider_transition_change_reason(
                &settings_snapshot,
                conversation.model_id.as_deref(),
                &target.profile,
            )
        } else if conversation.model_id.as_deref() == Some(target_model_id) {
            AgentProviderTransitionReason::SameProtocol
        } else {
            AgentProviderTransitionReason::NoIncompatibleHistory
        };
        let output = AgentProviderTransitionPreflightOutput {
            conversation_id: conversation_id.to_string(),
            target_model_id: target_model_id.to_string(),
            decision: if requires_compaction {
                AgentProviderTransitionDecision::RequiresCompaction
            } else {
                AgentProviderTransitionDecision::Compatible
            },
            reason,
            operation_id: Some(operation_id),
            transition_token: Some(transition_token),
            message: requires_compaction.then(|| match reason {
                AgentProviderTransitionReason::ApiProviderChanged => {
                    "API 厂商不同，新模型需要先压缩历史完成适配。".to_string()
                }
                _ => "模型兼容规则已变化，需要先压缩历史完成适配。".to_string(),
            }),
        };
        Ok(ProviderTransitionPreparation::Ready(Box::new(
            PreparedProviderTransition {
                output,
                conversation,
                conversation_revision,
                target,
                source_model_display_name,
                target_model_display_name,
                prefix,
                summary_owner_assistant_message_id,
            },
        )))
    }

    #[allow(clippy::too_many_arguments)]
    async fn run_provider_transition_compaction(
        &self,
        operation_id: String,
        run_id: String,
        target_model_id: String,
        conversation: ChatConversationRecord,
        conversation_revision: i64,
        target: ProviderTransitionTarget,
        prefix: mycopilot_core::ContextCompactionPrefix,
        mut receipt: ContextCompactionReceipt,
        notifications: CoreServerNotificationSender,
    ) {
        let result = self
            .execute_provider_transition_compaction(
                &run_id,
                &conversation,
                conversation_revision,
                &target,
                &prefix,
                &mut receipt,
            )
            .await;
        let operation = match result {
            Ok((summary, conversation_updated_at)) => {
                self.invalidate_conversation_context_state(&conversation.id);
                AgentProviderTransitionOperation {
                    schema_version: PROVIDER_TRANSITION_OPERATION_SCHEMA_VERSION,
                    operation_id: operation_id.clone(),
                    conversation_id: conversation.id.clone(),
                    target_model_id: target_model_id.clone(),
                    source_model_display_name: receipt
                        .provider_transition_source_model_display_name
                        .clone(),
                    target_model_display_name: receipt
                        .provider_transition_target_model_display_name
                        .clone(),
                    status: AgentProviderTransitionOperationStatus::Completed,
                    started_at: receipt.started_at,
                    completed_at: Some(summary.created_at),
                    conversation_updated_at: Some(conversation_updated_at),
                    model_id: Some(target_model_id),
                    summary_id: Some(summary.id),
                    covered_through_message_id: Some(
                        prefix.covered_through.message_id().to_string(),
                    ),
                    error: None,
                }
            }
            Err(error) => AgentProviderTransitionOperation {
                schema_version: PROVIDER_TRANSITION_OPERATION_SCHEMA_VERSION,
                operation_id: operation_id.clone(),
                conversation_id: conversation.id.clone(),
                target_model_id,
                source_model_display_name: receipt
                    .provider_transition_source_model_display_name
                    .clone(),
                target_model_display_name: receipt
                    .provider_transition_target_model_display_name
                    .clone(),
                status: AgentProviderTransitionOperationStatus::Failed,
                started_at: receipt.started_at,
                completed_at: Some(now_ms()),
                conversation_updated_at: None,
                model_id: None,
                summary_id: None,
                covered_through_message_id: Some(prefix.covered_through.message_id().to_string()),
                error: Some(error),
            },
        };
        self.provider_transition_operations
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .insert(operation_id.clone(), operation.clone());
        self.release_provider_transition_claim(&conversation.id, &operation_id);
        emit_provider_transition_notification(&notifications, &operation);
    }

    async fn execute_provider_transition_compaction(
        &self,
        run_id: &str,
        conversation: &ChatConversationRecord,
        conversation_revision: i64,
        target: &ProviderTransitionTarget,
        prefix: &mycopilot_core::ContextCompactionPrefix,
        receipt: &mut ContextCompactionReceipt,
    ) -> Result<(ContextCompactionSummary, i64), AgentProviderTransitionOperationError> {
        if let Err(error) = receipt.advance_stage(
            mycopilot_core::ContextCompactionReceiptStage::Preparing,
            now_ms(),
        ) {
            return self.fail_provider_transition_receipt(
                run_id,
                receipt,
                provider_transition_agent_error(
                    "provider_transition_generation_failed",
                    "历史压缩失败，模型未切换。",
                    &error,
                ),
                None,
            );
        }
        if let Err(error) = self
            .storage
            .record_context_compaction_receipt(receipt, None)
        {
            return self.fail_provider_transition_receipt(
                run_id,
                receipt,
                provider_transition_agent_error(
                    "provider_transition_generation_failed",
                    "历史压缩失败，模型未切换。",
                    &error,
                ),
                None,
            );
        }
        if let Err(error) = receipt.attach_prepared_prefix(prefix, now_ms()) {
            return self.fail_provider_transition_receipt(
                run_id,
                receipt,
                provider_transition_agent_error(
                    "provider_transition_generation_failed",
                    "历史压缩失败，模型未切换。",
                    &error,
                ),
                None,
            );
        }
        if let Err(error) = self
            .storage
            .record_context_compaction_receipt(receipt, None)
        {
            return self.fail_provider_transition_receipt(
                run_id,
                receipt,
                provider_transition_agent_error(
                    "provider_transition_generation_failed",
                    "历史压缩失败，模型未切换。",
                    &error,
                ),
                None,
            );
        }

        let continuity = match ContextContinuitySnapshot::from_prefix(prefix) {
            Ok(continuity) => continuity,
            Err(error) => {
                return self.fail_provider_transition_receipt(
                    run_id,
                    receipt,
                    provider_transition_agent_error(
                        "provider_transition_generation_failed",
                        "历史压缩失败，模型未切换。",
                        &error,
                    ),
                    None,
                )
            }
        };
        let request = AgentContextCompactionGenerationRequest {
            operation_id: receipt.operation_id.clone(),
            run_id: run_id.to_string(),
            conversation_id: conversation.id.clone(),
            assistant_message_id: receipt.assistant_message_id.clone(),
            request_index: 1,
            prefix: Arc::new(prefix.clone()),
            continuity,
            source_input_tokens: receipt.plan.source_input_tokens,
            uncovered_tail_input_tokens: 0,
            target_replacement_tokens: receipt.plan.target_replacement_tokens,
        };
        let generator = self
            .context_compaction_summary_generator
            .clone()
            .unwrap_or_else(|| {
                let generator =
                    AgentContextCompactionModelGenerator::from_chat_input(&target.generator_input);
                Arc::new(move |request, cancellation| {
                    let generator = generator.clone();
                    Box::pin(async move { generator.generate(request, cancellation).await })
                })
            });
        let cancellation = AgentCancellationToken::new();
        let generated = match generator(request, cancellation).await {
            Ok(generated) => generated,
            Err(error) => {
                let observation = error.model_request_observation().cloned();
                return self.fail_provider_transition_receipt(
                    run_id,
                    receipt,
                    provider_transition_agent_error(
                        "provider_transition_generation_failed",
                        "历史压缩失败，模型未切换。",
                        &error,
                    ),
                    observation.as_ref(),
                );
            }
        };
        if let Err(error) = receipt.advance_stage(
            mycopilot_core::ContextCompactionReceiptStage::Committing,
            now_ms(),
        ) {
            return self.fail_provider_transition_receipt(
                run_id,
                receipt,
                provider_transition_agent_error(
                    "provider_transition_commit_failed",
                    "历史压缩结果未能提交，模型未切换。",
                    &error,
                ),
                Some(&generated.observation),
            );
        }
        if let Err(error) = self
            .storage
            .record_context_compaction_receipt(receipt, None)
        {
            return self.fail_provider_transition_receipt(
                run_id,
                receipt,
                provider_transition_agent_error(
                    "provider_transition_commit_failed",
                    "历史压缩结果未能提交，模型未切换。",
                    &AgentError::new(error),
                ),
                Some(&generated.observation),
            );
        }
        let mut applied = receipt.clone();
        if let Err(error) =
            applied.complete_applied(&generated.draft, &generated.observation, now_ms())
        {
            return self.fail_provider_transition_receipt(
                run_id,
                receipt,
                provider_transition_agent_error(
                    "provider_transition_commit_failed",
                    "历史压缩结果未能提交，模型未切换。",
                    &error,
                ),
                Some(&generated.observation),
            );
        }
        let committed = match self
            .storage
            .commit_provider_transition_with_receipt_if_current(
                prefix,
                generated.draft,
                &applied,
                &generated.observation,
                conversation.model_id.as_deref(),
                conversation.updated_at,
                conversation_revision,
                &target.model.id,
                &target.protocol_revision,
            ) {
            Ok(committed) => committed,
            Err(error) => {
                return self.fail_provider_transition_receipt(
                    run_id,
                    receipt,
                    provider_transition_agent_error(
                        "provider_transition_commit_failed",
                        "历史压缩结果未能提交，模型未切换。",
                        &error,
                    ),
                    Some(&generated.observation),
                );
            }
        };
        let Some((summary, conversation_updated_at)) = committed else {
            return self.fail_provider_transition_receipt(
                run_id,
                receipt,
                AgentError::structured(
                    "provider_transition_stale",
                    "会话或模型配置已变化，历史压缩未提交。",
                    json!({}),
                ),
                Some(&generated.observation),
            );
        };
        applied.updated_at = applied.updated_at.max(conversation_updated_at);
        *receipt = applied;
        Ok((summary, conversation_updated_at))
    }

    fn fail_provider_transition_receipt(
        &self,
        _run_id: &str,
        receipt: &mut ContextCompactionReceipt,
        error: AgentError,
        observation: Option<&mycopilot_core::ModelRequestObservation>,
    ) -> Result<(ContextCompactionSummary, i64), AgentProviderTransitionOperationError> {
        let safe = provider_transition_operation_error_from_agent_error(&error);
        if receipt
            .complete_error(&error, observation, now_ms())
            .is_ok()
        {
            let _ = self
                .storage
                .record_context_compaction_receipt(receipt, observation);
        }
        Err(safe)
    }

    fn release_provider_transition_claim(&self, conversation_id: &str, operation_id: &str) {
        let mut transitions = self
            .provider_transitions
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if transitions.get(conversation_id).map(String::as_str) == Some(operation_id) {
            transitions.remove(conversation_id);
        }
    }
}

fn resolve_provider_transition_target(
    storage: &StorageService,
    conversation: &ChatConversationRecord,
    snapshot: &ModelSettingsSnapshot,
    target_model_id: &str,
) -> Result<ProviderTransitionTarget, String> {
    let model = snapshot
        .settings
        .models
        .iter()
        .find(|model| model.id == target_model_id)
        .cloned()
        .ok_or_else(|| "target model is not registered".to_string())?;
    if !model.enabled {
        return Err("target model is disabled".to_string());
    }
    let connection = snapshot.settings.effective_connection_for(&model)?;
    let dialect = ProviderProtocolDialect::detect_from_api_url(&connection.api_url);
    let profile = model
        .resolved_provider_profile_config(dialect)
        .map_err(|error| error.to_string())?;
    let protocol_revision = snapshot
        .provider_protocol_revisions
        .get(&model.id)
        .cloned()
        .ok_or_else(|| "target model protocol revision is missing".to_string())?;
    let connection_revision = snapshot
        .provider_connection_revisions
        .get(&model.id)
        .cloned()
        .ok_or_else(|| "target model connection revision is missing".to_string())?;
    let protocol = ProviderProtocolKey::new(
        dialect,
        &profile,
        model.id.clone(),
        Some(protocol_revision.clone()),
    )
    .map_err(|error| error.to_string())?;
    mycopilot_core::resolve_provider_runtime_capabilities(&protocol)
        .map_err(|error| error.to_string())?;
    let attachment_library = storage
        .build_attachment_library_context(&conversation.id, conversation.project_id.as_deref())?;
    let prompt_preferences =
        agent_prompt_preferences_from_record(storage.load_agent_prompt_preferences()?);
    let generator_input = AgentChatInput {
        api_url: connection.api_url,
        api_token: connection.api_token,
        provider_configuration_revision: Some(protocol_revision.clone()),
        provider_connection_revision: Some(connection_revision),
        search_connection_revision: Some(snapshot.search_connection_revision.clone()),
        provider_profile_config: Some(profile.clone()),
        provider_protocol_key: Some(protocol.clone()),
        model: model.id.clone(),
        model_capabilities: ModelCapabilities {
            image_input: model.supports_image,
        },
        api_style: Some(dialect.api_style()),
        context_window_tokens: Some(model.effective_context_window_tokens()),
        context_window_indicator_enabled: false,
        max_tokens: None,
        temperature: None,
        stream: Some(true),
        context: Some(AgentRunContext {
            collaboration_identity: None,
            conversation_id: Some(conversation.id.clone()),
            project_id: conversation.project_id.clone(),
            workspace: None,
            attachment_library: Some(attachment_library),
            permissions: Default::default(),
        }),
        search_config: Some(AgentSearchConfig {
            mode: search_mode_from_storage(&snapshot.settings.search_mode),
            tavily_api_key: non_empty(snapshot.settings.tavily_api_key.clone()),
        }),
        prompt_preferences: Some(prompt_preferences),
        approval_decision: None,
        tool_continuation: None,
        attachments: Vec::new(),
        resume_checkpoint: None,
        assistant_message_id: None,
        context_compaction_summary: None,
        world_state_records: Vec::new(),
        skill_activation: None,
        skill_discovery: None,
        messages: Vec::new(),
    };
    Ok(ProviderTransitionTarget {
        model,
        profile,
        protocol,
        protocol_revision,
        api_style: dialect.api_style(),
        generator_input,
    })
}

fn provider_transition_change_reason(
    snapshot: &ModelSettingsSnapshot,
    current_model_id: Option<&str>,
    target_profile: &ProviderProfileConfig,
) -> AgentProviderTransitionReason {
    let current_profile = current_model_id
        .and_then(|current_model_id| {
            snapshot
                .settings
                .models
                .iter()
                .find(|model| model.id == current_model_id)
        })
        .and_then(|model| {
            snapshot
                .settings
                .effective_connection_for(model)
                .ok()
                .map(|connection| ProviderProtocolDialect::detect_from_api_url(&connection.api_url))
                .and_then(|dialect| model.resolved_provider_profile_config(dialect).ok())
        });
    if current_profile
        .as_ref()
        .is_some_and(|profile| profile.profile() != target_profile.profile())
    {
        AgentProviderTransitionReason::ApiProviderChanged
    } else {
        AgentProviderTransitionReason::ProviderProtocolChanged
    }
}

fn provider_transition_context_fingerprint(
    conversation: &ChatConversationRecord,
    summary: Option<&ContextCompactionSummary>,
) -> Result<String, AgentServiceError> {
    let encoded = serde_json::to_vec(&json!({
        "conversation": conversation,
        "summaryId": summary.map(|summary| summary.id.as_str()),
        "summarySourceRevision": summary.map(|summary| summary.source_revision.as_str()),
    }))
    .map_err(|_| AgentServiceError::from("无法建立会话切换预检身份。".to_string()))?;
    Ok(mycopilot_core::content_revision(&encoded))
}

#[allow(clippy::too_many_arguments)]
fn provider_transition_token(
    conversation_id: &str,
    current_model_id: Option<&str>,
    conversation_updated_at: i64,
    conversation_revision: i64,
    target_model_id: &str,
    target_protocol: &ProviderProtocolKey,
    source_fingerprint: &str,
    failed_attempt_generation: u64,
) -> Result<String, AgentServiceError> {
    let encoded = serde_json::to_vec(&json!({
        "schemaVersion": PROVIDER_TRANSITION_OPERATION_SCHEMA_VERSION,
        "conversationId": conversation_id,
        "currentModelId": current_model_id,
        "conversationUpdatedAt": conversation_updated_at,
        "conversationRevision": conversation_revision,
        "targetModelId": target_model_id,
        "targetProtocol": target_protocol,
        "sourceFingerprint": source_fingerprint,
        "failedAttemptGeneration": failed_attempt_generation,
    }))
    .map_err(|_| AgentServiceError::from("无法建立模型切换预检身份。".to_string()))?;
    Ok(format!(
        "provider-transition-v1:{}",
        mycopilot_core::content_revision(&encoded)
    ))
}

fn provider_transition_operation_id(transition_token: &str) -> String {
    format!(
        "{PROVIDER_TRANSITION_OPERATION_PREFIX}{}",
        mycopilot_core::content_revision(transition_token.as_bytes())
    )
}

fn provider_transition_blocked_output(
    conversation_id: &str,
    target_model_id: &str,
    reason: AgentProviderTransitionReason,
    message: &str,
) -> AgentProviderTransitionPreflightOutput {
    AgentProviderTransitionPreflightOutput {
        conversation_id: conversation_id.to_string(),
        target_model_id: target_model_id.to_string(),
        decision: AgentProviderTransitionDecision::Blocked,
        reason,
        operation_id: None,
        transition_token: None,
        message: Some(message.to_string()),
    }
}

fn emit_provider_transition_notification(
    notifications: &CoreServerNotificationSender,
    operation: &AgentProviderTransitionOperation,
) {
    let _ = notifications.send(json!({
        "jsonrpc": "2.0",
        "method": AGENT_PROVIDER_TRANSITION_NOTIFICATION_METHOD,
        "params": operation,
    }));
}

fn provider_transition_operation_from_terminal_record(
    record: &ProviderTransitionTerminalRecord,
) -> AgentProviderTransitionOperation {
    AgentProviderTransitionOperation {
        schema_version: PROVIDER_TRANSITION_OPERATION_SCHEMA_VERSION,
        operation_id: record.operation_id.clone(),
        conversation_id: record.conversation_id.clone(),
        target_model_id: record.target_model_id.clone(),
        source_model_display_name: record.source_model_display_name.clone(),
        target_model_display_name: record.target_model_display_name.clone(),
        status: AgentProviderTransitionOperationStatus::Completed,
        started_at: record.started_at,
        completed_at: Some(record.completed_at),
        conversation_updated_at: Some(record.conversation_updated_at),
        model_id: Some(record.target_model_id.clone()),
        summary_id: None,
        covered_through_message_id: None,
        error: None,
    }
}

fn provider_transition_operation_from_receipt(
    receipt: &ContextCompactionReceipt,
) -> AgentProviderTransitionOperation {
    let status = match receipt.status {
        ContextCompactionReceiptStatus::InProgress => {
            AgentProviderTransitionOperationStatus::Running
        }
        ContextCompactionReceiptStatus::Applied => {
            AgentProviderTransitionOperationStatus::Completed
        }
        ContextCompactionReceiptStatus::Refreshed
        | ContextCompactionReceiptStatus::Failed
        | ContextCompactionReceiptStatus::Cancelled
        | ContextCompactionReceiptStatus::Interrupted => {
            AgentProviderTransitionOperationStatus::Failed
        }
    };
    let error = (status == AgentProviderTransitionOperationStatus::Failed).then(|| {
        let interrupted = receipt.status == ContextCompactionReceiptStatus::Interrupted;
        AgentProviderTransitionOperationError {
            code: if interrupted {
                "provider_transition_interrupted".to_string()
            } else {
                receipt
                    .error
                    .as_ref()
                    .and_then(|error| error.code.as_deref())
                    .filter(|code| {
                        matches!(
                            *code,
                            "provider_transition_stale"
                                | "provider_transition_generation_failed"
                                | "provider_transition_commit_failed"
                        )
                    })
                    .unwrap_or("provider_transition_generation_failed")
                    .to_string()
            },
            message: if interrupted {
                "历史压缩因应用重启中断，模型未切换。".to_string()
            } else {
                "历史压缩失败，模型未切换。".to_string()
            },
            recovery: AgentProviderTransitionRecovery::Retry,
        }
    });
    AgentProviderTransitionOperation {
        schema_version: PROVIDER_TRANSITION_OPERATION_SCHEMA_VERSION,
        operation_id: receipt.operation_id.clone(),
        conversation_id: receipt.conversation_id.clone(),
        target_model_id: receipt.model.clone(),
        source_model_display_name: receipt
            .provider_transition_source_model_display_name
            .clone(),
        target_model_display_name: receipt
            .provider_transition_target_model_display_name
            .clone(),
        status,
        started_at: receipt.started_at,
        completed_at: receipt.completed_at,
        conversation_updated_at: (status == AgentProviderTransitionOperationStatus::Completed)
            .then_some(receipt.updated_at),
        model_id: (status == AgentProviderTransitionOperationStatus::Completed)
            .then(|| receipt.model.clone()),
        summary_id: receipt.summary_id.clone(),
        covered_through_message_id: Some(receipt.plan.covered_through.message_id().to_string()),
        error,
    }
}

fn provider_transition_agent_error(
    code: &'static str,
    safe_message: &'static str,
    _cause: &impl std::fmt::Display,
) -> AgentError {
    AgentError::structured(code, safe_message, json!({}))
}

fn provider_transition_operation_error_from_agent_error(
    error: &AgentError,
) -> AgentProviderTransitionOperationError {
    let code = match error.code() {
        Some("provider_transition_stale") => "provider_transition_stale",
        Some("provider_transition_commit_failed") => "provider_transition_commit_failed",
        _ => "provider_transition_generation_failed",
    };
    AgentProviderTransitionOperationError {
        code: code.to_string(),
        message: if code == "provider_transition_stale" {
            "会话或模型配置已变化，模型未切换。".to_string()
        } else {
            "历史压缩失败，模型未切换。".to_string()
        },
        recovery: AgentProviderTransitionRecovery::Retry,
    }
}

fn provider_transition_error_requires_compaction(code: Option<&str>) -> bool {
    matches!(
        code,
        Some("provider_context_boundary_required")
            | Some("provider_continuation_missing")
            | Some("provider_continuation_unavailable")
            | Some("provider_continuation_corrupt")
            | Some("agent.invalid_model_tool_call_id")
    )
}

#[cfg(test)]
mod provider_transition_unit_tests {
    use super::*;

    #[test]
    fn opaque_vault_failures_require_a_new_text_only_epoch() {
        assert!(provider_transition_error_requires_compaction(Some(
            "provider_continuation_unavailable"
        )));
        assert!(provider_transition_error_requires_compaction(Some(
            "provider_continuation_corrupt"
        )));
        assert!(!provider_transition_error_requires_compaction(Some(
            "provider_profile_unsupported"
        )));
    }

    #[test]
    fn one_preflight_token_is_idempotent_but_a_failed_attempt_rotates_the_next_token() {
        let dialect = ProviderProtocolDialect::OpenAiChatCompletions;
        let profile = ProviderProfileConfig::generic_for_dialect(dialect);
        let protocol = ProviderProtocolKey::new(
            dialect,
            &profile,
            "target-model",
            Some("target-revision".to_string()),
        )
        .unwrap();
        let first = provider_transition_token(
            "conversation-1",
            None,
            1,
            1,
            "target-model",
            &protocol,
            "source-revision",
            0,
        )
        .unwrap();
        assert_eq!(
            provider_transition_operation_id(&first),
            provider_transition_operation_id(&first)
        );
        let newer_conversation_revision = provider_transition_token(
            "conversation-1",
            None,
            1,
            2,
            "target-model",
            &protocol,
            "source-revision",
            0,
        )
        .unwrap();
        assert_ne!(first, newer_conversation_revision);
        let retry = provider_transition_token(
            "conversation-1",
            None,
            1,
            1,
            "target-model",
            &protocol,
            "source-revision",
            1,
        )
        .unwrap();
        assert_ne!(first, retry);
        assert_ne!(
            provider_transition_operation_id(&first),
            provider_transition_operation_id(&retry)
        );
    }
}
