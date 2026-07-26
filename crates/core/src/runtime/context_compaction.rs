//! Blocking context-compaction orchestration owned by the agent runtime.
//!
//! Runtime extensions may contribute context to a compaction request, but they never own loop
//! control. This executor is invoked between capacity planning and the normal provider request.
//! Durable storage remains a host concern and summary generation is injected independently, so a
//! deterministic test generator can later be replaced by a real model request without changing
//! the prepare/commit protocol.

use crate::cancellation::AgentCancellationToken;
use crate::context::{
    AgentContextBaseline, ContextCompactionPlan, ContextCompactionPlanStatus,
    ContextCompactionPrefix, ContextCompactionSummaryDraft, ContextJournalCursor,
};
use crate::protocol::{AgentApiStyle, AgentError, AgentResult, AgentUsage};
use crate::{ContextCompactionReceipt, ContextCompactionReceiptStage, ModelRequestObservation};
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

type CompactionFuture<T> = Pin<Box<dyn Future<Output = AgentResult<T>> + Send + 'static>>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentContextCompactionPrepareRequest {
    pub run_id: String,
    pub conversation_id: String,
    pub assistant_message_id: String,
    pub expected_previous_summary_id: Option<String>,
    pub covered_through: ContextJournalCursor,
    /// Current-run trace prefix already observed by the main model. Later persisted entries remain
    /// in the uncommitted overlay and cannot be promoted by a compaction rebuild.
    pub visible_trace_item_count: usize,
    pub source_input_tokens: u64,
    pub uncovered_tail_input_tokens: u64,
    pub target_replacement_tokens: u64,
}

pub enum AgentContextCompactionPrepareOutcome {
    Ready(Arc<ContextCompactionPrefix>),
    /// The plan was based on an older journal head. The supplied baseline is authoritative and
    /// must replace the runtime's persistent context before planning again.
    Refresh(Box<AgentContextBaseline>),
}

#[derive(Clone)]
pub struct AgentContextCompactionGenerationRequest {
    pub operation_id: String,
    pub run_id: String,
    pub conversation_id: String,
    pub assistant_message_id: String,
    pub request_index: u64,
    pub prefix: Arc<ContextCompactionPrefix>,
    pub continuity: crate::ContextContinuitySnapshot,
    pub source_input_tokens: u64,
    pub uncovered_tail_input_tokens: u64,
    pub target_replacement_tokens: u64,
}

#[derive(Debug, Clone)]
pub struct AgentContextCompactionGenerationOutput {
    pub draft: ContextCompactionSummaryDraft,
    pub observation: ModelRequestObservation,
}

#[derive(Clone)]
pub struct AgentContextCompactionCommitRequest {
    pub run_id: String,
    pub conversation_id: String,
    pub assistant_message_id: String,
    pub visible_trace_item_count: usize,
    pub prefix: Arc<ContextCompactionPrefix>,
    pub draft: ContextCompactionSummaryDraft,
    pub receipt: ContextCompactionReceipt,
    pub observation: ModelRequestObservation,
}

pub enum AgentContextCompactionCommitOutcome {
    Applied {
        summary_id: String,
        baseline: Box<AgentContextBaseline>,
    },
    /// The durable source changed after generation. The summary was not committed.
    Refresh(Box<AgentContextBaseline>),
}

type PrepareCallback = Arc<
    dyn Fn(
            AgentContextCompactionPrepareRequest,
            AgentCancellationToken,
        ) -> CompactionFuture<AgentContextCompactionPrepareOutcome>
        + Send
        + Sync,
>;
type GenerateCallback = Arc<
    dyn Fn(
            AgentContextCompactionGenerationRequest,
            AgentCancellationToken,
        ) -> CompactionFuture<AgentContextCompactionGenerationOutput>
        + Send
        + Sync,
>;
type CommitCallback = Arc<
    dyn Fn(
            AgentContextCompactionCommitRequest,
            AgentCancellationToken,
        ) -> CompactionFuture<AgentContextCompactionCommitOutcome>
        + Send
        + Sync,
>;
type ReceiptCallback = Arc<
    dyn Fn(ContextCompactionReceipt, Option<ModelRequestObservation>) -> CompactionFuture<()>
        + Send
        + Sync,
