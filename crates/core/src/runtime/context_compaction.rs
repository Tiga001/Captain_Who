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
    ContextCompactionPrefix, ContextCompactionScope, ContextCompactionSummaryDraft,
};
use crate::protocol::{AgentError, AgentResult, AgentUsage};
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
    pub newly_covered_message_ids: Vec<String>,
    pub covered_through_message_id: String,
    pub source_input_tokens: u64,
    pub maximum_summary_tokens: u64,
}

pub enum AgentContextCompactionPrepareOutcome {
    Ready(Arc<ContextCompactionPrefix>),
    /// The plan was based on an older durable projection. The supplied baseline is authoritative
    /// and must replace the runtime's persistent context before planning again.
    Refresh(Box<AgentContextBaseline>),
}

#[derive(Clone)]
pub struct AgentContextCompactionGenerationRequest {
    pub prefix: Arc<ContextCompactionPrefix>,
    pub source_input_tokens: u64,
    pub maximum_summary_tokens: u64,
}

#[derive(Debug, Clone)]
pub struct AgentContextCompactionGenerationOutput {
    pub draft: ContextCompactionSummaryDraft,
    pub usage: Option<AgentUsage>,
}

#[derive(Clone)]
pub struct AgentContextCompactionCommitRequest {
    pub run_id: String,
    pub conversation_id: String,
    pub assistant_message_id: String,
    pub prefix: Arc<ContextCompactionPrefix>,
    pub draft: ContextCompactionSummaryDraft,
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

/// Dependencies required by the runtime-owned executor.
///
/// `prepare` and `commit` are supplied by the durable-state owner. `generate` is deliberately
/// independent so tests and future model-backed generation share exactly the same orchestration.
#[derive(Clone)]
pub struct AgentContextCompactionServices {
    prepare: PrepareCallback,
    generate: GenerateCallback,
    commit: CommitCallback,
}

impl AgentContextCompactionServices {
    pub fn new<P, PFut, G, GFut, C, CFut>(prepare: P, generate: G, commit: C) -> Self
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
    {
        Self {
            prepare: Arc::new(move |request, cancellation| {
                Box::pin(prepare(request, cancellation))
            }),
            generate: Arc::new(move |request, cancellation| {
                Box::pin(generate(request, cancellation))
            }),
            commit: Arc::new(move |request, cancellation| Box::pin(commit(request, cancellation))),
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
}

#[derive(Debug)]
pub(super) enum ContextCompactionExecution {
    NotApplicable,
    Rebase {
        baseline: Box<AgentContextBaseline>,
        usage: Option<AgentUsage>,
    },
}

pub(super) struct ContextCompactionExecutor {
    services: AgentContextCompactionServices,
}

impl ContextCompactionExecutor {
    pub(super) fn new(services: AgentContextCompactionServices) -> Self {
        Self { services }
    }

    pub(super) fn is_applicable(
        &self,
        plan: &ContextCompactionPlan,
        run_id: &str,
        conversation_id: Option<&str>,
        assistant_message_id: Option<&str>,
    ) -> bool {
        prepare_request_from_plan(plan, run_id, conversation_id, assistant_message_id).is_some()
    }

    pub(super) async fn execute(
        &self,
        plan: &ContextCompactionPlan,
        run_id: &str,
        conversation_id: Option<&str>,
        assistant_message_id: Option<&str>,
        cancellation_token: &AgentCancellationToken,
    ) -> AgentResult<ContextCompactionExecution> {
        let Some(request) =
            prepare_request_from_plan(plan, run_id, conversation_id, assistant_message_id)
        else {
            return Ok(ContextCompactionExecution::NotApplicable);
        };
        cancellation_token.check()?;

        let prepared = cancellable(
            cancellation_token,
            (self.services.prepare)(request.clone(), cancellation_token.clone()),
        )
        .await?;
        let prefix = match prepared {
            AgentContextCompactionPrepareOutcome::Ready(prefix) => prefix,
            AgentContextCompactionPrepareOutcome::Refresh(baseline) => {
                return Ok(ContextCompactionExecution::Rebase {
                    baseline,
                    usage: None,
                });
            }
        };
        validate_prepared_prefix(&request, &prefix)?;

        let generation_request = AgentContextCompactionGenerationRequest {
            prefix: prefix.clone(),
            source_input_tokens: request.source_input_tokens,
            maximum_summary_tokens: request.maximum_summary_tokens,
        };
        let generated = cancellable(
            cancellation_token,
            (self.services.generate)(generation_request, cancellation_token.clone()),
        )
        .await?;
        let draft = generated.draft;
        validate_generated_draft(&request, &prefix, &draft)?;

        let expected_summary_id = draft.id.clone();
        let committed = cancellable(
            cancellation_token,
            (self.services.commit)(
                AgentContextCompactionCommitRequest {
                    run_id: request.run_id,
                    conversation_id: request.conversation_id,
                    assistant_message_id: request.assistant_message_id,
                    prefix,
                    draft,
                },
                cancellation_token.clone(),
            ),
        )
        .await?;
        match committed {
            AgentContextCompactionCommitOutcome::Applied {
                summary_id,
                baseline,
            } => {
                if summary_id != expected_summary_id {
                    return Err(contract_error("宿主返回的摘要 ID 与已提交草稿不一致。"));
                }
                Ok(ContextCompactionExecution::Rebase {
                    baseline,
                    usage: generated.usage,
                })
            }
            AgentContextCompactionCommitOutcome::Refresh(baseline) => {
                Ok(ContextCompactionExecution::Rebase {
                    baseline,
                    usage: generated.usage,
                })
            }
        }
    }
}

fn prepare_request_from_plan(
    plan: &ContextCompactionPlan,
    run_id: &str,
    conversation_id: Option<&str>,
    assistant_message_id: Option<&str>,
) -> Option<AgentContextCompactionPrepareRequest> {
    if plan.status != ContextCompactionPlanStatus::Required {
        return None;
    }
    let durable_step = plan
        .steps
        .iter()
        .find(|step| step.scope == ContextCompactionScope::DurableHistory)?;
    let prefix = durable_step.durable_prefix.as_ref()?;
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
        newly_covered_message_ids: prefix.covered_message_ids.clone(),
        covered_through_message_id: prefix.covered_through_message_id.clone(),
        source_input_tokens: durable_step.source_input_tokens,
        maximum_summary_tokens: durable_step.maximum_summary_tokens,
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
        || prefix.covered_through_message_id != request.covered_through_message_id
    {
        return Err(contract_error(
            "宿主准备的 durable 前缀与压缩计划身份不一致。",
        ));
    }
    let previous_count = prefix
        .previous_summary
        .as_ref()
        .map_or(0, |summary| summary.covered_message_ids.len());
    if prefix.covered_message_ids[previous_count..] != request.newly_covered_message_ids {
        return Err(contract_error(
            "宿主准备的 durable 前缀消息范围与压缩计划不一致。",
        ));
    }
    Ok(())
}

fn validate_generated_draft(
    request: &AgentContextCompactionPrepareRequest,
    prefix: &ContextCompactionPrefix,
    draft: &ContextCompactionSummaryDraft,
) -> AgentResult<()> {
    draft.validate()?;
    if draft.source_revision != prefix.source_revision
        || draft.source_input_tokens != request.source_input_tokens
    {
        return Err(contract_error(
            "摘要生成结果没有绑定当前 durable 前缀及其计量。",
        ));
    }
    if draft.summary_input_tokens > request.maximum_summary_tokens {
        return Err(AgentError::structured(
            "context_compaction_summary_too_large",
            "上下文压缩摘要超过规划器允许的最大 token 预算。",
            serde_json::json!({
                "summaryInputTokens": draft.summary_input_tokens,
                "maximumSummaryTokens": request.maximum_summary_tokens,
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
            soft_trigger_input_tokens: Some(7_500),
            target_input_tokens: Some(7_500),
            durable_capacity_tokens: Some(9_000),
            durable_trigger_input_tokens: Some(6_750),
            durable_target_input_tokens: Some(1_350),
            required_reclaimed_tokens: 7_000,
            required_durable_reclaimed_tokens: 7_000,
            planned_reclaimed_tokens: 744,
            planned_durable_reclaimed_tokens: 744,
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
                scope: ContextCompactionScope::DurableHistory,
                ranges: Vec::new(),
                atomic_unit_count: 2,
                source_input_tokens: 1_000,
                maximum_summary_tokens: 256,
                expected_reclaimed_tokens: 744,
                contains_side_effects: false,
                contains_errors: false,
                durable_prefix: Some(ContextCompactionDurablePrefix {
                    previous_summary_id: None,
                    covered_message_ids: vec!["user-old".to_string(), "assistant-old".to_string()],
                    covered_through_message_id: "assistant-old".to_string(),
                }),
            }],
        }
    }

    fn prefix() -> ContextCompactionPrefix {
        ContextCompactionPrefix {
            conversation_id: "conversation-1".to_string(),
            source_revision: "source-revision-1".to_string(),
            covered_through_message_id: "assistant-old".to_string(),
            covered_message_ids: vec!["user-old".to_string(), "assistant-old".to_string()],
            previous_summary: None,
            source_messages: vec![
                crate::ContextCompactionSourceMessage {
                    message_id: "user-old".to_string(),
                    role: "user".to_string(),
                    content: "old request".to_string(),
                    created_at: 1,
                    status: Some("sent".to_string()),
                    conversation_turn_trace: None,
                },
                crate::ContextCompactionSourceMessage {
                    message_id: "assistant-old".to_string(),
                    role: "assistant".to_string(),
                    content: "old answer".to_string(),
                    created_at: 2,
                    status: Some("sent".to_string()),
                    conversation_turn_trace: None,
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
        );
        state.shared_baseline().unwrap()
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
        );
        let executor = ContextCompactionExecutor::new(services);
        let cancellation = AgentCancellationToken::new();
        let cancel = cancellation.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(10)).await;
            cancel.cancel();
        });

        let error = executor
            .execute(
                &plan(),
                "run-1",
                Some("conversation-1"),
                Some("assistant-current"),
                &cancellation,
            )
            .await
            .unwrap_err();

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
        );

        let execution = ContextCompactionExecutor::new(services)
            .execute(
                &plan(),
                "run-1",
                Some("conversation-1"),
                Some("assistant-current"),
                &AgentCancellationToken::new(),
            )
            .await
            .unwrap();

        assert!(matches!(
            execution,
            ContextCompactionExecution::Rebase { usage: None, .. }
        ));
        assert!(!generated.load(Ordering::SeqCst));
        assert!(!committed.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn oversized_generated_summary_is_rejected_before_commit() {
        let committed = Arc::new(AtomicBool::new(false));
        let committed_for_callback = committed.clone();
        let services = AgentContextCompactionServices::new(
            |_, _| async {
                Ok(AgentContextCompactionPrepareOutcome::Ready(Arc::new(
                    prefix(),
                )))
            },
            |request, _| async move {
                Ok(AgentContextCompactionGenerationOutput {
                    draft: ContextCompactionSummaryDraft {
                        id: "summary-too-large".to_string(),
                        source_revision: request.prefix.source_revision.clone(),
                        content: "oversized summary".to_string(),
                        generation: crate::ContextCompactionGeneration::test(),
                        source_input_tokens: request.source_input_tokens,
                        summary_input_tokens: request.maximum_summary_tokens + 1,
                        created_at: 1,
                    },
                    usage: None,
                })
            },
            move |_, _| {
                committed_for_callback.store(true, Ordering::SeqCst);
                async { panic!("oversized summary must not commit") }
            },
        );

        let error = ContextCompactionExecutor::new(services)
            .execute(
                &plan(),
                "run-1",
                Some("conversation-1"),
                Some("assistant-current"),
                &AgentCancellationToken::new(),
            )
            .await
            .unwrap_err();

        assert_eq!(error.code(), Some("context_compaction_summary_too_large"));
        assert!(!committed.load(Ordering::SeqCst));
    }
}
