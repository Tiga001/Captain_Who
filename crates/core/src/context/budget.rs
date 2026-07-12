//! Capacity policy for measured context frames.
//!
//! `ContextFrame` owns incremental message measurements. This module adds run-stable request
//! costs (tool definitions and protocol structure), applies the configured model window, and
//! decides whether a whole-frame verification pass is required near capacity.

use super::frame::{ContextFrame, ContextFrameMeasurement};
use super::measurement::{ContextMessageEstimate, ContextTokenEstimator, HeuristicTokenEstimator};
use crate::protocol::{
    AgentApiStyle, AgentContextWindowPhase, AgentContextWindowSnapshot, AgentContextWindowSource,
    AgentContextWindowStatus, AgentError, AgentResult, AgentToolDefinition,
};
use serde::Serialize;
use std::sync::Arc;

const SAFETY_MARGIN_PERCENT: u64 = 5;
const MINIMUM_SAFETY_MARGIN_TOKENS: u64 = 1_024;

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

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ContextTokenEstimate {
    pub(crate) estimator_id: String,
    pub(crate) estimator_version: u32,
    pub(crate) measurement_mode: ContextMeasurementMode,
    pub(crate) context_item_count: usize,
    pub(crate) context_revision: u64,
    pub(crate) estimated_input_tokens: u64,
    pub(crate) message_content_tokens: u64,
    pub(crate) message_structure_tokens: u64,
    pub(crate) tool_call_tokens: u64,
    pub(crate) tool_definition_tokens: u64,
    pub(crate) tool_definition_count: usize,
    pub(crate) image_tokens: u64,
    pub(crate) image_count: usize,
    pub(crate) image_token_reserve_per_image: u64,
    pub(crate) request_structure_tokens: u64,
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
    pub(crate) estimate: ContextTokenEstimate,
}

impl ContextBudgetReport {
    pub(crate) fn snapshot(
        &self,
        model: &str,
        phase: AgentContextWindowPhase,
        source: AgentContextWindowSource,
        request_index: Option<usize>,
        used_input_tokens: Option<u64>,
    ) -> AgentContextWindowSnapshot {
        let used_input_tokens = used_input_tokens.unwrap_or(self.estimate.estimated_input_tokens);
        let (status, remaining_input_tokens) = snapshot_capacity_state(
            self.context_window_tokens,
            self.available_input_tokens,
            used_input_tokens,
        );

        AgentContextWindowSnapshot {
            model: model.to_string(),
            status,
            phase,
            source,
            context_window_tokens: self.context_window_tokens,
            reserved_output_tokens: self.reserved_output_tokens,
            safety_margin_tokens: self.safety_margin_tokens,
            available_input_tokens: self.available_input_tokens,
            used_input_tokens,
            remaining_input_tokens,
            request_index,
            context_revision: self.estimate.context_revision,
        }
    }
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
                report.estimate.estimated_input_tokens,
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
        let fixed = FixedRequestEstimate {
            tool_definition_tokens: estimator.estimate_tool_definitions(tools),
            tool_definition_count: tools.len(),
            request_structure_tokens: estimator.request_structure_tokens(),
        };
        Self { estimator, fixed }
    }

    /// Measures all existing frame items once. Subsequent `push` calls update the cached sum.
    pub(crate) fn prepare_frame(&self, frame: &mut ContextFrame) {
        frame.measure_incrementally(self.estimator.clone());
    }

    pub(crate) fn inspect(
        &self,
        frame: &mut ContextFrame,
        context_window_tokens: Option<u32>,
        reserved_output_tokens: u32,
    ) -> ContextBudgetReport {
        let incremental = frame.measure_incrementally(self.estimator.clone());
        let mut report = self.build_report(
            incremental,
            ContextMeasurementMode::IncrementalCache,
            context_window_tokens,
            reserved_output_tokens,
        );
        if should_recount(self.estimator.full_recount_threshold_percent(), &report) {
            let full = frame.measure_full(self.estimator.clone());
            report = self.build_report(
                full,
                ContextMeasurementMode::FullRecount,
                context_window_tokens,
                reserved_output_tokens,
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
    ) -> ContextBudgetReport {
        let estimate = self.token_estimate(frame, measurement_mode);
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
    ) -> ContextTokenEstimate {
        let ContextMessageEstimate {
            message_content_tokens,
            message_structure_tokens,
            tool_call_tokens,
            image_tokens,
            image_count,
        } = frame.estimate;
        let estimated_input_tokens = frame
            .estimate
            .total_tokens()
            .saturating_add(self.fixed.tool_definition_tokens)
            .saturating_add(self.fixed.request_structure_tokens);

        ContextTokenEstimate {
            estimator_id: frame.estimator.label(),
            estimator_version: frame.estimator.version,
            measurement_mode,
            context_item_count: frame.item_count,
            context_revision: frame.revision,
            estimated_input_tokens,
            message_content_tokens,
            message_structure_tokens,
            tool_call_tokens,
            tool_definition_tokens: self.fixed.tool_definition_tokens,
            tool_definition_count: self.fixed.tool_definition_count,
            image_tokens,
            image_count,
            image_token_reserve_per_image: self.estimator.image_token_reserve_per_image(),
            request_structure_tokens: self.fixed.request_structure_tokens,
        }
    }
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
            estimate,
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
            estimate,
        };
    }

    let available_input_tokens = context_window_tokens - required_reserve;
    let estimated_input_tokens = estimate.estimated_input_tokens;
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
            estimate,
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
            estimate,
        }
    }
}