>;

/// Dependencies required by the runtime-owned executor.
///
/// `prepare` and `commit` are supplied by the durable-state owner. `generate` is deliberately
/// independent so tests and future model-backed generation share exactly the same orchestration.
#[derive(Clone)]
pub struct AgentContextCompactionServices {
    prepare: PrepareCallback,
    generate: GenerateCallback,
    commit: CommitCallback,
    record_receipt: ReceiptCallback,
}

impl AgentContextCompactionServices {
    pub fn new<P, PFut, G, GFut, C, CFut, R, RFut>(
        prepare: P,
        generate: G,
        commit: C,
        record_receipt: R,
    ) -> Self
    where
        P: Fn(AgentContextCompactionPrepareRequest, AgentCancellationToken) -> PFut
            + Send
            + Sync
            + 'static,
        PFut: Future<Output = AgentResult<AgentContextCompactionPrepareOutcome>> + Send + 'static,
        G: Fn(AgentContextCompactionGenerationRequest, AgentCancellationToken) -> GFut
            + Send
            + Sync
            + 'static,
        GFut: Future<Output = AgentResult<AgentContextCompactionGenerationOutput>> + Send + 'static,
        C: Fn(AgentContextCompactionCommitRequest, AgentCancellationToken) -> CFut
            + Send
            + Sync
            + 'static,
        CFut: Future<Output = AgentResult<AgentContextCompactionCommitOutcome>> + Send + 'static,
        R: Fn(ContextCompactionReceipt, Option<ModelRequestObservation>) -> RFut
            + Send
            + Sync
            + 'static,
        RFut: Future<Output = AgentResult<()>> + Send + 'static,
    {
        Self {
            prepare: Arc::new(move |request, cancellation| {
                Box::pin(prepare(request, cancellation))
            }),
            generate: Arc::new(move |request, cancellation| {
                Box::pin(generate(request, cancellation))
            }),
            commit: Arc::new(move |request, cancellation| Box::pin(commit(request, cancellation))),
            record_receipt: Arc::new(move |receipt, observation| {
                Box::pin(record_receipt(receipt, observation))
            }),
        }
    }

    pub async fn prepare(
        &self,
        request: AgentContextCompactionPrepareRequest,
        cancellation: AgentCancellationToken,
    ) -> AgentResult<AgentContextCompactionPrepareOutcome> {
        (self.prepare)(request, cancellation).await
    }

    pub async fn generate(
        &self,
        request: AgentContextCompactionGenerationRequest,
        cancellation: AgentCancellationToken,
    ) -> AgentResult<AgentContextCompactionGenerationOutput> {
        (self.generate)(request, cancellation).await
    }

    pub async fn commit(
        &self,
        request: AgentContextCompactionCommitRequest,
        cancellation: AgentCancellationToken,
    ) -> AgentResult<AgentContextCompactionCommitOutcome> {
        (self.commit)(request, cancellation).await
    }

    pub async fn record_receipt(
        &self,
        receipt: ContextCompactionReceipt,
        observation: Option<ModelRequestObservation>,
    ) -> AgentResult<()> {
        (self.record_receipt)(receipt, observation).await
    }
}

#[derive(Debug)]
pub(super) enum ContextCompactionExecution {
    Applied {
        baseline: Box<AgentContextBaseline>,
        usage: Option<AgentUsage>,
    },
    Refreshed {
        baseline: Box<AgentContextBaseline>,
        usage: Option<AgentUsage>,
    },
}

pub(super) struct ContextCompactionAttempt {
    request: AgentContextCompactionPrepareRequest,
    receipt: ContextCompactionReceipt,
}

pub(super) struct ContextCompactionExecutor {
    services: AgentContextCompactionServices,
}

