//! Read-only acceptance model for context compaction and request-size estimation.
//!
//! Audit reports never mutate compaction state and never contain prompt or summary bodies. They
//! distinguish hard consistency failures from soft planning misses and raw estimation evidence.

use crate::{
    AgentApiStyle, ContextCompactionReceipt, ContextCompactionReceiptStatus,
    ModelRequestObservation, ModelRequestObservationStatus, ModelRequestPurpose,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ContextCompactionAuditVerdict {
    Pass,
    Warning,
    Fail,
    InProgress,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ContextCompactionAuditCheckStatus {
    Pass,
    Warning,
    Fail,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ContextCompactionSummaryRelation {
    Active,
    Superseded,
    Detached,
    Missing,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ContextCompactionSummaryEvidence {
    pub summary_id: String,
    pub relation: ContextCompactionSummaryRelation,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_revision: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_input_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary_input_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub continuity_input_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub uncovered_tail_input_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub replacement_input_tokens: Option<u64>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ContextCompactionAuditCheck {
    pub code: String,
    pub status: ContextCompactionAuditCheckStatus,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<Value>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ContextCompactionAuditReport {
    pub operation_id: String,
    pub verdict: ContextCompactionAuditVerdict,
    pub receipt: ContextCompactionReceipt,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub generation_observation: Option<ModelRequestObservation>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<ContextCompactionSummaryEvidence>,
    pub checks: Vec<ContextCompactionAuditCheck>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ModelRequestEstimationErrorGroup {
    pub model: String,
    pub api_style: AgentApiStyle,
    pub purpose: ModelRequestPurpose,
    pub observation_count: u64,
    pub comparable_sample_count: u64,
    pub estimate_unavailable_count: u64,
    pub actual_usage_unavailable_count: u64,
    pub retry_affected_count: u64,
    pub estimated_input_tokens: u64,
    pub normalized_actual_input_tokens: u64,
    /// Sum of `estimated - normalized actual`; positive values mean overestimation.
    pub estimated_minus_actual_tokens: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub weighted_signed_error_basis_points: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub weighted_absolute_error_basis_points: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub median_absolute_percentage_error_basis_points: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub p95_absolute_percentage_error_basis_points: Option<u64>,
    pub underestimation_count: u64,
    pub overestimation_count: u64,
    pub exact_count: u64,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ContextCompactionAuditBundle {
    pub conversation_id: String,
    pub generated_at: i64,
    pub reports: Vec<ContextCompactionAuditReport>,
    pub estimation_error_groups: Vec<ModelRequestEstimationErrorGroup>,
}

pub(crate) fn build_compaction_audit_report(
    receipt: ContextCompactionReceipt,
    observation: Option<ModelRequestObservation>,
    summary: Option<ContextCompactionSummaryEvidence>,
) -> ContextCompactionAuditReport {
    let mut checks = Vec::new();
    push_check(
        &mut checks,
        "receipt_schema",
        ContextCompactionAuditCheckStatus::Pass,
        "Receipt 结构与状态机校验通过。",
        None,
    );

    match (&receipt.generation_observation_id, &observation) {
        (Some(expected), Some(actual)) if expected == &actual.id => {
            let identity_matches = actual.operation_id.as_deref()
                == Some(receipt.operation_id.as_str())
                && actual.run_id == receipt.run_id
                && actual.conversation_id.as_deref() == Some(receipt.conversation_id.as_str())
                && actual.assistant_message_id.as_deref()
                    == Some(receipt.assistant_message_id.as_str())
                && actual.model == receipt.model
                && actual.api_style == receipt.api_style;
            push_check(
                &mut checks,
                "generation_observation_identity",
                if identity_matches {
                    ContextCompactionAuditCheckStatus::Pass
                } else {
                    ContextCompactionAuditCheckStatus::Fail
                },
                if identity_matches {
                    "模型请求观测与本次压缩身份一致。"
                } else {
                    "模型请求观测与本次压缩身份不一致。"
                },
                None,
            );
            let retry_affected = actual
                .actual_usage
                .as_ref()
                .and_then(|usage| usage.raw.billable_request_count)
                .is_some_and(|count| count != 1);
            let comparable = (!retry_affected)
                .then(|| {
                    actual
                        .estimate
                        .as_ref()
                        .zip(actual.normalized_actual_input_tokens())
                })
                .flatten();
            match comparable {
                Some((estimate, actual_tokens)) => push_check(
                    &mut checks,
                    "generation_usage_observed",
                    ContextCompactionAuditCheckStatus::Pass,
                    "已同时记录发送前估算与 provider 实际输入 usage。",
                    Some(json!({
                        "estimatedInputTokens": estimate.estimated_input_tokens,
                        "normalizedActualInputTokens": actual_tokens,
                        "estimatedMinusActualTokens": signed_difference(
                            estimate.estimated_input_tokens,
                            actual_tokens,
                        ),
                    })),
                ),
                None if retry_affected => push_check(
                    &mut checks,
                    "generation_usage_retry_affected",
                    ContextCompactionAuditCheckStatus::Warning,
                    "本次 usage 汇总了多次 provider 尝试，已保留审计但不作为估算校准样本。",
                    None,
                ),
                None => push_check(
                    &mut checks,
                    "generation_usage_unavailable",
                    ContextCompactionAuditCheckStatus::Warning,
                    "本次请求缺少可比较的估算或 provider 输入 usage。",
                    None,
                ),
            }
        }
        (Some(_), None) => push_check(
            &mut checks,
            "generation_observation_missing",
            ContextCompactionAuditCheckStatus::Fail,
            "Receipt 引用了不存在的模型请求观测。",
            None,
        ),
        (None, Some(_)) => push_check(
            &mut checks,
            "generation_observation_unexpected",
            ContextCompactionAuditCheckStatus::Fail,
            "压缩 receipt 未引用随报告出现的模型请求观测。",
            None,
        ),
        (Some(expected), Some(actual)) => push_check(
            &mut checks,
            "generation_observation_reference",
            ContextCompactionAuditCheckStatus::Fail,
            "Receipt 引用的模型请求观测 ID 不一致。",
            Some(json!({ "expected": expected, "actual": actual.id })),
        ),
        (None, None) => {}
    }

    match receipt.status {
        ContextCompactionReceiptStatus::Applied => {
            let result = receipt.result.as_ref().expect("validated applied receipt");
            match summary.as_ref() {
                Some(evidence)
                    if evidence.relation == ContextCompactionSummaryRelation::Missing =>
                {
                    push_check(
                        &mut checks,
                        "summary_retention",
                        ContextCompactionAuditCheckStatus::Warning,
                        "压缩当时已成功提交，但摘要后来因历史编辑、删除或回退不再保留。",
                        None,
                    );
                }
                Some(evidence) => {
                    let metrics_match = evidence.source_revision.as_deref()
                        == receipt.source_revision.as_deref()
                        && evidence.source_input_tokens == Some(result.source_input_tokens)
                        && evidence.summary_input_tokens == Some(result.summary_input_tokens)
                        && evidence.continuity_input_tokens == Some(result.continuity_input_tokens)
                        && evidence.uncovered_tail_input_tokens
                            == Some(result.uncovered_tail_input_tokens)
                        && evidence.replacement_input_tokens
                            == Some(result.replacement_input_tokens);
                    push_check(
                        &mut checks,
                        "summary_metrics",
                        if metrics_match {
                            ContextCompactionAuditCheckStatus::Pass
                        } else {
                            ContextCompactionAuditCheckStatus::Fail
                        },
                        if metrics_match {
                            "不可变摘要与 receipt 的提交计量一致。"
                        } else {
                            "不可变摘要与 receipt 的提交计量不一致。"
                        },
                        None,
                    );
                    let (status, message) = match evidence.relation {
                        ContextCompactionSummaryRelation::Active => (
                            ContextCompactionAuditCheckStatus::Pass,
                            "该摘要当前仍是会话 active head。",
                        ),
                        ContextCompactionSummaryRelation::Superseded => (
                            ContextCompactionAuditCheckStatus::Pass,
                            "该摘要已被后续压缩正常接替。",
                        ),
                        ContextCompactionSummaryRelation::Detached => (
                            ContextCompactionAuditCheckStatus::Warning,
                            "该摘要存在，但已不在当前 active summary 链上。",
                        ),
                        ContextCompactionSummaryRelation::Missing => unreachable!(),
                    };
                    push_check(&mut checks, "summary_relation", status, message, None);
                }
                None => push_check(
                    &mut checks,
                    "summary_evidence_missing",
                    ContextCompactionAuditCheckStatus::Fail,
                    "验收查询没有返回 applied receipt 对应的摘要证据。",
                    None,
                ),
            }
            let target_met =
                result.replacement_input_tokens <= receipt.plan.target_replacement_tokens;
            push_check(
                &mut checks,
                "soft_replacement_target",
                if target_met {
                    ContextCompactionAuditCheckStatus::Pass
                } else {
                    ContextCompactionAuditCheckStatus::Warning
                },
                if target_met {
                    "替换内容达到规划器的软压缩目标。"
                } else {
                    "替换内容有效缩小了上下文，但未达到规划器的软压缩目标。"
                },
                Some(json!({
                    "targetReplacementTokens": receipt.plan.target_replacement_tokens,
                    "actualReplacementTokens": result.replacement_input_tokens,
                })),
            );
            let observation_completed = observation
                .as_ref()
                .is_some_and(|item| item.status == ModelRequestObservationStatus::Completed);
            let summary_present = summary.as_ref().is_some_and(|evidence| {
                evidence.relation != ContextCompactionSummaryRelation::Missing
            });
            push_check(
                &mut checks,
                "atomic_success_evidence",
                if observation_completed && summary_present {
                    ContextCompactionAuditCheckStatus::Pass
                } else if observation_completed
                    && summary.as_ref().is_some_and(|evidence| {
                        evidence.relation == ContextCompactionSummaryRelation::Missing
                    })
                {
                    ContextCompactionAuditCheckStatus::Warning
                } else {
                    ContextCompactionAuditCheckStatus::Fail
                },
                if observation_completed && summary_present {
                    "成功 receipt、模型请求观测和摘要提交证据齐全。"
                } else if observation_completed {
                    "模型请求观测仍在，但摘要已被后续历史变更清理，无法验收当前摘要行。"
                } else {
                    "成功压缩缺少模型请求观测或摘要提交证据。"
                },
                None,
            );
        }
        ContextCompactionReceiptStatus::InProgress => push_check(
            &mut checks,
            "operation_terminal_state",
            ContextCompactionAuditCheckStatus::Warning,
            "压缩仍在运行，当前报告不是终态验收。",
            None,
        ),
        ContextCompactionReceiptStatus::Refreshed => push_check(
            &mut checks,
            "stale_plan_refresh",
            ContextCompactionAuditCheckStatus::Pass,
            "压缩计划在提交前失效，未移动 active head，并已刷新上下文基线。",
            None,
        ),
        ContextCompactionReceiptStatus::Failed
        | ContextCompactionReceiptStatus::Cancelled
        | ContextCompactionReceiptStatus::Interrupted => push_check(
            &mut checks,
            "non_applied_terminal_state",
            ContextCompactionAuditCheckStatus::Pass,
            "压缩以非成功终态结束，receipt 未宣称已提交摘要。",
            None,
        ),
    }

    let verdict = if checks
        .iter()
        .any(|check| check.status == ContextCompactionAuditCheckStatus::Fail)
    {
        ContextCompactionAuditVerdict::Fail
    } else if receipt.status == ContextCompactionReceiptStatus::InProgress {
        ContextCompactionAuditVerdict::InProgress
    } else if checks
        .iter()
        .any(|check| check.status == ContextCompactionAuditCheckStatus::Warning)
    {
        ContextCompactionAuditVerdict::Warning
    } else {
        ContextCompactionAuditVerdict::Pass
    };
    ContextCompactionAuditReport {
        operation_id: receipt.operation_id.clone(),
        verdict,
        receipt,
        generation_observation: observation,
        summary,
        checks,
    }
}

pub(crate) fn group_estimation_errors(
    observations: &[ModelRequestObservation],
) -> Vec<ModelRequestEstimationErrorGroup> {
    let mut groups: BTreeMap<(String, String, ModelRequestPurpose), ErrorAccumulator> =
        BTreeMap::new();
    for observation in observations {
        let api_key = match observation.api_style {
            AgentApiStyle::OpenAiCompatible => "open_ai_compatible",
            AgentApiStyle::AnthropicCompatible => "anthropic_compatible",
        };
        groups
            .entry((
                observation.model.clone(),
                api_key.to_string(),
                observation.purpose,
            ))
            .or_insert_with(|| ErrorAccumulator::new(observation.api_style))
            .push(observation);
    }
    groups
        .into_iter()
        .map(|((model, _, purpose), accumulator)| accumulator.finish(model, purpose))
        .collect()
}

fn push_check(
    checks: &mut Vec<ContextCompactionAuditCheck>,
    code: &str,
    status: ContextCompactionAuditCheckStatus,
    message: &str,
    details: Option<Value>,
) {
    checks.push(ContextCompactionAuditCheck {
        code: code.to_string(),
        status,
        message: message.to_string(),
        details,
    });
}

struct ErrorAccumulator {
    api_style: AgentApiStyle,
    observation_count: u64,
    comparable_sample_count: u64,
    estimate_unavailable_count: u64,
    actual_usage_unavailable_count: u64,
    retry_affected_count: u64,
    estimated_input_tokens: u128,
    actual_input_tokens: u128,
    signed_error_tokens: i128,
    absolute_error_tokens: u128,
    absolute_percentage_errors: Vec<u64>,
    underestimation_count: u64,
    overestimation_count: u64,
    exact_count: u64,
}

impl ErrorAccumulator {
    fn new(api_style: AgentApiStyle) -> Self {
        Self {
            api_style,
            observation_count: 0,
            comparable_sample_count: 0,
            estimate_unavailable_count: 0,
            actual_usage_unavailable_count: 0,
            retry_affected_count: 0,
            estimated_input_tokens: 0,
            actual_input_tokens: 0,
            signed_error_tokens: 0,
            absolute_error_tokens: 0,
            absolute_percentage_errors: Vec::new(),
            underestimation_count: 0,
            overestimation_count: 0,
            exact_count: 0,
        }
    }

    fn push(&mut self, observation: &ModelRequestObservation) {
        self.observation_count = self.observation_count.saturating_add(1);
        let estimated = observation
            .estimate
            .as_ref()
            .map(|estimate| estimate.estimated_input_tokens);
        let actual = observation.normalized_actual_input_tokens();
        if estimated.is_none() {
            self.estimate_unavailable_count = self.estimate_unavailable_count.saturating_add(1);
        }
        if actual.is_none() {
            self.actual_usage_unavailable_count =
                self.actual_usage_unavailable_count.saturating_add(1);
        }
        let retry_affected = observation
            .actual_usage
            .as_ref()
            .and_then(|usage| usage.raw.billable_request_count)
            .is_some_and(|count| count != 1);
        if retry_affected {
            self.retry_affected_count = self.retry_affected_count.saturating_add(1);
            return;
        }
        let (Some(estimated), Some(actual)) = (estimated, actual) else {
            return;
        };
        self.comparable_sample_count = self.comparable_sample_count.saturating_add(1);
        self.estimated_input_tokens = self
            .estimated_input_tokens
            .saturating_add(u128::from(estimated));
        self.actual_input_tokens = self.actual_input_tokens.saturating_add(u128::from(actual));
        let difference = i128::from(estimated) - i128::from(actual);
        self.signed_error_tokens = self.signed_error_tokens.saturating_add(difference);
        self.absolute_error_tokens = self
            .absolute_error_tokens
            .saturating_add(difference.unsigned_abs());
        match difference.cmp(&0) {
            std::cmp::Ordering::Less => {
                self.underestimation_count = self.underestimation_count.saturating_add(1)
            }
            std::cmp::Ordering::Greater => {
                self.overestimation_count = self.overestimation_count.saturating_add(1)
            }
            std::cmp::Ordering::Equal => self.exact_count = self.exact_count.saturating_add(1),
        }
        if actual > 0 {
            self.absolute_percentage_errors.push(clamp_u128_to_u64(
                difference
                    .unsigned_abs()
                    .saturating_mul(10_000)
                    .saturating_div(u128::from(actual)),
            ));
        }
    }

    fn finish(
        mut self,
        model: String,
        purpose: ModelRequestPurpose,
    ) -> ModelRequestEstimationErrorGroup {
        self.absolute_percentage_errors.sort_unstable();
        let median = percentile(&self.absolute_percentage_errors, 50);
        let p95 = percentile(&self.absolute_percentage_errors, 95);
        let weighted_signed = (self.actual_input_tokens > 0).then(|| {
            clamp_i128_to_i64(
                self.signed_error_tokens
                    .saturating_mul(10_000)
                    .saturating_div(self.actual_input_tokens as i128),
            )
        });
        let weighted_absolute = (self.actual_input_tokens > 0).then(|| {
            clamp_u128_to_u64(
                self.absolute_error_tokens
                    .saturating_mul(10_000)
                    .saturating_div(self.actual_input_tokens),
            )
        });
        ModelRequestEstimationErrorGroup {
            model,
            api_style: self.api_style,
            purpose,
            observation_count: self.observation_count,
            comparable_sample_count: self.comparable_sample_count,
            estimate_unavailable_count: self.estimate_unavailable_count,
            actual_usage_unavailable_count: self.actual_usage_unavailable_count,
            retry_affected_count: self.retry_affected_count,
            estimated_input_tokens: clamp_u128_to_u64(self.estimated_input_tokens),
            normalized_actual_input_tokens: clamp_u128_to_u64(self.actual_input_tokens),
            estimated_minus_actual_tokens: clamp_i128_to_i64(self.signed_error_tokens),
            weighted_signed_error_basis_points: weighted_signed,
            weighted_absolute_error_basis_points: weighted_absolute,
            median_absolute_percentage_error_basis_points: median,
            p95_absolute_percentage_error_basis_points: p95,
            underestimation_count: self.underestimation_count,
            overestimation_count: self.overestimation_count,
            exact_count: self.exact_count,
        }
    }
}

fn percentile(values: &[u64], percentile: usize) -> Option<u64> {
    if values.is_empty() {
        return None;
    }
    let rank = values
        .len()
        .saturating_mul(percentile)
        .saturating_add(99)
        .saturating_div(100)
        .max(1);
    values.get(rank.saturating_sub(1)).copied()
}

fn signed_difference(estimated: u64, actual: u64) -> i64 {
    clamp_i128_to_i64(i128::from(estimated) - i128::from(actual))
}

fn clamp_i128_to_i64(value: i128) -> i64 {
    value.clamp(i128::from(i64::MIN), i128::from(i64::MAX)) as i64
}

fn clamp_u128_to_u64(value: u128) -> u64 {
    value.min(u128::from(u64::MAX)) as u64
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model_request_observation::ModelRequestObservationBuilder;
    use crate::{
        AgentUsage, ContextCompactionReceiptPlan, ContextCompactionReceiptResult,
        ContextCompactionReceiptStage, ContextJournalCursor, ModelRequestCapacityStatus,
        ModelRequestEstimate, ModelRequestMeasurementMode,
    };

    fn estimate(tokens: u64) -> ModelRequestEstimate {
        ModelRequestEstimate {
            estimator_id: "test-estimator".to_string(),
            estimator_version: 1,
            measurement_mode: ModelRequestMeasurementMode::FullRecount,
            capacity_status: ModelRequestCapacityStatus::WithinBudget,
            context_revision: "1".to_string(),
            persistent_revision: "1".to_string(),
            fixed_input_tokens: tokens,
            durable_input_tokens: 0,
            run_transient_input_tokens: 0,
            request_only_input_tokens: 0,
            additive_input_tokens: tokens,
            verified_total_input_tokens: None,
            estimated_input_tokens: tokens,
            system_tokens: tokens,
            tool_schema_tokens: 0,
            summary_tokens: 0,
            continuity_tokens: 0,
            world_state_tokens: 0,
            goal_tokens: 0,
            todo_tokens: 0,
            recent_history_tokens: 0,
            total_input_tokens: tokens,
            context_window_tokens: Some(1_000),
            reserved_output_tokens: 100,
            safety_margin_tokens: 0,
        }
    }

    fn agent_observation(id: &str, estimated: u64, actual: Option<u64>) -> ModelRequestObservation {
        ModelRequestObservationBuilder::new(
            id,
            "run-1",
            None,
            None,
            None,
            1,
            ModelRequestPurpose::AgentLoop,
            "test-model",
            AgentApiStyle::OpenAiCompatible,
            Some(estimate(estimated)),
            1,
        )
        .completed(
            actual.map(|actual| AgentUsage {
                input_tokens: Some(actual),
                output_tokens: Some(1),
                output_thinking_tokens: None,
                total_tokens: Some(actual.saturating_add(1)),
                cached_input_tokens: None,
                cache_creation_input_tokens: None,
                billable_request_count: Some(1),
            }),
            Some("stop".to_string()),
            2,
        )
        .unwrap()
    }

    fn compaction_observation() -> ModelRequestObservation {
        ModelRequestObservationBuilder::new(
            "observation-1",
            "run-1",
            Some("conversation-1".to_string()),
            Some("assistant-1".to_string()),
            Some("operation-1".to_string()),
            1,
            ModelRequestPurpose::ContextCompaction,
            "test-model",
            AgentApiStyle::OpenAiCompatible,
            Some(estimate(80)),
            1,
        )
        .completed(
            Some(AgentUsage {
                input_tokens: Some(100),
                output_tokens: Some(10),
                output_thinking_tokens: None,
                total_tokens: Some(110),
                cached_input_tokens: None,
                cache_creation_input_tokens: None,
                billable_request_count: Some(1),
            }),
            Some("stop".to_string()),
            2,
        )
        .unwrap()
    }

    fn applied_receipt() -> ContextCompactionReceipt {
        ContextCompactionReceipt {
            schema_version: crate::CONTEXT_COMPACTION_RECEIPT_SCHEMA_VERSION,
            operation_id: "operation-1".to_string(),
            run_id: "run-1".to_string(),
            conversation_id: "conversation-1".to_string(),
            assistant_message_id: "assistant-1".to_string(),
            request_index: 1,
            attempt_index: 1,
            model: "test-model".to_string(),
            api_style: AgentApiStyle::OpenAiCompatible,
            status: ContextCompactionReceiptStatus::Applied,
            stage: ContextCompactionReceiptStage::Completed,
            plan: ContextCompactionReceiptPlan {
                context_revision: "1".to_string(),
                persistent_revision: "1".to_string(),
                request_input_tokens: 120,
                available_input_tokens: Some(120),
                request_trigger_input_tokens: Some(100),
                request_target_input_tokens: Some(30),
                request_pressure: true,
                durable_input_tokens: 100,
                durable_capacity_tokens: Some(120),
                durable_trigger_input_tokens: Some(100),
                durable_target_input_tokens: Some(30),
                durable_pressure: true,
                source_input_tokens: 100,
                target_replacement_tokens: 30,
                expected_reclaimed_tokens: 70,
                planned_reclaimed_tokens: 70,
                projected_request_input_tokens: 50,
                projected_durable_input_tokens: 40,
                best_effort: false,
                protected_input_tokens: 0,
                protected_reasons: Default::default(),
                atomic_unit_count: 2,
                previous_summary_id: None,
                covered_through: ContextJournalCursor::message("assistant-old"),
            },
            source_revision: Some("source-1".to_string()),
            generation_observation_id: Some("observation-1".to_string()),
            summary_id: Some("summary-1".to_string()),
            result: Some(ContextCompactionReceiptResult {
                summary_id: "summary-1".to_string(),
                source_input_tokens: 100,
                summary_input_tokens: 20,
                continuity_input_tokens: 20,
                uncovered_tail_input_tokens: 10,
                replacement_input_tokens: 40,
                reclaimed_input_tokens: 60,
            }),
            error: None,
            started_at: 1,
            updated_at: 2,
            completed_at: Some(2),
        }
    }

    #[test]
    fn estimation_groups_report_direction_and_distribution_without_calibration() {
        let observations = vec![
            agent_observation("observation-1", 100, Some(80)),
            agent_observation("observation-2", 90, Some(100)),
            agent_observation("observation-3", 50, None),
        ];

        let groups = group_estimation_errors(&observations);

        assert_eq!(groups.len(), 1);
        let group = &groups[0];
        assert_eq!(group.observation_count, 3);
        assert_eq!(group.comparable_sample_count, 2);
        assert_eq!(group.actual_usage_unavailable_count, 1);
        assert_eq!(group.retry_affected_count, 0);
        assert_eq!(group.estimated_input_tokens, 190);
        assert_eq!(group.normalized_actual_input_tokens, 180);
        assert_eq!(group.estimated_minus_actual_tokens, 10);
        assert_eq!(group.underestimation_count, 1);
        assert_eq!(group.overestimation_count, 1);
        assert_eq!(
            group.median_absolute_percentage_error_basis_points,
            Some(1_000)
        );
        assert_eq!(
            group.p95_absolute_percentage_error_basis_points,
            Some(2_500)
        );
    }

    #[test]
    fn missing_historical_summary_and_soft_target_miss_are_warnings_not_hard_failures() {
        let report = build_compaction_audit_report(
            applied_receipt(),
            Some(compaction_observation()),
            Some(ContextCompactionSummaryEvidence {
                summary_id: "summary-1".to_string(),
                relation: ContextCompactionSummaryRelation::Missing,
                source_revision: None,
                source_input_tokens: None,
                summary_input_tokens: None,
                continuity_input_tokens: None,
                uncovered_tail_input_tokens: None,
                replacement_input_tokens: None,
            }),
        );

        assert_eq!(report.verdict, ContextCompactionAuditVerdict::Warning);
        assert!(!report
            .checks
            .iter()
            .any(|check| check.status == ContextCompactionAuditCheckStatus::Fail));
    }
}
