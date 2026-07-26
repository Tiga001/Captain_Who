//! Classified context accounting and capacity policy for measured request frames.
//!
//! `ContextFrame` owns incremental message measurements. This module adds run-stable request
//! costs (tool definitions and protocol structure), applies the configured model window, and
//! decides whether a whole-frame verification pass is required near capacity.

use super::frame::{
    ContextFrame, ContextFrameEstimateBucket, ContextFrameMeasurement,
    ContextFrameSemanticBreakdown,
};
use super::measurement::{
    combine_context_revisions, ContextMessageEstimate, ContextRevisionHasher, ContextTextBudget,
    ContextTokenEstimator, HeuristicTokenEstimator,
};
use crate::llm::{LlmMessage, LlmToolCall};
use crate::protocol::{
    AgentApiStyle, AgentContextCostBreakdown, AgentContextWindowPhase, AgentContextWindowSnapshot,
    AgentContextWindowStatus, AgentError, AgentResult, AgentToolDefinition,
};
use serde::Serialize;
use std::sync::Arc;

const SAFETY_MARGIN_PERCENT: u64 = 5;
const MINIMUM_SAFETY_MARGIN_TOKENS: u64 = 1_024;
const TOOL_TEXT_OUTPUT_BUDGET_PERCENT: u64 = 15;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ContextBudgetStatus {
    Unconfigured,
    WithinBudget,
    OverBudget,
    InvalidConfiguration,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ContextMeasurementMode {
    IncrementalCache,
    FullRecount,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ContextTokenCategoryEstimate {
    pub(crate) input_tokens: u64,
    pub(crate) context_item_count: usize,
    pub(crate) message_content_tokens: u64,
    pub(crate) message_structure_tokens: u64,
    pub(crate) tool_call_tokens: u64,
    pub(crate) tool_definition_tokens: u64,
    pub(crate) tool_definition_count: usize,
    pub(crate) image_tokens: u64,
    pub(crate) image_count: usize,
    pub(crate) request_structure_tokens: u64,
}

impl ContextTokenCategoryEstimate {
    fn from_frame_bucket(bucket: ContextFrameEstimateBucket) -> Self {
        let ContextMessageEstimate {
            message_content_tokens,
            message_structure_tokens,
            tool_call_tokens,
            image_tokens,
            image_count,
        } = bucket.estimate;
        Self {
            input_tokens: bucket.estimate.total_tokens(),
            context_item_count: bucket.item_count,
            message_content_tokens,
            message_structure_tokens,
            tool_call_tokens,
            tool_definition_tokens: 0,
            tool_definition_count: 0,
            image_tokens,
            image_count,
            request_structure_tokens: 0,
        }
    }

    fn add_fixed_request_costs(&mut self, fixed: &FixedRequestEstimate) {
        self.tool_definition_tokens = self
            .tool_definition_tokens
            .saturating_add(fixed.tool_definition_tokens);
        self.tool_definition_count = self
            .tool_definition_count
            .saturating_add(fixed.tool_definition_count);
        self.request_structure_tokens = self
            .request_structure_tokens
            .saturating_add(fixed.request_structure_tokens);
        self.input_tokens = self
            .input_tokens
            .saturating_add(fixed.tool_definition_tokens)
            .saturating_add(fixed.request_structure_tokens);
    }

    fn add_transient_tool_costs(&mut self, transient: &FixedRequestEstimate) {
        self.tool_definition_tokens = self
            .tool_definition_tokens
            .saturating_add(transient.tool_definition_tokens);
        self.tool_definition_count = self
            .tool_definition_count
            .saturating_add(transient.tool_definition_count);
        self.input_tokens = self
            .input_tokens
            .saturating_add(transient.tool_definition_tokens);
    }

    fn merge(&mut self, other: &Self) {
        self.input_tokens = self.input_tokens.saturating_add(other.input_tokens);
        self.context_item_count = self
            .context_item_count
            .saturating_add(other.context_item_count);
        self.message_content_tokens = self
            .message_content_tokens
            .saturating_add(other.message_content_tokens);
        self.message_structure_tokens = self
            .message_structure_tokens
            .saturating_add(other.message_structure_tokens);
        self.tool_call_tokens = self.tool_call_tokens.saturating_add(other.tool_call_tokens);
        self.tool_definition_tokens = self
            .tool_definition_tokens
            .saturating_add(other.tool_definition_tokens);
        self.tool_definition_count = self
            .tool_definition_count
            .saturating_add(other.tool_definition_count);
        self.image_tokens = self.image_tokens.saturating_add(other.image_tokens);
        self.image_count = self.image_count.saturating_add(other.image_count);
        self.request_structure_tokens = self
            .request_structure_tokens
            .saturating_add(other.request_structure_tokens);
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ContextTokenBreakdown {
    pub(crate) fixed: ContextTokenCategoryEstimate,
    pub(crate) durable: ContextTokenCategoryEstimate,
    pub(crate) run_transient: ContextTokenCategoryEstimate,
    pub(crate) request_only: ContextTokenCategoryEstimate,
    pub(crate) total: ContextTokenCategoryEstimate,
    pub(crate) semantic: ContextFrameSemanticBreakdown,
}

impl ContextTokenBreakdown {
    fn from_frame(
        frame: &ContextFrameMeasurement,
        fixed: &FixedRequestEstimate,
        transient_tools: &FixedRequestEstimate,
    ) -> Self {
        debug_assert_eq!(
            frame.breakdown.semantic.total_tokens(),
            frame
                .breakdown
                .fixed
                .estimate
                .total_tokens()
                .saturating_add(frame.breakdown.durable.estimate.total_tokens())
                .saturating_add(frame.breakdown.run_transient.estimate.total_tokens())
                .saturating_add(frame.breakdown.request_only.estimate.total_tokens())
        );
        let mut fixed_category =
            ContextTokenCategoryEstimate::from_frame_bucket(frame.breakdown.fixed);
        fixed_category.add_fixed_request_costs(fixed);
        let durable = ContextTokenCategoryEstimate::from_frame_bucket(frame.breakdown.durable);
        let mut run_transient =
            ContextTokenCategoryEstimate::from_frame_bucket(frame.breakdown.run_transient);
        run_transient.add_transient_tool_costs(transient_tools);
        let request_only =
            ContextTokenCategoryEstimate::from_frame_bucket(frame.breakdown.request_only);
        let mut total = ContextTokenCategoryEstimate::default();
        for category in [&fixed_category, &durable, &run_transient, &request_only] {
            total.merge(category);
        }
        Self {
            fixed: fixed_category,
            durable,
            run_transient,
            request_only,
            total,
            semantic: frame.breakdown.semantic,
        }
    }

    pub(crate) fn persistent_input_tokens(&self) -> u64 {
        self.fixed
            .input_tokens
            .saturating_add(self.durable.input_tokens)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ContextTokenEstimate {
    pub(crate) estimator_id: String,
    pub(crate) estimator_version: u32,
    pub(crate) measurement_mode: ContextMeasurementMode,
    pub(crate) context_revision: u64,
    pub(crate) persistent_revision: u64,
    pub(crate) breakdown: ContextTokenBreakdown,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) verified_total_input_tokens: Option<u64>,
    pub(crate) image_token_reserve_per_image: u64,
}

impl ContextTokenEstimate {
    pub(crate) fn request_input_tokens(&self) -> u64 {
        self.verified_total_input_tokens
            .unwrap_or(self.breakdown.total.input_tokens)
    }

    pub(crate) fn persistent_input_tokens(&self) -> u64 {
        self.breakdown.persistent_input_tokens()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ContextBudgetReport {
    pub(crate) status: ContextBudgetStatus,
    pub(crate) context_window_tokens: Option<u64>,
    pub(crate) reserved_output_tokens: u64,
    pub(crate) safety_margin_tokens: u64,
    pub(crate) available_input_tokens: Option<u64>,
    pub(crate) remaining_input_tokens: Option<i64>,
    pub(crate) excess_input_tokens: Option<u64>,
    pub(crate) usage: ContextTokenEstimate,
}

impl ContextBudgetReport {
    /// Maximum output reserve that can coexist with this exact measured input while preserving
    /// the configured safety margin. This is independent of the reserve used to build the report.
    pub(crate) fn maximum_output_tokens_for_current_input(&self) -> Option<u64> {
        self.context_window_tokens.map(|context_window_tokens| {
            context_window_tokens
                .saturating_sub(self.safety_margin_tokens)
                .saturating_sub(self.usage.request_input_tokens())
        })
    }

    pub(crate) fn persistent_snapshot(
        &self,
        model: &str,
        phase: AgentContextWindowPhase,
    ) -> AgentContextWindowSnapshot {
        let fixed_input_tokens = self.usage.breakdown.fixed.input_tokens;
        let durable_input_tokens = self.usage.breakdown.durable.input_tokens;
        let durable_capacity_tokens = self
            .available_input_tokens
            .map(|available| available.saturating_sub(fixed_input_tokens));
        let (status, remaining_durable_tokens) = durable_snapshot_capacity_state(
            self.context_window_tokens,
            self.available_input_tokens,
            fixed_input_tokens,
            durable_input_tokens,
        );

        AgentContextWindowSnapshot {
            model: model.to_string(),
            status,
            phase,
            context_window_tokens: self.context_window_tokens,
            reserved_output_tokens: self.reserved_output_tokens,
            safety_margin_tokens: self.safety_margin_tokens,
            durable_capacity_tokens,
            durable_input_tokens,
            run_transient_input_tokens: self.usage.breakdown.run_transient.input_tokens,
            request_input_tokens: self.usage.request_input_tokens(),
            cost_breakdown: self.context_cost_breakdown(),
            remaining_durable_tokens,
            persistent_revision: format!("{:016x}", self.usage.persistent_revision),
        }
    }

    pub(crate) fn context_cost_breakdown(&self) -> AgentContextCostBreakdown {
        let semantic = self.usage.breakdown.semantic;
        AgentContextCostBreakdown {
            system_tokens: semantic
                .system_tokens
                .saturating_add(self.usage.breakdown.fixed.request_structure_tokens),
            tool_schema_tokens: self
                .usage
                .breakdown
                .fixed
                .tool_definition_tokens
                .saturating_add(self.usage.breakdown.run_transient.tool_definition_tokens),
            summary_tokens: semantic.summary_tokens,
            continuity_tokens: semantic.continuity_tokens,
            world_state_tokens: semantic.world_state_tokens,
            goal_tokens: semantic.goal_tokens,
            todo_tokens: semantic.todo_tokens,
            recent_history_tokens: semantic.recent_history_tokens,
            total_input_tokens: self.usage.request_input_tokens(),
        }
    }

    pub(crate) fn compaction_query(&self) -> ContextCompactionQuery {
        ContextCompactionQuery {
            status: self.status,
            available_input_tokens: self.available_input_tokens,
            remaining_input_tokens: self.remaining_input_tokens,
            request_input_tokens: self.usage.request_input_tokens(),
            additive_input_tokens: self.usage.breakdown.total.input_tokens,
            persistent_input_tokens: self.usage.persistent_input_tokens(),
            context_revision: self.usage.context_revision,
            persistent_revision: self.usage.persistent_revision,
            breakdown: self.usage.breakdown.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
/// Stable handoff consumed by the future blocking compaction hook before capacity enforcement.
/// It is derived from the same report as the capacity gate and never triggers a second recount.
pub(crate) struct ContextCompactionQuery {
    pub(crate) status: ContextBudgetStatus,
    pub(crate) available_input_tokens: Option<u64>,
    pub(crate) remaining_input_tokens: Option<i64>,
    pub(crate) request_input_tokens: u64,
    pub(crate) additive_input_tokens: u64,
    pub(crate) persistent_input_tokens: u64,
    pub(crate) context_revision: u64,
    pub(crate) persistent_revision: u64,
    pub(crate) breakdown: ContextTokenBreakdown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ContextCapacityErrorCode {
    CapacityExceeded,
    InvalidConfiguration,
}

impl ContextCapacityErrorCode {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::CapacityExceeded => "context_capacity_exceeded",
            Self::InvalidConfiguration => "invalid_context_capacity_configuration",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ContextCapacityError {
    pub(crate) code: ContextCapacityErrorCode,
    pub(crate) report: ContextBudgetReport,
}

impl ContextCapacityError {
    fn into_agent_error(self) -> AgentError {
        let report = &self.report;
        let context_window = report.context_window_tokens.unwrap_or_default();
        let available_input = report.available_input_tokens.unwrap_or_default();
        let message = match self.code {
            ContextCapacityErrorCode::CapacityExceeded => format!(
                "上下文容量不足（{}）：本次请求预计需要 {} 个输入 token；模型总窗口为 {}，预留 {} 个输出 token 和 {} 个安全余量后，可用输入容量为 {}，超出 {}。请求尚未发送。请缩短对话或工具结果，或核对模型的上下文窗口配置。",
                self.code.as_str(),
                report.usage.request_input_tokens(),
                context_window,
                report.reserved_output_tokens,
                report.safety_margin_tokens,
                available_input,
                report.excess_input_tokens.unwrap_or_default(),
            ),
            ContextCapacityErrorCode::InvalidConfiguration => format!(
                "模型上下文容量配置无效（{}）：总窗口为 {}，但输出预留为 {}，安全余量为 {}，没有可用的输入空间。请求尚未发送。请调整模型上下文窗口或单次最大输出 token。",
                self.code.as_str(),
                context_window,
                report.reserved_output_tokens,
                report.safety_margin_tokens,
            ),
        };
        let details = serde_json::to_value(&self.report).unwrap_or(serde_json::Value::Null);
        AgentError::structured(self.code.as_str(), message, details)
    }
}

#[derive(Debug)]
struct FixedRequestEstimate {
    tool_definition_tokens: u64,
    tool_definition_count: usize,
    request_structure_tokens: u64,
    revision: u64,
}

/// Run-scoped capacity gate for fully assembled context frames.
///
/// The selected estimator and tool definitions are fixed for a run. A later tokenizer registry
/// can replace `select_token_estimator` without changing `ContextFrame` or the agent loop.
#[derive(Debug)]
pub(crate) struct ContextCapacityDetector {
    estimator: Arc<dyn ContextTokenEstimator>,
    fixed: FixedRequestEstimate,
}

impl ContextCapacityDetector {
    pub(crate) fn for_model(
        model: &str,
        api_style: AgentApiStyle,
        tools: &[AgentToolDefinition],
    ) -> Self {
        Self::new(select_token_estimator(model, api_style), tools)
    }

    fn new(estimator: Arc<dyn ContextTokenEstimator>, tools: &[AgentToolDefinition]) -> Self {
        let identity = estimator.identity();
        let request_structure_tokens = estimator.request_structure_tokens();
        let fixed = FixedRequestEstimate {
            tool_definition_tokens: estimator.estimate_tool_definitions(tools),
            tool_definition_count: tools.len(),
            request_structure_tokens,
            revision: fixed_request_revision(&identity, tools, request_structure_tokens),
        };
        Self { estimator, fixed }
    }

    /// Measures all existing frame items once. Subsequent `push` calls update the cached sum.
    pub(crate) fn prepare_frame(&self, frame: &mut ContextFrame) {
        frame.measure_incrementally(self.estimator.clone());
    }

    /// Derives a bounded tool-text allowance from the same estimator and reserve policy used by
    /// request capacity checks. The allowance is intentionally a soft content budget: tools may
    /// expose a continuation cursor rather than rejecting a large source.
    pub(crate) fn tool_output_text_budget(
        &self,
        context_window_tokens: Option<u32>,
        reserved_output_tokens: u32,
    ) -> ContextTextBudget {
        let max_tokens = context_window_tokens
            .map(u64::from)
            .map(|context_window_tokens| {
                context_window_tokens
                    .saturating_sub(u64::from(reserved_output_tokens))
                    .saturating_sub(safety_margin(context_window_tokens))
                    .saturating_mul(TOOL_TEXT_OUTPUT_BUDGET_PERCENT)
                    / 100
            })
            .unwrap_or(ContextTextBudget::DEFAULT_MAX_TOKENS)
            .clamp(1, ContextTextBudget::DEFAULT_MAX_TOKENS);
        ContextTextBudget::new(self.estimator.clone(), max_tokens)
    }

    /// Creates a text allowance backed by the exact estimator selected for this run.
    ///
    /// Runtime extensions use this when a newly disclosed context fragment must fit the
    /// remaining model-input capacity before they commit any activation side effects.
    pub(crate) fn text_budget(&self, max_tokens: u64) -> ContextTextBudget {
        ContextTextBudget::new(self.estimator.clone(), max_tokens)
    }

    /// Estimates the assistant messages retained by `ToolCallBatch` after runtime filtering.
    ///
    /// The runtime stores parallel calls as adjacent one-call messages (only the first keeps the
    /// assistant narration), so measuring one synthetic multi-call message would undercount
    /// structure and would charge calls discarded by a progressive-disclosure barrier.
    pub(crate) fn estimate_assistant_tool_batch_tokens(
        &self,
        content: &str,
        tool_calls: &[LlmToolCall],
    ) -> u64 {
        tool_calls
            .iter()
            .enumerate()
            .fold(0_u64, |total, (index, tool_call)| {
                total.saturating_add(
                    self.estimator
                        .estimate_message(&LlmMessage::assistant(
                            if index == 0 { content } else { "" },
                            vec![tool_call.clone()],
                        ))
                        .total_tokens(),
                )
            })
    }

    pub(crate) fn estimate_message_tokens(&self, message: &LlmMessage) -> u64 {
        self.estimator.estimate_message(message).total_tokens()
    }

    pub(crate) fn inspect(
        &self,
        frame: &mut ContextFrame,
        context_window_tokens: Option<u32>,
        reserved_output_tokens: u32,
    ) -> ContextBudgetReport {
        self.inspect_with_dynamic_tools(frame, context_window_tokens, reserved_output_tokens, &[])
    }

    /// Measures a request whose Skill-gated tool schemas are appended after the run-stable
    /// request prefix.
    ///
    /// Dynamic schemas consume real input capacity, but they deliberately remain outside the
    /// persistent revision and fixed-token bucket. This keeps cache diagnostics honest: changing
    /// an activated Skill changes the effective request revision without pretending that the
    /// stable prefix itself changed.
    pub(crate) fn inspect_with_dynamic_tools(
        &self,
        frame: &mut ContextFrame,
        context_window_tokens: Option<u32>,
        reserved_output_tokens: u32,
        dynamic_tools: &[AgentToolDefinition],
    ) -> ContextBudgetReport {
        let transient_tools = self.transient_tool_estimate(dynamic_tools);
        let incremental = frame.measure_incrementally(self.estimator.clone());
        let mut report = self.build_report(
            incremental,
            ContextMeasurementMode::IncrementalCache,
            context_window_tokens,
            reserved_output_tokens,
            &transient_tools,
        );
        if should_recount(self.estimator.full_recount_threshold_percent(), &report) {
            let full = frame.measure_full(self.estimator.clone());
            report = self.build_report(
                full,
                ContextMeasurementMode::FullRecount,
                context_window_tokens,
                reserved_output_tokens,
                &transient_tools,
            );
        }
        report
    }

    pub(crate) fn ensure_sendable(&self, report: ContextBudgetReport) -> AgentResult<()> {
        let code = match report.status {
            ContextBudgetStatus::Unconfigured | ContextBudgetStatus::WithinBudget => return Ok(()),
            ContextBudgetStatus::OverBudget => ContextCapacityErrorCode::CapacityExceeded,
            ContextBudgetStatus::InvalidConfiguration => {
                ContextCapacityErrorCode::InvalidConfiguration
            }
        };
        Err(ContextCapacityError { code, report }.into_agent_error())
    }

    fn build_report(
        &self,
        frame: ContextFrameMeasurement,
        measurement_mode: ContextMeasurementMode,
        context_window_tokens: Option<u32>,
        reserved_output_tokens: u32,
        transient_tools: &FixedRequestEstimate,
    ) -> ContextBudgetReport {
        let estimate = self.token_estimate(frame, measurement_mode, transient_tools);
        build_budget_report(
            estimate,
            context_window_tokens,
            u64::from(reserved_output_tokens),
        )
    }

    fn token_estimate(
        &self,
        frame: ContextFrameMeasurement,
        measurement_mode: ContextMeasurementMode,
        transient_tools: &FixedRequestEstimate,
    ) -> ContextTokenEstimate {
        let verified_total_input_tokens = frame.verified_total.map(|estimate| {
            estimate
                .total_tokens()
                .saturating_add(self.fixed.tool_definition_tokens)
                .saturating_add(transient_tools.tool_definition_tokens)
                .saturating_add(self.fixed.request_structure_tokens)
        });
        let breakdown = ContextTokenBreakdown::from_frame(&frame, &self.fixed, transient_tools);
        let context_revision = if transient_tools.tool_definition_count == 0 {
            frame.revision
        } else {
            combine_context_revisions(frame.revision, transient_tools.revision)
        };

        ContextTokenEstimate {
            estimator_id: frame.estimator.label(),
            estimator_version: frame.estimator.version,
            measurement_mode,
            context_revision,
            persistent_revision: combine_context_revisions(
                frame.persistent_revision,
                self.fixed.revision,
            ),
            breakdown,
            verified_total_input_tokens,
            image_token_reserve_per_image: self.estimator.image_token_reserve_per_image(),
        }
    }

    fn transient_tool_estimate(&self, tools: &[AgentToolDefinition]) -> FixedRequestEstimate {
        if tools.is_empty() {
            return FixedRequestEstimate {
                tool_definition_tokens: 0,
                tool_definition_count: 0,
                request_structure_tokens: 0,
                revision: 0,
            };
        }
        FixedRequestEstimate {
            tool_definition_tokens: self.estimator.estimate_tool_definitions(tools),
            tool_definition_count: tools.len(),
            request_structure_tokens: 0,
            revision: fixed_request_revision(&self.estimator.identity(), tools, 0),
        }
    }
}

fn fixed_request_revision(
    estimator: &super::measurement::ContextEstimatorIdentity,
    tools: &[AgentToolDefinition],
    request_structure_tokens: u64,
) -> u64 {
    let mut hasher = ContextRevisionHasher::new();
    hasher.write_str(estimator.family);
    hasher.write_u64(u64::from(estimator.version));
    hasher.write_str(estimator.variant.as_deref().unwrap_or_default());
    hasher.write_u64(request_structure_tokens);
    for tool in tools {
        hasher.write_str(&serde_json::to_string(tool).unwrap_or_else(|_| "null".to_string()));
    }
    hasher.finish()
}

fn select_token_estimator(
    _model: &str,
    _api_style: AgentApiStyle,
) -> Arc<dyn ContextTokenEstimator> {
    Arc::new(HeuristicTokenEstimator)
}

fn build_budget_report(
    estimate: ContextTokenEstimate,
    context_window_tokens: Option<u32>,
    reserved_output_tokens: u64,
) -> ContextBudgetReport {
    let Some(context_window_tokens) = context_window_tokens.map(u64::from) else {
        return ContextBudgetReport {
            status: ContextBudgetStatus::Unconfigured,
            context_window_tokens: None,
            reserved_output_tokens,
            safety_margin_tokens: 0,
            available_input_tokens: None,
            remaining_input_tokens: None,
            excess_input_tokens: None,
            usage: estimate,
        };
    };

    let safety_margin_tokens = safety_margin(context_window_tokens);
    let required_reserve = reserved_output_tokens.saturating_add(safety_margin_tokens);
    if required_reserve >= context_window_tokens {
        return ContextBudgetReport {
            status: ContextBudgetStatus::InvalidConfiguration,
            context_window_tokens: Some(context_window_tokens),
            reserved_output_tokens,
            safety_margin_tokens,
            available_input_tokens: Some(0),
            remaining_input_tokens: None,
            excess_input_tokens: None,
            usage: estimate,
        };
    }

    let available_input_tokens = context_window_tokens - required_reserve;
    let estimated_input_tokens = estimate.request_input_tokens();
    if estimated_input_tokens > available_input_tokens {
        ContextBudgetReport {
            status: ContextBudgetStatus::OverBudget,
            context_window_tokens: Some(context_window_tokens),
            reserved_output_tokens,
            safety_margin_tokens,
            available_input_tokens: Some(available_input_tokens),
            remaining_input_tokens: Some(saturating_signed_difference(
                available_input_tokens,
                estimated_input_tokens,
            )),
            excess_input_tokens: Some(estimated_input_tokens - available_input_tokens),
            usage: estimate,
        }
    } else {
        ContextBudgetReport {
            status: ContextBudgetStatus::WithinBudget,
            context_window_tokens: Some(context_window_tokens),
            reserved_output_tokens,
            safety_margin_tokens,
            available_input_tokens: Some(available_input_tokens),
            remaining_input_tokens: Some(saturating_signed_difference(
                available_input_tokens,
                estimated_input_tokens,
            )),
            excess_input_tokens: None,
            usage: estimate,
        }
    }
}

fn durable_snapshot_capacity_state(
    context_window_tokens: Option<u64>,
    available_input_tokens: Option<u64>,
    fixed_input_tokens: u64,
    durable_input_tokens: u64,
) -> (AgentContextWindowStatus, Option<i64>) {
    if context_window_tokens.is_none() {
        return (AgentContextWindowStatus::Unconfigured, None);
    }
    let Some(available_input_tokens) = available_input_tokens else {
        return (AgentContextWindowStatus::Unconfigured, None);
    };
    if available_input_tokens == 0 {
        return (AgentContextWindowStatus::InvalidConfiguration, None);
    }

    let durable_capacity_tokens = available_input_tokens.saturating_sub(fixed_input_tokens);
    let remaining_durable_tokens = Some(saturating_signed_difference(
        durable_capacity_tokens,
        durable_input_tokens,
    ));
    if fixed_input_tokens > available_input_tokens || durable_input_tokens > durable_capacity_tokens
    {
        (
            AgentContextWindowStatus::OverBudget,
            remaining_durable_tokens,
        )
    } else {
        (
            AgentContextWindowStatus::WithinBudget,
            remaining_durable_tokens,
        )
    }
}

fn should_recount(threshold_percent: Option<u8>, report: &ContextBudgetReport) -> bool {
    let Some(threshold_percent) = threshold_percent else {
        return false;
    };
    if report.status == ContextBudgetStatus::OverBudget {
        return true;
    }
    if report.status != ContextBudgetStatus::WithinBudget {
        return false;
    }
    let Some(available_input_tokens) = report.available_input_tokens else {
        return false;
    };
    let threshold_percent = u64::from(threshold_percent.clamp(1, 100));
    report.usage.request_input_tokens().saturating_mul(100)
        >= available_input_tokens.saturating_mul(threshold_percent)
}

fn safety_margin(context_window_tokens: u64) -> u64 {
    context_window_tokens
        .saturating_mul(SAFETY_MARGIN_PERCENT)
        .div_ceil(100)
        .max(MINIMUM_SAFETY_MARGIN_TOKENS)
}

fn saturating_signed_difference(left: u64, right: u64) -> i64 {
    if left >= right {
        i64::try_from(left - right).unwrap_or(i64::MAX)
    } else {
        -i64::try_from(right - left).unwrap_or(i64::MAX)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::{
        ContextItem, ContextMetadata, ContextRetention, ContextScope, ContextSource,
    };
    use crate::llm::{LlmImage, LlmMessage, LlmMessageRole, LlmToolCall};
    use crate::protocol::{AgentToolApprovalMode, AgentToolSafety};
    use serde_json::json;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn frame(messages: Vec<LlmMessage>) -> ContextFrame {
        ContextFrame::new(
            messages
                .into_iter()
                .map(|message| {
                    ContextItem::new(
                        message,
                        ContextMetadata::new(
                            ContextSource::ConversationHistory,
                            ContextScope::Conversation,
                            ContextRetention::Retained,
                        ),
                    )
                })
                .collect(),
        )
    }

    fn detector(tools: &[AgentToolDefinition]) -> ContextCapacityDetector {
        ContextCapacityDetector::for_model("model", AgentApiStyle::OpenAiCompatible, tools)
    }

    fn read_tool() -> AgentToolDefinition {
        AgentToolDefinition {
            name: "read_file".to_string(),
            description: "Read a file".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": { "path": { "type": "string" } }
            }),
            requires_approval: false,
            safety: AgentToolSafety::ReadOnly,
            requires_workspace: false,
            approval_mode: AgentToolApprovalMode::Never,
        }
    }

    #[test]
    fn reports_unconfigured_without_inventing_a_model_window() {
        let mut frame = frame(vec![LlmMessage::text(LlmMessageRole::User, "hello")]);
        let detector = detector(&[]);

        let report = detector.inspect(&mut frame, None, 1_000);

        assert_eq!(report.status, ContextBudgetStatus::Unconfigured);
        assert_eq!(report.context_window_tokens, None);
        assert!(report.usage.request_input_tokens() > 0);
        assert_eq!(
            report.usage.measurement_mode,
            ContextMeasurementMode::IncrementalCache
        );
        assert!(detector.ensure_sendable(report).is_ok());
    }

    #[test]
    fn distinguishes_within_budget_and_over_budget_requests() {
        let mut frame = frame(vec![LlmMessage::text(
            LlmMessageRole::User,
            "a".repeat(12_000),
        )]);
        let detector = detector(&[]);

        let within = detector.inspect(&mut frame, Some(16_000), 1_000);
        let over = detector.inspect(&mut frame, Some(5_000), 1_000);

        assert_eq!(within.status, ContextBudgetStatus::WithinBudget);
        assert!(within.remaining_input_tokens.is_some_and(|value| value > 0));
        assert_eq!(over.status, ContextBudgetStatus::OverBudget);
        assert!(over.excess_input_tokens.is_some_and(|value| value > 0));
        let error = detector.ensure_sendable(over).unwrap_err().to_string();
        assert!(error.contains("context_capacity_exceeded"));
        assert!(error.contains("请求尚未发送"));
    }

    #[test]
    fn reserves_output_and_five_percent_safety_from_the_total_window() {
        let mut frame = frame(vec![LlmMessage::text(LlmMessageRole::User, "hello")]);
        let detector = detector(&[]);

        let report = detector.inspect(&mut frame, Some(128_000), 30_000);

        assert_eq!(report.status, ContextBudgetStatus::WithinBudget);
        assert_eq!(report.safety_margin_tokens, 6_400);
        assert_eq!(report.available_input_tokens, Some(91_600));
        assert_eq!(
            report.maximum_output_tokens_for_current_input(),
            report.remaining_input_tokens.map(|remaining| {
                report
                    .reserved_output_tokens
                    .saturating_add(u64::try_from(remaining).unwrap())
            })
        );
    }

    #[test]
    fn tool_text_budget_uses_effective_input_capacity_and_a_twenty_four_k_cap() {
        let detector = detector(&[]);

        let compact_window = detector.tool_output_text_budget(Some(128_000), 30_000);
        let large_window = detector.tool_output_text_budget(Some(256_000), 30_000);
        let unconfigured = detector.tool_output_text_budget(None, 30_000);

        assert_eq!(compact_window.max_tokens(), 13_740);
        assert_eq!(large_window.max_tokens(), 24_000);
        assert_eq!(unconfigured.max_tokens(), 24_000);
    }

    #[test]
    fn unconfigured_window_has_no_derived_output_capacity() {
        let mut frame = frame(vec![LlmMessage::text(LlmMessageRole::User, "hello")]);
        let detector = detector(&[]);

        let report = detector.inspect(&mut frame, None, 0);

        assert_eq!(report.maximum_output_tokens_for_current_input(), None);
    }

    #[test]
    fn persistent_snapshot_does_not_follow_run_transient_growth() {
        let mut frame = frame(vec![LlmMessage::text(LlmMessageRole::User, "hello")]);
        let detector = detector(&[]);
        let initial = detector.inspect(&mut frame, Some(128_000), 30_000);
        let initial_snapshot =
            initial.persistent_snapshot("provider/model", AgentContextWindowPhase::DurableCommit);

        frame.push(ContextItem::text(
            LlmMessageRole::Assistant,
            "large transient observation ".repeat(20_000),
            ContextSource::ToolResult,
            ContextScope::Run,
            ContextRetention::Retained,
        ));
        let expanded = detector.inspect(&mut frame, Some(128_000), 30_000);
        let expanded_snapshot =
            expanded.persistent_snapshot("provider/model", AgentContextWindowPhase::DurableCommit);

        assert_eq!(initial_snapshot.model, "provider/model");
        assert_eq!(
            initial_snapshot.phase,
            AgentContextWindowPhase::DurableCommit
        );
        assert_eq!(
            expanded_snapshot.durable_input_tokens,
            initial_snapshot.durable_input_tokens
        );
        assert_eq!(
            expanded_snapshot.persistent_revision,
            initial_snapshot.persistent_revision
        );
        assert!(expanded.usage.request_input_tokens() > initial.usage.request_input_tokens());
        assert_eq!(expanded.status, ContextBudgetStatus::OverBudget);
        assert_eq!(
            expanded_snapshot.status,
            AgentContextWindowStatus::WithinBudget
        );

        frame.push(ContextItem::text(
            LlmMessageRole::Assistant,
            "persisted final answer",
            ContextSource::ConversationHistory,
            ContextScope::Conversation,
            ContextRetention::Retained,
        ));
        let committed = detector.inspect(&mut frame, Some(128_000), 30_000);
        let committed_snapshot =
            committed.persistent_snapshot("provider/model", AgentContextWindowPhase::DurableCommit);
        assert!(committed_snapshot.durable_input_tokens > expanded_snapshot.durable_input_tokens);
        assert_ne!(
            committed_snapshot.persistent_revision,
            expanded_snapshot.persistent_revision
        );
    }

    #[test]
    fn persistent_snapshot_excludes_fixed_costs_from_the_display_ratio() {
        let mut frame = ContextFrame::new(vec![
            ContextItem::text(
                LlmMessageRole::System,
                "fixed system rules ".repeat(1_000),
                ContextSource::BackendSystemPrompt,
                ContextScope::Run,
                ContextRetention::Retained,
            ),
            ContextItem::text(
                LlmMessageRole::User,
                "durable conversation",
                ContextSource::ConversationHistory,
                ContextScope::Conversation,
                ContextRetention::Retained,
            ),
        ]);
        let detector = detector(&[read_tool()]);

        let report = detector.inspect(&mut frame, Some(128_000), 30_000);
        let snapshot = report.persistent_snapshot("provider/model", AgentContextWindowPhase::Idle);

        assert_eq!(
            snapshot.durable_input_tokens,
            report.usage.breakdown.durable.input_tokens
        );
        assert_eq!(
            snapshot.durable_capacity_tokens,
            report.available_input_tokens.map(|available| {
                available.saturating_sub(report.usage.breakdown.fixed.input_tokens)
            })
        );
        assert!(
            report.usage.persistent_input_tokens() > snapshot.durable_input_tokens,
            "fixed system and tool costs must not appear as used durable history"
        );
    }

    #[test]
    fn rejects_configuration_that_leaves_no_input_capacity() {
        let mut frame = frame(vec![LlmMessage::text(LlmMessageRole::User, "hello")]);
        let detector = detector(&[]);

        let report = detector.inspect(&mut frame, Some(5_000), 4_500);

        assert_eq!(report.status, ContextBudgetStatus::InvalidConfiguration);
        let error = detector.ensure_sendable(report).unwrap_err().to_string();
        assert!(error.contains("invalid_context_capacity_configuration"));
    }

    #[test]
    fn includes_tool_schemas_calls_and_images_in_the_estimate() {
        let mut plain = frame(vec![LlmMessage::text(LlmMessageRole::User, "hello")]);
        let mut image_message = LlmMessage::text(LlmMessageRole::User, "hello");
        image_message.images.push(LlmImage {
            mime_type: "image/png".to_string(),
            data_base64: "not-counted-as-text".to_string(),
        });
        let mut rich = frame(vec![
            image_message,
            LlmMessage::assistant(
                "",
                vec![LlmToolCall {
                    id: "call-1".to_string(),
                    name: "read_file".to_string(),
                    args: json!({ "path": "notes.txt" }),
                }],
            ),
        ]);
        let tools = vec![read_tool()];
        let plain_detector = detector(&[]);
        let rich_detector = detector(&tools);

        let plain_estimate = plain_detector.inspect(&mut plain, None, 1_000).usage;
        let rich_estimate = rich_detector.inspect(&mut rich, None, 1_000).usage;

        assert_eq!(rich_estimate.breakdown.total.image_count, 1);
        assert_eq!(rich_estimate.breakdown.total.image_tokens, 4_096);
        assert_eq!(rich_estimate.image_token_reserve_per_image, 4_096);
        assert!(rich_estimate.breakdown.total.tool_call_tokens > 0);
        assert!(rich_estimate.breakdown.fixed.tool_definition_tokens > 0);
        assert_eq!(rich_estimate.breakdown.fixed.tool_definition_count, 1);
        assert!(rich_estimate.request_input_tokens() > plain_estimate.request_input_tokens());
    }

    #[test]
    fn one_classified_report_feeds_capacity_compaction_and_persistent_views() {
        let mut frame = ContextFrame::new(vec![
            ContextItem::text(
                LlmMessageRole::System,
                "system rules",
                ContextSource::BackendSystemPrompt,
                ContextScope::Run,
                ContextRetention::Retained,
            ),
            ContextItem::text(
                LlmMessageRole::User,
                "persisted question",
                ContextSource::ConversationHistory,
                ContextScope::Conversation,
                ContextRetention::Retained,
            ),
            ContextItem::text(
                LlmMessageRole::Assistant,
                "historical tool activity",
                ContextSource::ConversationTrace,
                ContextScope::Conversation,
                ContextRetention::Retained,
            ),
            ContextItem::text(
                LlmMessageRole::Assistant,
                "current run narration",
                ContextSource::ModelResponse,
                ContextScope::Run,
                ContextRetention::Retained,
            ),
            ContextItem::text(
                LlmMessageRole::System,
                "current todo state",
                ContextSource::RuntimeTodo,
                ContextScope::Run,
                ContextRetention::RequestOnly,
            ),
        ]);
        let detector = detector(&[read_tool()]);

        let report = detector.inspect(&mut frame, Some(128_000), 30_000);
        let breakdown = &report.usage.breakdown;
        let category_sum = breakdown
            .fixed
            .input_tokens
            .saturating_add(breakdown.durable.input_tokens)
            .saturating_add(breakdown.run_transient.input_tokens)
            .saturating_add(breakdown.request_only.input_tokens);

        assert_eq!(breakdown.fixed.context_item_count, 1);
        assert_eq!(breakdown.durable.context_item_count, 2);
        assert_eq!(breakdown.run_transient.context_item_count, 1);
        assert_eq!(breakdown.request_only.context_item_count, 1);
        assert_eq!(breakdown.total.input_tokens, category_sum);
        assert_eq!(breakdown.total.context_item_count, 5);
        assert_eq!(
            report.usage.persistent_input_tokens(),
            breakdown
                .fixed
                .input_tokens
                .saturating_add(breakdown.durable.input_tokens)
        );

        let compaction = report.compaction_query();
        assert_eq!(compaction.breakdown, breakdown.clone());
        assert_eq!(
            compaction.request_input_tokens,
            report.usage.request_input_tokens()
        );
        assert_eq!(
            compaction.persistent_input_tokens,
            report.usage.persistent_input_tokens()
        );
    }

    #[test]
    fn reports_bounded_context_costs_by_semantic_owner() {
        let mut frame = ContextFrame::new(vec![
            ContextItem::text(
                LlmMessageRole::System,
                "system contract",
                ContextSource::BackendSystemPrompt,
                ContextScope::Run,
                ContextRetention::Retained,
            ),
            ContextItem::text(
                LlmMessageRole::Assistant,
                "historical summary",
                ContextSource::ConversationSummary,
                ContextScope::Conversation,
                ContextRetention::Retained,
            ),
            ContextItem::text(
                LlmMessageRole::User,
                "bounded continuity refs",
                ContextSource::ContinuityIndex,
                ContextScope::Conversation,
                ContextRetention::Retained,
            ),
            ContextItem::text(
                LlmMessageRole::User,
                "workspace state",
                ContextSource::WorldStateSnapshot,
                ContextScope::Conversation,
                ContextRetention::Retained,
            ),
            ContextItem::text(
                LlmMessageRole::User,
                "explicit objective",
                ContextSource::ConversationGoal,
                ContextScope::Conversation,
                ContextRetention::Retained,
            ),
            ContextItem::text(
                LlmMessageRole::User,
                "recent conversation tail",
                ContextSource::ConversationHistory,
                ContextScope::Conversation,
                ContextRetention::Retained,
            ),
            ContextItem::text(
                LlmMessageRole::User,
                "run todo",
                ContextSource::RuntimeTodo,
                ContextScope::Run,
                ContextRetention::RequestOnly,
            ),
        ]);
        let report = detector(&[read_tool()]).inspect(&mut frame, None, 1_000);
        let costs = report.context_cost_breakdown();

        assert!(costs.system_tokens > 0);
        assert!(costs.tool_schema_tokens > 0);
        assert!(costs.summary_tokens > 0);
        assert!(costs.continuity_tokens > 0);
        assert!(costs.world_state_tokens > 0);
        assert!(costs.goal_tokens > 0);
        assert!(costs.todo_tokens > 0);
        assert!(costs.recent_history_tokens > 0);
        assert_eq!(
            costs.total_input_tokens,
            costs
                .system_tokens
                .saturating_add(costs.tool_schema_tokens)
                .saturating_add(costs.summary_tokens)
                .saturating_add(costs.continuity_tokens)
                .saturating_add(costs.world_state_tokens)
                .saturating_add(costs.goal_tokens)
                .saturating_add(costs.todo_tokens)
                .saturating_add(costs.recent_history_tokens)
        );
    }

    #[test]
    fn ordinary_conversation_has_zero_goal_and_todo_context_cost() {
        let mut frame = ContextFrame::new(vec![
            ContextItem::text(
                LlmMessageRole::System,
                "system contract",
                ContextSource::BackendSystemPrompt,
                ContextScope::Run,
                ContextRetention::Retained,
            ),
            ContextItem::text(
                LlmMessageRole::User,
                "an ordinary user message",
                ContextSource::ConversationHistory,
                ContextScope::Conversation,
                ContextRetention::Retained,
            ),
        ]);
        let report = detector(&[]).inspect(&mut frame, None, 1_000);
        let costs = report.context_cost_breakdown();

        assert_eq!(costs.goal_tokens, 0);
        assert_eq!(costs.todo_tokens, 0);
        assert!(costs.recent_history_tokens > 0);
    }

    #[test]
    fn persistent_revision_fingerprints_content_and_fixed_tool_contracts() {
        let mut original = frame(vec![LlmMessage::text(LlmMessageRole::User, "hello")]);
        let mut changed = frame(vec![LlmMessage::text(LlmMessageRole::User, "world")]);
        let mut same_with_tool = frame(vec![LlmMessage::text(LlmMessageRole::User, "hello")]);
        let plain_detector = detector(&[]);
        let tool_detector = detector(&[read_tool()]);

        let original = plain_detector
            .inspect(&mut original, Some(128_000), 30_000)
            .persistent_snapshot("model", AgentContextWindowPhase::Idle);
        let changed = plain_detector
            .inspect(&mut changed, Some(128_000), 30_000)
            .persistent_snapshot("model", AgentContextWindowPhase::Idle);
        let same_with_tool = tool_detector
            .inspect(&mut same_with_tool, Some(128_000), 30_000)
            .persistent_snapshot("model", AgentContextWindowPhase::Idle);

        assert_ne!(original.persistent_revision, changed.persistent_revision);
        assert_ne!(
            original.persistent_revision,
            same_with_tool.persistent_revision
        );
    }

    #[test]
    fn dynamic_tool_schemas_are_transient_without_changing_stable_revision() {
        let stable_tool = read_tool();
        let mut dynamic_tool = read_tool();
        dynamic_tool.name = "office_document".to_string();
        dynamic_tool.description = "Edit a Word document".to_string();
        let detector = detector(&[stable_tool]);
        let mut without_dynamic = frame(vec![LlmMessage::text(LlmMessageRole::User, "hello")]);
        let mut with_dynamic = frame(vec![LlmMessage::text(LlmMessageRole::User, "hello")]);

        let baseline = detector.inspect(&mut without_dynamic, Some(128_000), 30_000);
        let activated = detector.inspect_with_dynamic_tools(
            &mut with_dynamic,
            Some(128_000),
            30_000,
            &[dynamic_tool],
        );

        assert_eq!(
            baseline.usage.persistent_revision,
            activated.usage.persistent_revision
        );
        assert_ne!(
            baseline.usage.context_revision,
            activated.usage.context_revision
        );
        assert_eq!(
            activated
                .usage
                .breakdown
                .run_transient
                .tool_definition_count,
            1
        );
        assert!(activated.usage.request_input_tokens() > baseline.usage.request_input_tokens());
    }

    #[derive(Debug, Default)]
    struct EstimatorCounters {
        message_calls: AtomicUsize,
        tool_definition_calls: AtomicUsize,
        full_recount_calls: AtomicUsize,
    }

    #[derive(Debug)]
    struct CountingEstimator {
        variant: &'static str,
        counters: Arc<EstimatorCounters>,
        full_recount_threshold_percent: Option<u8>,
    }

    impl ContextTokenEstimator for CountingEstimator {
        fn identity(&self) -> super::super::measurement::ContextEstimatorIdentity {
            super::super::measurement::ContextEstimatorIdentity {
                family: "counting",
                version: 1,
                variant: Some(Arc::from(self.variant)),
            }
        }

        fn estimate_message(&self, message: &LlmMessage) -> ContextMessageEstimate {
            self.counters.message_calls.fetch_add(1, Ordering::SeqCst);
            HeuristicTokenEstimator.estimate_message(message)
        }

        fn estimate_messages(&self, messages: &[&LlmMessage]) -> ContextMessageEstimate {
            self.counters
                .full_recount_calls
                .fetch_add(1, Ordering::SeqCst);
            messages
                .iter()
                .fold(ContextMessageEstimate::default(), |mut total, message| {
                    total.merge(HeuristicTokenEstimator.estimate_message(message));
                    total
                })
        }

        fn estimate_tool_definitions(&self, tools: &[AgentToolDefinition]) -> u64 {
            self.counters
                .tool_definition_calls
                .fetch_add(1, Ordering::SeqCst);
            HeuristicTokenEstimator.estimate_tool_definitions(tools)
        }

        fn request_structure_tokens(&self) -> u64 {
            HeuristicTokenEstimator.request_structure_tokens()
        }

        fn image_token_reserve_per_image(&self) -> u64 {
            HeuristicTokenEstimator.image_token_reserve_per_image()
        }

        fn full_recount_threshold_percent(&self) -> Option<u8> {
            self.full_recount_threshold_percent
        }
    }

    fn counting_detector(
        variant: &'static str,
        counters: Arc<EstimatorCounters>,
        full_recount_threshold_percent: Option<u8>,
        tools: &[AgentToolDefinition],
    ) -> ContextCapacityDetector {
        ContextCapacityDetector::new(
            Arc::new(CountingEstimator {
                variant,
                counters,
                full_recount_threshold_percent,
            }),
            tools,
        )
    }

    #[test]
    fn measures_retained_items_once_and_only_measures_new_request_items() {
        let counters = Arc::new(EstimatorCounters::default());
        let tools = vec![read_tool()];
        let detector = counting_detector("a", counters.clone(), None, &tools);
        let mut retained = frame(vec![
            LlmMessage::text(LlmMessageRole::System, "rules"),
            LlmMessage::text(LlmMessageRole::User, "history"),
            LlmMessage::text(LlmMessageRole::Assistant, "answer"),
        ]);

        detector.prepare_frame(&mut retained);
        assert_eq!(counters.message_calls.load(Ordering::SeqCst), 3);
        assert_eq!(counters.tool_definition_calls.load(Ordering::SeqCst), 1);

        for revision in 0..5 {
            let mut request = retained.clone();
            request.push(ContextItem::text(
                LlmMessageRole::User,
                format!("request-only todo revision {revision}"),
                ContextSource::RuntimeExtension,
                ContextScope::Run,
                ContextRetention::RequestOnly,
            ));
            detector.inspect(&mut request, None, 1_000);
        }

        assert_eq!(counters.message_calls.load(Ordering::SeqCst), 8);
        assert_eq!(counters.tool_definition_calls.load(Ordering::SeqCst), 1);
        detector.inspect(&mut retained, None, 1_000);
        assert_eq!(counters.message_calls.load(Ordering::SeqCst), 8);

        retained.push(ContextItem::text(
            LlmMessageRole::Assistant,
            "new model response",
            ContextSource::ModelResponse,
            ContextScope::Run,
            ContextRetention::Retained,
        ));
        assert_eq!(counters.message_calls.load(Ordering::SeqCst), 9);
    }

    #[test]
    fn compaction_inventory_reuses_cached_item_measurements() {
        let counters = Arc::new(EstimatorCounters::default());
        let detector = counting_detector("planner", counters.clone(), None, &[]);
        let mut context = frame(vec![
            LlmMessage::text(LlmMessageRole::System, "rules"),
            LlmMessage::text(LlmMessageRole::User, "history"),
            LlmMessage::text(LlmMessageRole::Assistant, "answer"),
        ]);

        detector.prepare_frame(&mut context);
        let measured_calls = counters.message_calls.load(Ordering::SeqCst);
        assert_eq!(measured_calls, 3);

        let first = context.planning_items().unwrap();
        let second = context.planning_items().unwrap();

        assert_eq!(first, second);
        assert_eq!(first.len(), 3);
        assert_eq!(
            counters.message_calls.load(Ordering::SeqCst),
            measured_calls,
            "building compaction inventory must not invoke the estimator again"
        );
        assert_eq!(counters.full_recount_calls.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn shared_durable_baseline_is_not_remeasured_by_run_overlays() {
        let counters = Arc::new(EstimatorCounters::default());
        let detector = counting_detector("shared", counters.clone(), None, &[]);
        let mut durable = frame(vec![
            LlmMessage::text(LlmMessageRole::System, "rules"),
            LlmMessage::text(LlmMessageRole::User, "history"),
            LlmMessage::text(LlmMessageRole::Assistant, "answer"),
        ]);

        detector.prepare_frame(&mut durable);
        let baseline = durable.share_measured_persistent_baseline().unwrap();
        assert_eq!(counters.message_calls.load(Ordering::SeqCst), 3);

        let mut first_run = ContextFrame::from_measured_baseline(baseline.clone());
        let mut second_run = ContextFrame::from_measured_baseline(baseline);
        detector.prepare_frame(&mut first_run);
        detector.prepare_frame(&mut second_run);
        assert_eq!(counters.message_calls.load(Ordering::SeqCst), 3);

        first_run.push(ContextItem::text(
            LlmMessageRole::Assistant,
            "run narration",
            ContextSource::ModelResponse,
            ContextScope::Run,
            ContextRetention::Retained,
        ));
        second_run.push(ContextItem::text(
            LlmMessageRole::System,
            "request-only todo",
            ContextSource::RuntimeExtension,
            ContextScope::Run,
            ContextRetention::RequestOnly,
        ));
        detector.inspect(&mut first_run, None, 1_000);
        detector.inspect(&mut second_run, None, 1_000);

        assert_eq!(counters.message_calls.load(Ordering::SeqCst), 5);
    }

    #[test]
    fn checkpoint_restore_reuses_matching_durable_baseline_prefix() {
        let counters = Arc::new(EstimatorCounters::default());
        let detector = counting_detector("checkpoint-shared", counters.clone(), None, &[]);
        let durable_messages = vec![
            LlmMessage::text(LlmMessageRole::System, "rules"),
            LlmMessage::text(LlmMessageRole::User, "history"),
            LlmMessage::text(LlmMessageRole::Assistant, "answer"),
        ];
        let mut durable = frame(durable_messages.clone());
        detector.prepare_frame(&mut durable);
        let baseline = durable.share_measured_persistent_baseline().unwrap();
        assert_eq!(counters.message_calls.load(Ordering::SeqCst), 3);

        let mut checkpoint_source = frame(durable_messages);
        checkpoint_source.push(ContextItem::text(
            LlmMessageRole::Assistant,
            "run activity before approval",
            ContextSource::ModelResponse,
            ContextScope::Run,
            ContextRetention::Retained,
        ));
        let checkpoint = checkpoint_source.checkpoint_items().unwrap();
        let restored = ContextFrame::from_checkpoint_items(checkpoint).unwrap();
        let mut rebased = restored.rebase_onto_measured_baseline(baseline);

        assert_eq!(counters.message_calls.load(Ordering::SeqCst), 4);
        detector.prepare_frame(&mut rebased);
        assert_eq!(counters.message_calls.load(Ordering::SeqCst), 4);
        assert_eq!(rebased.to_messages().len(), 4);
    }

    #[test]
    fn tokenizer_switch_and_compacted_frame_rebuild_measurements_once() {
        let counters_a = Arc::new(EstimatorCounters::default());
        let counters_b = Arc::new(EstimatorCounters::default());
        let detector_a = counting_detector("a", counters_a.clone(), None, &[]);
        let detector_b = counting_detector("b", counters_b.clone(), None, &[]);
        let mut original = frame(vec![
            LlmMessage::text(LlmMessageRole::System, "rules"),
            LlmMessage::text(LlmMessageRole::User, "long history"),
            LlmMessage::text(LlmMessageRole::Assistant, "long answer"),
        ]);

        detector_a.prepare_frame(&mut original);
        detector_a.prepare_frame(&mut original);
        assert_eq!(counters_a.message_calls.load(Ordering::SeqCst), 3);

        detector_b.prepare_frame(&mut original);
        detector_b.prepare_frame(&mut original);
        assert_eq!(counters_b.message_calls.load(Ordering::SeqCst), 3);

        let mut compacted = frame(vec![LlmMessage::text(
            LlmMessageRole::System,
            "compacted summary",
        )]);
        detector_b.prepare_frame(&mut compacted);
        detector_b.prepare_frame(&mut compacted);
        assert_eq!(counters_b.message_calls.load(Ordering::SeqCst), 4);
    }

    #[test]
    fn checkpoint_restore_discards_derived_cache_and_rebuilds_it_once() {
        let counters = Arc::new(EstimatorCounters::default());
        let detector = counting_detector("a", counters.clone(), None, &[]);
        let mut original = frame(vec![
            LlmMessage::text(LlmMessageRole::System, "rules"),
            LlmMessage::text(LlmMessageRole::User, "evidence"),
        ]);
        detector.prepare_frame(&mut original);
        assert_eq!(counters.message_calls.load(Ordering::SeqCst), 2);

        let checkpoint = original.checkpoint_items().unwrap();
        let mut restored = ContextFrame::from_checkpoint_items(checkpoint).unwrap();
        detector.prepare_frame(&mut restored);
        detector.prepare_frame(&mut restored);

        assert_eq!(counters.message_calls.load(Ordering::SeqCst), 4);
    }

    #[test]
    fn full_recount_is_cached_until_the_frame_changes() {
        let counters = Arc::new(EstimatorCounters::default());
        let detector = counting_detector("boundary-sensitive", counters.clone(), Some(50), &[]);
        let mut frame = frame(vec![LlmMessage::text(
            LlmMessageRole::User,
            "x".repeat(15_000),
        )]);

        let first = detector.inspect(&mut frame, Some(10_000), 1_000);
        let second = detector.inspect(&mut frame, Some(10_000), 1_000);

        assert_eq!(
            first.usage.measurement_mode,
            ContextMeasurementMode::FullRecount
        );
        assert_eq!(
            second.usage.measurement_mode,
            ContextMeasurementMode::FullRecount
        );
        assert_eq!(counters.full_recount_calls.load(Ordering::SeqCst), 1);

        frame.push(ContextItem::text(
            LlmMessageRole::User,
            "more context",
            ContextSource::CurrentTurn,
            ContextScope::Run,
            ContextRetention::Retained,
        ));
        detector.inspect(&mut frame, Some(10_000), 1_000);
        assert_eq!(counters.full_recount_calls.load(Ordering::SeqCst), 2);
    }
}