impl ContextCompactionExecutor {
    pub(super) fn new(services: AgentContextCompactionServices) -> Self {
        Self { services }
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) async fn begin(
        &self,
        plan: &ContextCompactionPlan,
        operation_id: &str,
        run_id: &str,
        conversation_id: Option<&str>,
        assistant_message_id: Option<&str>,
        request_index: u64,
        attempt_index: u64,
        model: &str,
        api_style: AgentApiStyle,
        visible_trace_item_count: usize,
        cancellation_token: &AgentCancellationToken,
    ) -> AgentResult<Option<ContextCompactionAttempt>> {
        cancellation_token.check()?;
        let Some(request) = prepare_request_from_plan(
            plan,
            run_id,
            conversation_id,
            assistant_message_id,
            visible_trace_item_count,
        ) else {
            return Ok(None);
        };
        let receipt = ContextCompactionReceipt::begin(
            operation_id,
            run_id,
            &request.conversation_id,
            &request.assistant_message_id,
            request_index,
            attempt_index,
            model,
            api_style,
            plan,
            crate::storage::now_ms(),
        )?;
        self.services.record_receipt(receipt.clone(), None).await?;
        Ok(Some(ContextCompactionAttempt { request, receipt }))
    }

    pub(super) async fn execute(
        &self,
        mut attempt: ContextCompactionAttempt,
        cancellation_token: &AgentCancellationToken,
    ) -> AgentResult<ContextCompactionExecution> {
        if let Err(error) = cancellation_token.check() {
            self.finalize_error(&mut attempt.receipt, &error, None)
                .await?;
            return Err(error);
        }
        attempt.receipt.advance_stage(
            ContextCompactionReceiptStage::Preparing,
            crate::storage::now_ms(),
        )?;
        self.services
            .record_receipt(attempt.receipt.clone(), None)
            .await?;

        let prepared = match cancellable(
            cancellation_token,
            (self.services.prepare)(attempt.request.clone(), cancellation_token.clone()),
        )
        .await
        {
            Ok(prepared) => prepared,
            Err(error) => {
                self.finalize_error(&mut attempt.receipt, &error, None)
                    .await?;
                return Err(error);
            }
        };
        let prefix = match prepared {
            AgentContextCompactionPrepareOutcome::Ready(prefix) => prefix,
            AgentContextCompactionPrepareOutcome::Refresh(baseline) => {
                attempt
                    .receipt
                    .complete_refreshed(None, crate::storage::now_ms())?;
                self.services.record_receipt(attempt.receipt, None).await?;
                return Ok(ContextCompactionExecution::Refreshed {
                    baseline,
                    usage: None,
                });
            }
        };
        if let Err(error) = validate_prepared_prefix(&attempt.request, &prefix) {
            self.finalize_error(&mut attempt.receipt, &error, None)
                .await?;
            return Err(error);
        }
        if let Err(error) = attempt
            .receipt
            .attach_prepared_prefix(&prefix, crate::storage::now_ms())
        {
            self.finalize_error(&mut attempt.receipt, &error, None)
                .await?;
            return Err(error);
        }
        self.services
            .record_receipt(attempt.receipt.clone(), None)
            .await?;

        let continuity = match crate::ContextContinuitySnapshot::from_prefix(&prefix) {
            Ok(continuity) => continuity,
            Err(error) => {
                self.finalize_error(&mut attempt.receipt, &error, None)
                    .await?;
                return Err(error);
            }
        };
        let generation_request = AgentContextCompactionGenerationRequest {
            operation_id: attempt.receipt.operation_id.clone(),
            run_id: attempt.request.run_id.clone(),
            conversation_id: attempt.request.conversation_id.clone(),
            assistant_message_id: attempt.request.assistant_message_id.clone(),
            request_index: attempt.receipt.request_index,
            prefix: prefix.clone(),
            continuity: continuity.clone(),
            source_input_tokens: attempt.request.source_input_tokens,
            uncovered_tail_input_tokens: attempt.request.uncovered_tail_input_tokens,
            target_replacement_tokens: attempt.request.target_replacement_tokens,
        };
        let generated = match cancellable(
            cancellation_token,
            (self.services.generate)(generation_request, cancellation_token.clone()),
        )
        .await
        {
            Ok(generated) => generated,
            Err(error) => {
                let observation = error.model_request_observation().cloned();
                self.finalize_error(&mut attempt.receipt, &error, observation.as_ref())
                    .await?;
                return Err(error);
            }
        };
        let generated_usage = generated
            .observation
            .actual_usage
            .as_ref()
            .map(|usage| usage.raw.clone());
        let draft = generated.draft;
        if let Err(error) = validate_generated_draft(&attempt.request, &prefix, &continuity, &draft)
        {
            let error = error
                .with_usage(generated_usage.clone())
                .with_model_request_observation(generated.observation.clone());
            self.finalize_error(&mut attempt.receipt, &error, Some(&generated.observation))
                .await?;
            return Err(error);
        }

        if let Err(error) = attempt.receipt.advance_stage(
            ContextCompactionReceiptStage::Committing,
            crate::storage::now_ms(),
        ) {
            let error = error
                .with_usage(generated_usage.clone())
                .with_model_request_observation(generated.observation.clone());
            self.finalize_error(&mut attempt.receipt, &error, Some(&generated.observation))
                .await?;
            return Err(error);
        }
        self.services
            .record_receipt(attempt.receipt.clone(), None)
            .await?;

        let expected_summary_id = draft.id.clone();
        let mut applied_receipt = attempt.receipt.clone();
        if let Err(error) = applied_receipt.complete_applied(
            &draft,
            &generated.observation,
            crate::storage::now_ms(),
        ) {
            let error = error
                .with_usage(generated_usage.clone())
                .with_model_request_observation(generated.observation.clone());
            self.finalize_error(&mut attempt.receipt, &error, Some(&generated.observation))
                .await?;
            return Err(error);
        }
        let committed = match cancellable(
            cancellation_token,
            (self.services.commit)(
                AgentContextCompactionCommitRequest {
                    run_id: attempt.request.run_id,
                    conversation_id: attempt.request.conversation_id,
                    assistant_message_id: attempt.request.assistant_message_id,
                    visible_trace_item_count: attempt.request.visible_trace_item_count,
                    prefix,
                    draft,
                    receipt: applied_receipt,
                    observation: generated.observation.clone(),
                },
                cancellation_token.clone(),
            ),
        )
        .await
        {
            Ok(committed) => committed,
            Err(error) if error.code() == Some("context_compaction_applied_rebuild_failed") => {
                return Err(error
                    .with_usage(generated_usage)
                    .with_model_request_observation(generated.observation));
            }
            Err(error) => {
                let error = error
                    .with_usage(generated_usage.clone())
                    .with_model_request_observation(generated.observation.clone());
                self.finalize_error(&mut attempt.receipt, &error, Some(&generated.observation))
                    .await?;
                return Err(error);
            }
        };
        match committed {
            AgentContextCompactionCommitOutcome::Applied {
                summary_id,
                baseline,
            } => {
                if summary_id != expected_summary_id {
                    return Err(contract_error("宿主返回的摘要 ID 与已提交草稿不一致。")
                        .with_usage(generated_usage)
                        .with_model_request_observation(generated.observation));
                }
                Ok(ContextCompactionExecution::Applied {
                    baseline,
                    usage: generated_usage,
                })
            }
            AgentContextCompactionCommitOutcome::Refresh(baseline) => {
                attempt
                    .receipt
                    .complete_refreshed(Some(&generated.observation), crate::storage::now_ms())?;
                self.services
                    .record_receipt(attempt.receipt, Some(generated.observation.clone()))
                    .await?;
                Ok(ContextCompactionExecution::Refreshed {
                    baseline,
                    usage: generated_usage,
                })
            }
        }
    }