fn snapshot_capacity_state(
    context_window_tokens: Option<u64>,
    available_input_tokens: Option<u64>,
    used_input_tokens: u64,
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

    let remaining_input_tokens = Some(saturating_signed_difference(
        available_input_tokens,
        used_input_tokens,
    ));
    if used_input_tokens > available_input_tokens {
        (AgentContextWindowStatus::OverBudget, remaining_input_tokens)
    } else {
        (
            AgentContextWindowStatus::WithinBudget,
            remaining_input_tokens,
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
    report.estimate.estimated_input_tokens.saturating_mul(100)
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
        assert!(report.estimate.estimated_input_tokens > 0);
        assert_eq!(
            report.estimate.measurement_mode,
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
    }

    #[test]
    fn snapshot_can_replace_estimate_with_provider_reported_input() {
        let mut frame = frame(vec![LlmMessage::text(LlmMessageRole::User, "hello")]);
        let detector = detector(&[]);
        let report = detector.inspect(&mut frame, Some(128_000), 30_000);

        let snapshot = report.snapshot(
            "provider/model",
            AgentContextWindowPhase::ModelRequest,
            AgentContextWindowSource::ProviderReported,
            Some(2),
            Some(42_000),
        );

        assert_eq!(snapshot.model, "provider/model");
        assert_eq!(snapshot.source, AgentContextWindowSource::ProviderReported);
        assert_eq!(snapshot.request_index, Some(2));
        assert_eq!(snapshot.used_input_tokens, 42_000);
        assert_eq!(snapshot.remaining_input_tokens, Some(49_600));
        assert_eq!(snapshot.status, AgentContextWindowStatus::WithinBudget);
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

        let plain_estimate = plain_detector.inspect(&mut plain, None, 1_000).estimate;
        let rich_estimate = rich_detector.inspect(&mut rich, None, 1_000).estimate;

        assert_eq!(rich_estimate.image_count, 1);
        assert_eq!(rich_estimate.image_tokens, 4_096);
        assert_eq!(rich_estimate.image_token_reserve_per_image, 4_096);
        assert!(rich_estimate.tool_call_tokens > 0);
        assert!(rich_estimate.tool_definition_tokens > 0);
        assert_eq!(rich_estimate.tool_definition_count, 1);
        assert!(rich_estimate.estimated_input_tokens > plain_estimate.estimated_input_tokens);
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
            first.estimate.measurement_mode,
            ContextMeasurementMode::FullRecount
        );
        assert_eq!(
            second.estimate.measurement_mode,
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
