use super::provider_transition::{resolve_provider_transition_target, ProviderTransitionTarget};
use super::*;
use mycopilot_core::storage::models::{
    ManualContextCompactionOperation, ManualContextCompactionUsageRecord,
};
use mycopilot_core::{
    ContextCompactionPrefix, ContextCompactionReceipt, ContextCompactionReceiptStage,
    ContextContinuitySnapshot, ModelRequestObservation,
};
use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentManualContextCompactionStartInput {
    pub conversation_id: String,
    pub request_id: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentManualContextCompactionStatusInput {
    pub conversation_id: String,
    pub operation_id: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentManualContextCompactionCancelInput {
    pub conversation_id: String,
    pub operation_id: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentManualContextCompactionOperation {
    pub schema_version: u32,
    pub operation_id: String,
    pub request_id: String,
    pub conversation_id: String,
    pub status: String,
    pub phase: String,
    pub is_busy: bool,
    pub started_at: i64,
    pub updated_at: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub completed_at: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub covered_through_message_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl From<ManualContextCompactionOperation> for AgentManualContextCompactionOperation {
    fn from(value: ManualContextCompactionOperation) -> Self {
        Self {
            schema_version: 1,
            operation_id: value.operation_id,
            request_id: value.request_id,
            conversation_id: value.conversation_id,
            is_busy: value.status == "running",
            status: value.status,
            phase: value.phase,
            started_at: value.started_at,
            updated_at: value.updated_at,
            completed_at: value.completed_at,
            model_id: value.model_id,
            summary_id: value.summary_id,
            covered_through_message_id: value.covered_through_message_id,
            error: value.error,
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentManualContextCompactionStatusOutput {
    pub operations: Vec<AgentManualContextCompactionOperation>,
}

impl AgentService {
    fn project_manual_operation(
        &self,
        operation: ManualContextCompactionOperation,
    ) -> AgentManualContextCompactionOperation {
        let draining = self
            .manual_context_compaction_cancellations
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .contains_key(&operation.operation_id);
        let mut projected = AgentManualContextCompactionOperation::from(operation);
        projected.is_busy |= draining;
        projected
    }

    pub fn start_manual_context_compaction(
        &self,
        input: AgentManualContextCompactionStartInput,
        notifications: CoreServerNotificationSender,
    ) -> Result<AgentManualContextCompactionOperation, AgentServiceError> {
        validate_identity(&input.conversation_id, "conversationId")?;
        validate_identity(&input.request_id, "requestId")?;
        self.authorize_user_conversation_write(&input.conversation_id)?;
        let _admission = self
            .conversation_admission
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let operation_id = format!(
            "manual-compaction-{}",
            mycopilot_core::content_revision(
                &serde_json::to_vec(&(&input.conversation_id, &input.request_id))
                    .map_err(|e| e.to_string())?
            )
        );
        if let Some(existing) = self
            .storage
            .get_manual_context_compaction(&input.conversation_id, Some(&operation_id))?
        {
            return Ok(self.project_manual_operation(existing));
        }
        self.ensure_no_manual_context_compaction(&input.conversation_id)?;
        let conversation = self
            .storage
            .load_conversation(&input.conversation_id)?
            .ok_or_else(|| "请先开始聊天，再压缩上下文。".to_string())?;
        if conversation.archived_at.is_some()
            || self.is_conversation_deleting(Some(&conversation.id))
            || self.is_project_deleting(conversation.project_id.as_deref())
        {
            return Err("当前聊天不可压缩。".to_string().into());
        }
        if let Some(agent) = self
            .storage
            .get_agent_node_by_conversation(&conversation.id)
            .map_err(|e| e.to_string())?
        {
            if agent.parent_agent_id.is_some()
                || agent.lifecycle != mycopilot_core::AgentLifecycle::Active
            {
                return Err("只能压缩当前可用的根聊天。".to_string().into());
            }
        }
        if self.has_conversation_turn_occupancy(&conversation.id)?
            || self
                .provider_transitions
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .contains_key(&conversation.id)
            || self
                .pending_actions
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .values()
                .any(|action| {
                    action.snapshot.conversation_id.as_deref() == Some(&conversation.id)
                        && matches!(
                            action.snapshot.status,
                            PendingActionStatus::Pending
                                | PendingActionStatus::Approved
                                | PendingActionStatus::Executing
                        )
                })
        {
            return Err("请等待当前运行或待审批操作完成后再压缩上下文。"
                .to_string()
                .into());
        }
        let now = now_ms();
        let mut operation = ManualContextCompactionOperation {
            operation_id,
            request_id: input.request_id,
            conversation_id: conversation.id.clone(),
            status: "running".into(),
            phase: "preparing".into(),
            assistant_message_id: None,
            covered_through_message_id: None,
            model_id: conversation.model_id.clone(),
            summary_id: None,
            source_input_tokens: None,
            replacement_input_tokens: None,
            error: None,
            started_at: now,
            updated_at: now,
            completed_at: None,
        };
        let assistant = conversation
            .messages
            .iter()
            .rev()
            .find(|message| message.role == "assistant");
        let Some(assistant) = assistant else {
            operation.status = "noop".into();
            operation.completed_at = Some(now);
            return self.claim_manual_noop(operation, &notifications);
        };
        if conversation
            .messages
            .last()
            .is_some_and(|message| message.id != assistant.id)
            || matches!(assistant.status.as_deref(), Some("pending" | "streaming"))
        {
            return Err("当前聊天尚未形成完整回复，请稍后压缩上下文。"
                .to_string()
                .into());
        }
        operation.assistant_message_id = Some(assistant.id.clone());
        let model_id = conversation
            .model_id
            .as_deref()
            .ok_or_else(|| "当前聊天缺少模型配置。".to_string())?;
        let snapshot = self
            .storage
            .load_model_settings_snapshot_for_model(model_id, true)?
            .ok_or_else(|| "请先配置模型。".to_string())?;
        let target =
            resolve_provider_transition_target(&self.storage, &conversation, &snapshot, model_id)?;
        let (preview, traces) =
            self.persisted_conversation_context_state(&target.generator_input, &conversation.id)?;
        if traces
            .iter()
            .any(|trace| !trace.terminal_status.is_terminal())
        {
            return Err("请等待当前运行结束后再压缩上下文。".to_string().into());
        }
        let boundary = conversation
            .messages
            .last()
            .map(|message| message.id.clone())
            .ok_or_else(|| "当前聊天缺少安全的历史边界。".to_string())?;
        operation.covered_through_message_id = Some(boundary.clone());
        if preview
            .context_compaction_summary
            .as_ref()
            .is_some_and(|summary| {
                summary.covered_through == ContextJournalCursor::message(&boundary)
            })
        {
            operation.status = "noop".into();
            operation.completed_at = Some(now);
            return self.claim_manual_noop(operation, &notifications);
        }
        let prefix = self.storage.prepare_context_compaction_prefix(
            &conversation.id,
            &ContextJournalCursor::message(&boundary),
        )?;
        let source_tokens = mycopilot_core::estimate_provider_transition_compaction_source_tokens(
            &prefix,
            &target.model.provider_model_id,
            target.api_style,
        )
        .map_err(|error| error.to_string())?;
        // Very small histories cannot benefit from another structured continuity summary.
        if source_tokens < 512 {
            operation.status = "noop".into();
            operation.completed_at = Some(now);
            return self.claim_manual_noop(operation, &notifications);
        }
        operation.source_input_tokens = Some(source_tokens);
        let receipt = ContextCompactionReceipt::begin_manual_context_compaction(
            &operation.operation_id,
            &conversation.id,
            &assistant.id,
            &target.model.id,
            &target.model.provider_model_id,
            target.api_style,
            &prefix,
            source_tokens,
            source_tokens.saturating_div(4).max(1),
            now,
        )
        .map_err(|error| error.to_string())?;
        let claimed = self.storage.claim_manual_context_compaction(&operation)?;
        if claimed != operation {
            return Ok(self.project_manual_operation(claimed));
        }
        let cancellation = AgentCancellationToken::new();
        self.manual_context_compaction_cancellations
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .insert(operation.operation_id.clone(), cancellation.clone());
        let output = self.project_manual_operation(operation.clone());
        emit_manual_notification(&notifications, &output);
        let service = self.clone();
        tokio::spawn(async move {
            service
                .run_manual_context_compaction(
                    operation,
                    target,
                    prefix,
                    receipt,
                    cancellation,
                    notifications,
                )
                .await;
        });
        Ok(output)
    }

    fn claim_manual_noop(
        &self,
        mut operation: ManualContextCompactionOperation,
        notifications: &CoreServerNotificationSender,
    ) -> Result<AgentManualContextCompactionOperation, AgentServiceError> {
        operation.status = "running".into();
        operation.completed_at = None;
        let mut claimed = self.storage.claim_manual_context_compaction(&operation)?;
        if claimed.status == "running" {
            claimed.status = "noop".into();
            claimed.completed_at = Some(claimed.updated_at);
            claimed = self.storage.update_manual_context_compaction(&claimed)?;
        }
        let output = self.project_manual_operation(claimed);
        emit_manual_notification(notifications, &output);
        Ok(output)
    }

    pub fn get_manual_context_compaction_status(
        &self,
        input: AgentManualContextCompactionStatusInput,
    ) -> Result<AgentManualContextCompactionStatusOutput, AgentServiceError> {
        validate_identity(&input.conversation_id, "conversationId")?;
        self.authorize_user_conversation_write(&input.conversation_id)?;
        let operations = match input.operation_id {
            Some(operation_id) => {
                validate_identity(&operation_id, "operationId")?;
                self.storage
                    .get_manual_context_compaction(&input.conversation_id, Some(&operation_id))?
                    .into_iter()
                    .collect()
            }
            None => {
                let operations = self
                    .storage
                    .list_manual_context_compactions(&input.conversation_id)?;
                let mut recent = operations.into_iter().rev().take(50).collect::<Vec<_>>();
                recent.reverse();
                recent
            }
        };
        Ok(AgentManualContextCompactionStatusOutput {
            operations: operations
                .into_iter()
                .map(|operation| self.project_manual_operation(operation))
                .collect(),
        })
    }

    pub fn cancel_manual_context_compaction(
        &self,
        input: AgentManualContextCompactionCancelInput,
        notifications: CoreServerNotificationSender,
    ) -> Result<AgentManualContextCompactionOperation, AgentServiceError> {
        validate_identity(&input.conversation_id, "conversationId")?;
        validate_identity(&input.operation_id, "operationId")?;
        self.authorize_user_conversation_write(&input.conversation_id)?;
        let _admission = self
            .conversation_admission
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let mut operation = self
            .storage
            .get_manual_context_compaction(&input.conversation_id, Some(&input.operation_id))?
            .ok_or_else(|| "未找到上下文压缩操作。".to_string())?;
        if operation.status == "running" {
            operation.status = "cancelled".into();
            operation.updated_at = operation.updated_at.max(now_ms());
            operation.completed_at = Some(operation.updated_at);
            operation = self.storage.update_manual_context_compaction(&operation)?;
            if operation.status == "cancelled" {
                let mut cancellations = self
                    .manual_context_compaction_cancellations
                    .lock()
                    .unwrap_or_else(|error| error.into_inner());
                if let Some(token) = cancellations.get(&input.operation_id) {
                    token.cancel();
                }
                // Preparing has not dispatched a provider request. The persisted terminal CAS
                // prevents a worker which has not started from entering generation later.
                if operation.phase == "preparing" {
                    cancellations.remove(&input.operation_id);
                }
            }
        }
        let output = self.project_manual_operation(operation);
        emit_manual_notification(&notifications, &output);
        Ok(output)
    }

    pub(crate) fn ensure_no_manual_context_compaction(
        &self,
        conversation_id: &str,
    ) -> Result<(), String> {
        let mut operations = self
            .storage
            .list_manual_context_compactions(conversation_id)?;
        if let Some(agent) = self
            .storage
            .get_agent_node_by_conversation(conversation_id)
            .map_err(|error| error.to_string())?
        {
            if agent.root_conversation_id != conversation_id {
                operations.extend(
                    self.storage
                        .list_manual_context_compactions(&agent.root_conversation_id)?,
                );
            }
        }
        let cancellations = self
            .manual_context_compaction_cancellations
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if operations.iter().any(|operation| {
            operation.status == "running" || cancellations.contains_key(&operation.operation_id)
        }) {
            return Err("当前聊天正在压缩上下文，请稍后重试。".to_string());
        }
        Ok(())
    }

    pub(crate) fn save_conversation_meta_checked(
        &self,
        conversation: mycopilot_core::storage::models::ChatConversationMetaRecord,
    ) -> Result<mycopilot_core::storage::models::ChatConversationMetaRecord, AgentServiceError>
    {
        self.authorize_user_conversation_write(&conversation.id)?;
        let _admission = self
            .conversation_admission
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let previous = self.storage.load_conversation(&conversation.id)?;
        if previous.as_ref().is_some_and(|previous| {
            previous.archived_at != conversation.archived_at
                || previous.model_id != conversation.model_id
        }) {
            self.ensure_no_manual_context_compaction(&conversation.id)?;
        }
        self.storage
            .save_conversation_meta(conversation)
            .map_err(Into::into)
    }

    #[allow(clippy::too_many_arguments)]
    async fn run_manual_context_compaction(
        &self,
        mut operation: ManualContextCompactionOperation,
        target: ProviderTransitionTarget,
        prefix: ContextCompactionPrefix,
        mut receipt: ContextCompactionReceipt,
        cancellation: AgentCancellationToken,
        notifications: CoreServerNotificationSender,
    ) {
        let result = self
            .execute_manual_context_compaction(
                &mut operation,
                &target,
                &prefix,
                &mut receipt,
                &cancellation,
                &notifications,
            )
            .await;
        if let Err(error) = result {
            operation.status = if cancellation.is_cancelled() {
                "cancelled"
            } else {
                "failed"
            }
            .into();
            operation.summary_id = None;
            operation.replacement_input_tokens = None;
            operation.error =
                (!cancellation.is_cancelled()).then(|| "上下文压缩失败，原上下文仍然有效。".into());
            operation.updated_at = operation.updated_at.max(receipt.updated_at).max(now_ms());
            operation.completed_at = Some(operation.updated_at);
            let _ = receipt.complete_error(
                &error,
                error.model_request_observation(),
                operation.updated_at,
            );
            let _ = self
                .storage
                .record_context_compaction_receipt(&receipt, error.model_request_observation());
            if let Ok(stored) = self.storage.update_manual_context_compaction(&operation) {
                operation = stored;
            }
        }
        self.manual_context_compaction_cancellations
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .remove(&operation.operation_id);
        if let Ok(Some(stored)) = self.storage.get_manual_context_compaction(
            &operation.conversation_id,
            Some(&operation.operation_id),
        ) {
            operation = stored;
        }
        emit_manual_notification(&notifications, &self.project_manual_operation(operation));
        self.schedule_async_human_input_deliveries(notifications.clone());
    }

    async fn execute_manual_context_compaction(
        &self,
        operation: &mut ManualContextCompactionOperation,
        target: &ProviderTransitionTarget,
        prefix: &ContextCompactionPrefix,
        receipt: &mut ContextCompactionReceipt,
        cancellation: &AgentCancellationToken,
        notifications: &CoreServerNotificationSender,
    ) -> Result<(), AgentError> {
        cancellation.check()?;
        self.storage
            .record_context_compaction_receipt(receipt, None)
            .map_err(AgentError::new)?;
        receipt.advance_stage(
            ContextCompactionReceiptStage::Preparing,
            receipt.updated_at.max(now_ms()),
        )?;
        receipt.attach_prepared_prefix(prefix, receipt.updated_at.max(now_ms()))?;
        operation.phase = "generating".into();
        operation.updated_at = operation.updated_at.max(now_ms());
        *operation = self
            .storage
            .update_manual_context_compaction(operation)
            .map_err(AgentError::new)?;
        if operation.status != "running" {
            return Err(AgentError::cancelled());
        }
        self.storage
            .record_context_compaction_receipt(receipt, None)
            .map_err(AgentError::new)?;
        emit_manual_notification(
            notifications,
            &self.project_manual_operation(operation.clone()),
        );
        let request = AgentContextCompactionGenerationRequest {
            operation_id: operation.operation_id.clone(),
            run_id: receipt.run_id.clone(),
            conversation_id: operation.conversation_id.clone(),
            assistant_message_id: receipt.assistant_message_id.clone(),
            request_index: 1,
            prefix: Arc::new(prefix.clone()),
            continuity: ContextContinuitySnapshot::from_prefix(prefix)?,
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
        let generated = match generator(request, cancellation.clone()).await {
            Ok(generated) => generated,
            Err(error) => {
                if let Some(observation) = error.model_request_observation() {
                    self.persist_manual_usage(operation, target, observation)?;
                }
                return Err(error);
            }
        };
        // Persist an observed paid request before checking cancellation or applying the summary.
        // A response which loses the commit race still owns its usage exactly once.
        self.persist_manual_usage(operation, target, &generated.observation)?;
        cancellation.check()?;
        operation.phase = "committing".into();
        operation.updated_at = operation.updated_at.max(now_ms());
        *operation = self
            .storage
            .update_manual_context_compaction(operation)
            .map_err(AgentError::new)?;
        if operation.status != "running" {
            return Err(AgentError::cancelled());
        }
        receipt.advance_stage(
            ContextCompactionReceiptStage::Committing,
            receipt.updated_at.max(now_ms()),
        )?;
        self.storage
            .record_context_compaction_receipt(receipt, None)
            .map_err(AgentError::new)?;
        let mut applied = receipt.clone();
        applied.complete_applied(
            &generated.draft,
            &generated.observation,
            applied.updated_at.max(now_ms()),
        )?;
        operation.status = "completed".into();
        operation.summary_id = applied.summary_id.clone();
        operation.replacement_input_tokens = Some(generated.draft.replacement_input_tokens);
        operation.updated_at = operation.updated_at.max(applied.updated_at).max(now_ms());
        operation.completed_at = Some(operation.updated_at);
        let committed = self
            .storage
            .commit_manual_context_compaction_if_current(
                prefix,
                generated.draft,
                &applied,
                &generated.observation,
                operation,
                &target.model.id,
                &target.protocol_revision,
            )
            .map_err(AgentError::new)?;
        if committed.is_none() {
            return Err(AgentError::new("上下文压缩提交边界已经变化。"));
        }
        *receipt = applied;
        self.invalidate_conversation_context_state(&operation.conversation_id);
        Ok(())
    }

    fn persist_manual_usage(
        &self,
        operation: &ManualContextCompactionOperation,
        target: &ProviderTransitionTarget,
        observation: &ModelRequestObservation,
    ) -> Result<(), AgentError> {
        let usage = observation.actual_usage.as_ref().map(|usage| &usage.raw);
        let semantics = mycopilot_core::resolve_provider_runtime_capabilities(&target.protocol)
            .map_err(|error| AgentError::new(error.to_string()))?
            .usage();
        let input_tokens = usage.and_then(|u| u.input_tokens);
        let cached_input_tokens = usage.and_then(|u| u.cached_input_tokens);
        let billable_output_tokens = usage.and_then(|u| semantics.billable_output_tokens(u));
        let record = ManualContextCompactionUsageRecord {
            operation_id: operation.operation_id.clone(),
            conversation_id: operation.conversation_id.clone(),
            project_id: target
                .generator_input
                .context
                .as_ref()
                .and_then(|context| context.project_id.clone()),
            model_id: target.model.id.clone(),
            model_name: target.model.display_label(),
            started_at: Some(observation.started_at),
            completed_at: Some(observation.completed_at),
            status: Some("observed".into()),
            error: None,
            created_at: observation.completed_at,
            input_tokens,
            cached_input_tokens,
            output_tokens: usage.and_then(|u| u.output_tokens),
            output_thinking_tokens: usage.and_then(|u| u.output_thinking_tokens),
            total_tokens: usage.and_then(|u| u.total_tokens),
            cache_creation_input_tokens: usage.and_then(|u| u.cache_creation_input_tokens),
            billable_request_count: usage.and_then(|u| u.billable_request_count).unwrap_or(1),
            input_price: Some(target.model.input_price.clone()),
            cached_input_price: Some(target.model.effective_cached_input_price().to_string()),
            output_price: Some(target.model.output_price.clone()),
            estimated_cost: self.storage.estimate_usage_cost(
                input_tokens,
                cached_input_tokens,
                billable_output_tokens,
                &target.model.input_price,
                target.model.effective_cached_input_price(),
                &target.model.output_price,
            ),
        };
        self.storage
            .record_manual_context_compaction_observation_and_usage(observation, &record)
            .map_err(AgentError::new)
    }
}

fn validate_identity(value: &str, name: &str) -> Result<(), AgentServiceError> {
    if value.trim().is_empty() || value.trim() != value || value.len() > 512 {
        return Err(format!("{name} 无效。").into());
    }
    Ok(())
}

fn emit_manual_notification(
    notifications: &CoreServerNotificationSender,
    operation: &AgentManualContextCompactionOperation,
) {
    let _ = notifications.send(serde_json::json!({"jsonrpc":"2.0", "method": "agent.manualContextCompaction", "params":operation}));
}