    async fn finalize_error(
        &self,
        receipt: &mut ContextCompactionReceipt,
        error: &AgentError,
        observation: Option<&ModelRequestObservation>,
    ) -> AgentResult<()> {
        let completed_at = crate::storage::now_ms();
        if let Err(observation_error) = receipt.complete_error(error, observation, completed_at) {
            if observation.is_none() {
                return Err(observation_error);
            }
            // A malformed provider observation must not prevent the operation itself from
            // reaching a durable failed terminal state. The unrelated observation is omitted.
            receipt.complete_error(error, None, completed_at)?;
            return self.services.record_receipt(receipt.clone(), None).await;
        }
        self.services
            .record_receipt(receipt.clone(), observation.cloned())
            .await
    }
}

fn prepare_request_from_plan(
    plan: &ContextCompactionPlan,
    run_id: &str,
    conversation_id: Option<&str>,
    assistant_message_id: Option<&str>,
    visible_trace_item_count: usize,
) -> Option<AgentContextCompactionPrepareRequest> {
    if plan.status != ContextCompactionPlanStatus::Required {
        return None;
    }
    let step = plan.steps.first()?;
    let prefix = step.durable_prefix.as_ref()?;
    let conversation_id = conversation_id?.trim();
    let assistant_message_id = assistant_message_id?.trim();
    if conversation_id.is_empty() || assistant_message_id.is_empty() {
        return None;
    }
    Some(AgentContextCompactionPrepareRequest {
        run_id: run_id.to_string(),
        conversation_id: conversation_id.to_string(),
        assistant_message_id: assistant_message_id.to_string(),
        expected_previous_summary_id: prefix.previous_summary_id.clone(),
        covered_through: prefix.covered_through.clone(),
        visible_trace_item_count,
        source_input_tokens: step.source_input_tokens,
        uncovered_tail_input_tokens: plan
            .projected_durable_input_tokens
            .saturating_add(plan.planned_reclaimed_tokens)
            .saturating_sub(step.source_input_tokens),
        target_replacement_tokens: step.target_replacement_tokens,
    })
}

fn validate_prepared_prefix(
    request: &AgentContextCompactionPrepareRequest,
    prefix: &ContextCompactionPrefix,
) -> AgentResult<()> {
    prefix.validate()?;
    let previous_summary_id = prefix
        .previous_summary
        .as_ref()
        .map(|summary| summary.id.as_str());
    if previous_summary_id != request.expected_previous_summary_id.as_deref()
        || prefix.covered_through != request.covered_through
    {
        return Err(contract_error(
            "宿主准备的 durable 前缀与压缩计划身份不一致。",
        ));
    }
    Ok(())
}

fn validate_generated_draft(
    request: &AgentContextCompactionPrepareRequest,
    prefix: &ContextCompactionPrefix,
    expected_continuity: &crate::ContextContinuitySnapshot,
    draft: &ContextCompactionSummaryDraft,
) -> AgentResult<()> {
    if draft.replacement_input_tokens >= draft.source_input_tokens {
        return Err(AgentError::structured(
            "context_compaction_replacement_invalid",
            "上下文压缩替换内容没有实际缩小上下文。",
            serde_json::json!({
                "replacementInputTokens": draft.replacement_input_tokens,
                "sourceInputTokens": draft.source_input_tokens,
                "targetReplacementTokens": request.target_replacement_tokens,
            }),
        ));
    }
    draft.validate()?;
    if draft.source_revision != prefix.source_revision
        || draft.source_input_tokens != request.source_input_tokens
        || draft.uncovered_tail_input_tokens != request.uncovered_tail_input_tokens
    {
        return Err(contract_error(
            "摘要生成结果没有绑定当前 durable 前缀及其计量。",
        ));
    }
    if draft.continuity != *expected_continuity
        || draft.continuity.covered_through != prefix.covered_through
    {
        return Err(AgentError::structured(
            "context_compaction_replacement_invalid",
            "上下文压缩生成器返回的连续性骨架与后端确定性骨架不一致。",
            serde_json::json!({
                "replacementInputTokens": draft.replacement_input_tokens,
                "sourceInputTokens": draft.source_input_tokens,
                "targetReplacementTokens": request.target_replacement_tokens,
            }),
        ));
    }
    Ok(())
}

async fn cancellable<T>(
    cancellation_token: &AgentCancellationToken,
    future: CompactionFuture<T>,
) -> AgentResult<T> {
    tokio::select! {
        _ = cancellation_token.cancelled() => Err(AgentError::cancelled()),
        result = future => result,
    }
}

fn contract_error(message: &str) -> AgentError {
    AgentError::structured(
        "context_compaction_contract_violation",
        message,
        serde_json::json!({}),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::{
        AgentConversationContextState, ContextCapacityDetector, ContextCompactionDurablePrefix,
        ContextCompactionProtectedEstimate, ContextCompactionStep, ContextFrame, ContextItem,
        ContextRetention, ContextScope, ContextSource,
    };
    use crate::protocol::AgentApiStyle;
    use std::collections::BTreeMap;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Duration;

    fn plan() -> ContextCompactionPlan {
        ContextCompactionPlan {
            status: ContextCompactionPlanStatus::Required,
            context_revision: 1,
            persistent_revision: 1,
            request_input_tokens: 9_000,
            available_input_tokens: Some(10_000),
            soft_trigger_input_tokens: Some(9_000),
            target_input_tokens: Some(7_500),
            durable_capacity_tokens: Some(9_000),
            durable_trigger_input_tokens: Some(8_100),
            durable_target_input_tokens: Some(1_350),
            required_reclaimed_tokens: 7_000,
            required_durable_reclaimed_tokens: 7_000,
            planned_reclaimed_tokens: 744,
            projected_request_input_tokens: 8_256,
            projected_durable_input_tokens: 8_256,
            request_target_satisfied: false,
            durable_target_satisfied: false,
            best_effort: true,
            compactable_input_tokens: 1_000,
            protected: ContextCompactionProtectedEstimate {
                input_tokens: 8_000,
                atomic_unit_count: 1,
                reasons: BTreeMap::new(),
            },
            steps: vec![ContextCompactionStep {
                ranges: Vec::new(),
                atomic_unit_count: 2,
                source_input_tokens: 1_000,
                target_replacement_tokens: 256,
                expected_reclaimed_tokens: 744,
                contains_side_effects: false,
                contains_errors: false,
                durable_prefix: Some(ContextCompactionDurablePrefix {
                    previous_summary_id: None,
                    covered_through: ContextJournalCursor::message("assistant-old"),
                }),
            }],
        }
    }

    fn prefix() -> ContextCompactionPrefix {
        ContextCompactionPrefix {
            conversation_id: "conversation-1".to_string(),
            source_revision: "source-revision-1".to_string(),
            covered_through: ContextJournalCursor::message("assistant-old"),
            previous_summary: None,
            source_items: vec![
                crate::ContextCompactionSourceItem::Message {
                    cursor: ContextJournalCursor::message("user-old"),
                    role: "user".to_string(),
                    content: "old request".to_string(),
                    created_at: 1,
                    status: Some("sent".to_string()),
                    terminal_status: None,
                    terminal_error: None,
                },
                crate::ContextCompactionSourceItem::Message {
                    cursor: ContextJournalCursor::message("assistant-old"),
                    role: "assistant".to_string(),
                    content: "old answer".to_string(),
                    created_at: 2,
                    status: Some("sent".to_string()),
                    terminal_status: None,
                    terminal_error: None,
                },
            ],
        }
    }

    fn baseline() -> AgentContextBaseline {
        let frame = ContextFrame::new(vec![
            ContextItem::text(
                crate::llm::LlmMessageRole::System,
                "rules",
                ContextSource::BackendSystemPrompt,
                ContextScope::Run,
                ContextRetention::Retained,
            ),
            ContextItem::text(
                crate::llm::LlmMessageRole::User,
                "current request",
                ContextSource::ConversationHistory,
                ContextScope::Conversation,
                ContextRetention::Retained,
            ),
        ]);
        let detector =
            ContextCapacityDetector::for_model("test-model", AgentApiStyle::OpenAiCompatible, &[]);
        let mut state = AgentConversationContextState::new(
            "configuration-1".to_string(),
            "test-model".to_string(),
            Some(10_000),
            1_000,
            detector,
            frame,
            crate::context::ConversationTimingTracker::default(),
        );
        state.shared_baseline().unwrap()
    }

    fn generation_observation(
        request: &AgentContextCompactionGenerationRequest,
    ) -> ModelRequestObservation {
        crate::model_request_observation::ModelRequestObservationBuilder::new(
            format!("model-request-{}", request.operation_id),
            request.run_id.clone(),
            Some(request.conversation_id.clone()),
            Some(request.assistant_message_id.clone()),
            Some(request.operation_id.clone()),
            request.request_index,
            crate::ModelRequestPurpose::ContextCompaction,
            "test-model",
            AgentApiStyle::OpenAiCompatible,
            None,
            1,
        )
        .completed(None, Some("stop".to_string()), 2)
        .unwrap()
    }

    async fn begin_attempt(
        executor: &ContextCompactionExecutor,
        cancellation: &AgentCancellationToken,
    ) -> ContextCompactionAttempt {
        executor
            .begin(
                &plan(),
                "operation-1",
                "run-1",
                Some("conversation-1"),
                Some("assistant-current"),
                1,
                1,
                "test-model",
                AgentApiStyle::OpenAiCompatible,
                0,
                cancellation,
            )
            .await
            .unwrap()
            .unwrap()
    }

    #[tokio::test]
    async fn cancellation_interrupts_a_blocked_summary_generator_before_commit() {
        let services = AgentContextCompactionServices::new(
            |_, _| async {
                Ok(AgentContextCompactionPrepareOutcome::Ready(Arc::new(
                    prefix(),
                )))
            },
            |_, _| async {
                std::future::pending::<()>().await;
                unreachable!()
            },
            |_, _| async { panic!("cancelled generation must not commit") },
            |_, _| async { Ok(()) },
        );
        let executor = ContextCompactionExecutor::new(services);
        let cancellation = AgentCancellationToken::new();
        let attempt = begin_attempt(&executor, &cancellation).await;
        let cancel = cancellation.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(10)).await;
            cancel.cancel();
        });

        let error = executor.execute(attempt, &cancellation).await.unwrap_err();

        assert!(error.is_cancelled());
    }

    #[tokio::test]
    async fn stale_prepare_rebases_without_generating_or_committing() {
        let generated = Arc::new(AtomicBool::new(false));
        let committed = Arc::new(AtomicBool::new(false));
        let generated_for_callback = generated.clone();
        let committed_for_callback = committed.clone();
        let refreshed = baseline();
        let services = AgentContextCompactionServices::new(
            move |_, _| {
                let refreshed = refreshed.clone();
                async move {
                    Ok(AgentContextCompactionPrepareOutcome::Refresh(Box::new(
                        refreshed,
                    )))
                }
            },
            move |_, _| {
                generated_for_callback.store(true, Ordering::SeqCst);
                async { panic!("stale preparation must not generate") }
            },
            move |_, _| {
                committed_for_callback.store(true, Ordering::SeqCst);
                async { panic!("stale preparation must not commit") }
            },
            |_, _| async { Ok(()) },
        );
        let executor = ContextCompactionExecutor::new(services);
        let cancellation = AgentCancellationToken::new();
        let attempt = begin_attempt(&executor, &cancellation).await;
        let execution = executor.execute(attempt, &cancellation).await.unwrap();

        assert!(matches!(
            execution,
            ContextCompactionExecution::Refreshed { usage: None, .. }
        ));
        assert!(!generated.load(Ordering::SeqCst));
        assert!(!committed.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn non_shrinking_generated_replacement_is_rejected_before_commit() {
        let committed = Arc::new(AtomicBool::new(false));
        let committed_for_callback = committed.clone();
        let services = AgentContextCompactionServices::new(
            |_, _| async {
                Ok(AgentContextCompactionPrepareOutcome::Ready(Arc::new(
                    prefix(),
                )))
            },
            |request, _| async move {
                let observation = generation_observation(&request);
                Ok(AgentContextCompactionGenerationOutput {
                    draft: ContextCompactionSummaryDraft {
                        id: "summary-too-large".to_string(),
                        source_revision: request.prefix.source_revision.clone(),
                        content: "oversized summary".to_string(),
                        continuity: request.continuity,
                        generation: crate::ContextCompactionGeneration::test(),
                        source_input_tokens: request.source_input_tokens,
                        summary_input_tokens: 10,
                        continuity_input_tokens: 20,
                        uncovered_tail_input_tokens: request.uncovered_tail_input_tokens,
                        replacement_input_tokens: request.source_input_tokens,
                        created_at: 1,
                    },
                    observation,
                })
            },
            move |_, _| {
                committed_for_callback.store(true, Ordering::SeqCst);
                async { panic!("oversized summary must not commit") }
            },
            |_, _| async { Ok(()) },
        );
        let executor = ContextCompactionExecutor::new(services);
        let cancellation = AgentCancellationToken::new();
        let attempt = begin_attempt(&executor, &cancellation).await;
        let error = executor.execute(attempt, &cancellation).await.unwrap_err();

        assert_eq!(error.code(), Some("context_compaction_replacement_invalid"));
        assert!(!committed.load(Ordering::SeqCst));
    }
}
