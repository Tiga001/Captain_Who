//! Provider request accounting captured at the exact send boundary.
//!
//! Observations intentionally contain measurements and provider usage only. Prompt bodies,
//! credentials and tool payloads remain outside this durable diagnostic record.

use crate::context::{ContextBudgetReport, ContextBudgetStatus, ContextMeasurementMode};
use crate::protocol::{AgentApiStyle, AgentError, AgentResult, AgentUsage};
use serde::{Deserialize, Serialize};

pub const MODEL_REQUEST_OBSERVATION_SCHEMA_VERSION: u32 = 3;
const MAXIMUM_OBSERVATION_ERROR_CHARACTERS: usize = 2_000;

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum ModelRequestPurpose {
    AgentLoop,
    ContextCompaction,
}

impl ModelRequestPurpose {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::AgentLoop => "agent_loop",
            Self::ContextCompaction => "context_compaction",
        }
    }

    pub(crate) fn from_str(value: &str) -> Option<Self> {
        match value {
            "agent_loop" => Some(Self::AgentLoop),
            "context_compaction" => Some(Self::ContextCompaction),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ModelRequestObservationStatus {
    Completed,
    Failed,
    Cancelled,
}

impl ModelRequestObservationStatus {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        }
    }

    pub(crate) fn from_str(value: &str) -> Option<Self> {
        match value {
            "completed" => Some(Self::Completed),
            "failed" => Some(Self::Failed),
            "cancelled" => Some(Self::Cancelled),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ModelRequestMeasurementMode {
    IncrementalCache,
    FullRecount,
}

impl From<ContextMeasurementMode> for ModelRequestMeasurementMode {
    fn from(value: ContextMeasurementMode) -> Self {
        match value {
            ContextMeasurementMode::IncrementalCache => Self::IncrementalCache,
            ContextMeasurementMode::FullRecount => Self::FullRecount,
        }
    }
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ModelRequestCapacityStatus {
    Unconfigured,
    WithinBudget,
    OverBudget,
    InvalidConfiguration,
}

impl From<ContextBudgetStatus> for ModelRequestCapacityStatus {
    fn from(value: ContextBudgetStatus) -> Self {
        match value {
            ContextBudgetStatus::Unconfigured => Self::Unconfigured,
            ContextBudgetStatus::WithinBudget => Self::WithinBudget,
            ContextBudgetStatus::OverBudget => Self::OverBudget,
            ContextBudgetStatus::InvalidConfiguration => Self::InvalidConfiguration,
        }
    }
}

/// Provider-owned ordering between native Tool schemas and conversational input.
///
/// This describes the request envelope only. It deliberately makes no claim that the provider
/// accepted, stored, or reused a cache entry; provider-reported usage remains the sole authority
/// for actual cache hits.
#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ProviderCacheTopology {
    /// OpenAI-compatible APIs carry `tools` and `messages` as separate top-level fields. Their
    /// internal cache ordering is provider-defined and cannot be inferred by this client.
    ProviderDefinedSeparateFields,
    /// Anthropic-compatible APIs serialize native Tools before system blocks and messages.
    ToolsBeforeSystemMessages,
}

impl ProviderCacheTopology {
    pub fn for_api_style(api_style: AgentApiStyle) -> Self {
        match api_style {
            AgentApiStyle::OpenAiCompatible => Self::ProviderDefinedSeparateFields,
            AgentApiStyle::AnthropicCompatible => Self::ToolsBeforeSystemMessages,
        }
    }
}

/// Model-visible Tool partitions frozen at one Agent-loop request boundary.
///
/// Revisions identify contracts assembled by trusted backend code. Counts are diagnostic only;
/// neither this record nor its cache topology is evidence of a provider cache hit.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ModelRequestToolSetObservation {
    pub stable_revision: String,
    pub dynamic_revision: String,
    pub effective_revision: String,
    pub stable_tool_count: u64,
    pub dynamic_tool_count: u64,
    pub provider_cache_topology: ProviderCacheTopology,
}

impl ModelRequestToolSetObservation {
    pub fn new(
        api_style: AgentApiStyle,
        stable_revision: impl Into<String>,
        dynamic_revision: impl Into<String>,
        effective_revision: impl Into<String>,
        stable_tool_count: u64,
        dynamic_tool_count: u64,
    ) -> Self {
        Self {
            stable_revision: stable_revision.into(),
            dynamic_revision: dynamic_revision.into(),
            effective_revision: effective_revision.into(),
            stable_tool_count,
            dynamic_tool_count,
            provider_cache_topology: ProviderCacheTopology::for_api_style(api_style),
        }
    }

    fn validate(&self, api_style: AgentApiStyle) -> AgentResult<()> {
        for (label, revision) in [
            ("稳定工具集", self.stable_revision.as_str()),
            ("动态工具集", self.dynamic_revision.as_str()),
            ("有效工具集", self.effective_revision.as_str()),
        ] {
            if revision.trim().is_empty() || revision.trim() != revision {
                return Err(AgentError::new(format!(
                    "模型请求观测的{label} revision 无效。"
                )));
            }
        }
        if self.provider_cache_topology != ProviderCacheTopology::for_api_style(api_style) {
            return Err(AgentError::new(
                "模型请求观测的 provider cache topology 与 API 类型不一致。",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ModelRequestEstimate {
    pub estimator_id: String,
    pub estimator_version: u32,
    pub measurement_mode: ModelRequestMeasurementMode,
    pub capacity_status: ModelRequestCapacityStatus,
    pub context_revision: String,
    pub persistent_revision: String,
    pub fixed_input_tokens: u64,
    pub durable_input_tokens: u64,
    pub run_transient_input_tokens: u64,
    pub request_only_input_tokens: u64,
    pub additive_input_tokens: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub verified_total_input_tokens: Option<u64>,
    pub estimated_input_tokens: u64,
    #[serde(default)]
    pub system_tokens: u64,
    #[serde(default)]
    pub tool_schema_tokens: u64,
    #[serde(default)]
    pub summary_tokens: u64,
    #[serde(default)]
    pub world_state_tokens: u64,
    #[serde(default)]
    pub todo_tokens: u64,
    #[serde(default)]
    pub provider_continuation_tokens: u64,
    #[serde(default)]
    pub recent_history_tokens: u64,
    #[serde(default)]
    pub total_input_tokens: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context_window_tokens: Option<u64>,
    pub reserved_output_tokens: u64,
    pub safety_margin_tokens: u64,
}

impl ModelRequestEstimate {
    pub(crate) fn from_budget_report(report: &ContextBudgetReport) -> Self {
        let breakdown = &report.usage.breakdown;
        let costs = report.context_cost_breakdown();
        Self {
            estimator_id: report.usage.estimator_id.clone(),
            estimator_version: report.usage.estimator_version,
            measurement_mode: report.usage.measurement_mode.into(),
            capacity_status: report.status.into(),
            context_revision: format!("{:016x}", report.usage.context_revision),
            persistent_revision: format!("{:016x}", report.usage.persistent_revision),
            fixed_input_tokens: breakdown.fixed.input_tokens,
            durable_input_tokens: breakdown.durable.input_tokens,
            run_transient_input_tokens: breakdown.run_transient.input_tokens,
            request_only_input_tokens: breakdown.request_only.input_tokens,
            additive_input_tokens: breakdown.total.input_tokens,
            verified_total_input_tokens: report.usage.verified_total_input_tokens,
            estimated_input_tokens: report.usage.request_input_tokens(),
            system_tokens: costs.system_tokens,
            tool_schema_tokens: costs.tool_schema_tokens,
            summary_tokens: costs.summary_tokens,
            world_state_tokens: costs.world_state_tokens,
            todo_tokens: costs.todo_tokens,
            provider_continuation_tokens: costs.provider_continuation_tokens,
            recent_history_tokens: costs.recent_history_tokens,
            total_input_tokens: costs.total_input_tokens,
            context_window_tokens: report.context_window_tokens,
            reserved_output_tokens: report.reserved_output_tokens,
            safety_margin_tokens: report.safety_margin_tokens,
        }
    }

    fn validate(&self) -> AgentResult<()> {
        if self.estimator_id.trim().is_empty() || self.estimator_version == 0 {
            return Err(AgentError::new("模型请求观测缺少有效的估算器身份。"));
        }
        if self.context_revision.trim().is_empty() || self.persistent_revision.trim().is_empty() {
            return Err(AgentError::new("模型请求观测缺少上下文 revision。"));
        }
        let additive = self
            .fixed_input_tokens
            .saturating_add(self.durable_input_tokens)
            .saturating_add(self.run_transient_input_tokens)
            .saturating_add(self.request_only_input_tokens);
        if additive != self.additive_input_tokens {
            return Err(AgentError::new("模型请求观测的分类 token 汇总不一致。"));
        }
        let expected_total = self
            .verified_total_input_tokens
            .unwrap_or(self.additive_input_tokens);
        if expected_total != self.estimated_input_tokens {
            return Err(AgentError::new("模型请求观测的总 token 估算不一致。"));
        }
        if self.total_input_tokens > 0 {
            let semantic_total = self
                .system_tokens
                .saturating_add(self.tool_schema_tokens)
                .saturating_add(self.summary_tokens)
                .saturating_add(self.world_state_tokens)
                .saturating_add(self.todo_tokens)
                .saturating_add(self.provider_continuation_tokens)
                .saturating_add(self.recent_history_tokens);
            if semantic_total != self.additive_input_tokens
                || self.total_input_tokens != self.estimated_input_tokens
            {
                return Err(AgentError::new("模型请求观测的语义 token 成本汇总不一致。"));
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ModelRequestUsageNormalization {
    OpenAiInputTokens,
    AnthropicInputPlusCache,
    Unavailable,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ModelRequestActualUsage {
    pub raw: AgentUsage,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub normalized_input_tokens: Option<u64>,
    pub normalization: ModelRequestUsageNormalization,
}

impl ModelRequestActualUsage {
    fn from_usage(api_style: AgentApiStyle, usage: AgentUsage) -> Self {
        let normalized_input_tokens = match api_style {
            AgentApiStyle::OpenAiCompatible => usage.input_tokens,
            AgentApiStyle::AnthropicCompatible => {
                let has_input = usage.input_tokens.is_some()
                    || usage.cached_input_tokens.is_some()
                    || usage.cache_creation_input_tokens.is_some();
                has_input.then(|| {
                    usage
                        .input_tokens
                        .unwrap_or(0)
                        .saturating_add(usage.cached_input_tokens.unwrap_or(0))
                        .saturating_add(usage.cache_creation_input_tokens.unwrap_or(0))
                })
            }
        };
        let normalization = match (api_style, normalized_input_tokens) {
            (_, None) => ModelRequestUsageNormalization::Unavailable,
            (AgentApiStyle::OpenAiCompatible, Some(_)) => {
                ModelRequestUsageNormalization::OpenAiInputTokens
            }
            (AgentApiStyle::AnthropicCompatible, Some(_)) => {
                ModelRequestUsageNormalization::AnthropicInputPlusCache
            }
        };
        Self {
            raw: usage,
            normalized_input_tokens,
            normalization,
        }
    }

    fn validate(&self, api_style: AgentApiStyle) -> AgentResult<()> {
        let expected = Self::from_usage(api_style, self.raw.clone());
        if &expected != self {
            return Err(AgentError::new(
                "模型请求观测的 provider usage 标准化结果不一致。",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ModelRequestObservation {
    pub schema_version: u32,
    pub id: String,
    pub run_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub conversation_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub assistant_message_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub operation_id: Option<String>,
    pub request_index: u64,
    pub purpose: ModelRequestPurpose,
    pub model: String,
    pub api_style: AgentApiStyle,
    pub status: ModelRequestObservationStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_set: Option<ModelRequestToolSetObservation>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub estimate: Option<ModelRequestEstimate>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub actual_usage: Option<ModelRequestActualUsage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub finish_reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_code: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_message: Option<String>,
    pub started_at: i64,
    pub completed_at: i64,
}

impl ModelRequestObservation {
    pub fn validate(&self) -> AgentResult<()> {
        if self.schema_version != MODEL_REQUEST_OBSERVATION_SCHEMA_VERSION {
            return Err(AgentError::new(format!(
                "不支持的 ModelRequestObservation schema version：{}。",
                self.schema_version
            )));
        }
        for (label, value) in [
            ("观测 ID", self.id.as_str()),
            ("run ID", self.run_id.as_str()),
            ("模型", self.model.as_str()),
        ] {
            if value.trim().is_empty() {
                return Err(AgentError::new(format!("模型请求{label}不能为空。")));
            }
        }
        if self.conversation_id.is_some() != self.assistant_message_id.is_some() {
            return Err(AgentError::new(
                "模型请求观测的会话 ID 与助手消息 ID 必须同时存在或同时缺省。",
            ));
        }
        if self.request_index == 0 {
            return Err(AgentError::new("模型请求观测的请求序号必须大于 0。"));
        }
        match self.purpose {
            ModelRequestPurpose::AgentLoop if self.operation_id.is_some() => {
                return Err(AgentError::new(
                    "普通 Agent 请求不能绑定压缩 operation ID。",
                ));
            }
            ModelRequestPurpose::ContextCompaction
                if self
                    .operation_id
                    .as_deref()
                    .is_none_or(|value| value.trim().is_empty()) =>
            {
                return Err(AgentError::new("压缩模型请求必须绑定 operation ID。"));
            }
            ModelRequestPurpose::ContextCompaction if self.conversation_id.is_none() => {
                return Err(AgentError::new("压缩模型请求必须绑定会话和助手消息。"));
            }
            _ => {}
        }
        if self.purpose == ModelRequestPurpose::ContextCompaction && self.tool_set.is_some() {
            return Err(AgentError::new(
                "上下文压缩模型请求不能携带 Agent 工具集观测。",
            ));
        }
        if let Some(tool_set) = &self.tool_set {
            tool_set.validate(self.api_style)?;
        }
        if let Some(estimate) = &self.estimate {
            estimate.validate()?;
        }
        if let Some(actual_usage) = &self.actual_usage {
            actual_usage.validate(self.api_style)?;
        }
        if self.started_at < 0 || self.completed_at < self.started_at {
            return Err(AgentError::new("模型请求观测的时间范围无效。"));
        }
        let has_error = self
            .error_message
            .as_deref()
            .is_some_and(|message| !message.trim().is_empty());
        if (self.status == ModelRequestObservationStatus::Completed && has_error)
            || (self.status != ModelRequestObservationStatus::Completed && !has_error)
        {
            return Err(AgentError::new("模型请求观测的状态与错误信息不一致。"));
        }
        Ok(())
    }

    pub fn normalized_actual_input_tokens(&self) -> Option<u64> {
        self.actual_usage
            .as_ref()
            .and_then(|usage| usage.normalized_input_tokens)
    }
}

#[derive(Debug, Clone)]
pub(crate) struct ModelRequestObservationBuilder {
    id: String,
    run_id: String,
    conversation_id: Option<String>,
    assistant_message_id: Option<String>,
    operation_id: Option<String>,
    request_index: u64,
    purpose: ModelRequestPurpose,
    model: String,
    api_style: AgentApiStyle,
    tool_set: Option<ModelRequestToolSetObservation>,
    estimate: Option<ModelRequestEstimate>,
    started_at: i64,
}

impl ModelRequestObservationBuilder {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        id: impl Into<String>,
        run_id: impl Into<String>,
        conversation_id: Option<String>,
        assistant_message_id: Option<String>,
        operation_id: Option<String>,
        request_index: u64,
        purpose: ModelRequestPurpose,
        model: impl Into<String>,
        api_style: AgentApiStyle,
        estimate: Option<ModelRequestEstimate>,
        started_at: i64,
    ) -> Self {
        Self {
            id: id.into(),
            run_id: run_id.into(),
            conversation_id,
            assistant_message_id,
            operation_id,
            request_index,
            purpose,
            model: model.into(),
            api_style,
            tool_set: None,
            estimate,
            started_at,
        }
    }

    pub(crate) fn with_tool_set(mut self, tool_set: ModelRequestToolSetObservation) -> Self {
        self.tool_set = Some(tool_set);
        self
    }

    pub(crate) fn completed(
        self,
        usage: Option<AgentUsage>,
        finish_reason: Option<String>,
        completed_at: i64,
    ) -> AgentResult<ModelRequestObservation> {
        self.finish(
            ModelRequestObservationStatus::Completed,
            usage,
            finish_reason,
            None,
            None,
            completed_at,
        )
    }

    pub(crate) fn failed(
        self,
        usage: Option<AgentUsage>,
        error: &AgentError,
        completed_at: i64,
    ) -> AgentResult<ModelRequestObservation> {
        let status = if error.is_cancelled() {
            ModelRequestObservationStatus::Cancelled
        } else {
            ModelRequestObservationStatus::Failed
        };
        self.finish(
            status,
            usage,
            None,
            error.code().map(str::to_string),
            Some(truncate_error(&error.to_string())),
            completed_at,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn finish(
        self,
        status: ModelRequestObservationStatus,
        usage: Option<AgentUsage>,
        finish_reason: Option<String>,
        error_code: Option<String>,
        error_message: Option<String>,
        completed_at: i64,
    ) -> AgentResult<ModelRequestObservation> {
        let observation = ModelRequestObservation {
            schema_version: MODEL_REQUEST_OBSERVATION_SCHEMA_VERSION,
            id: self.id,
            run_id: self.run_id,
            conversation_id: self.conversation_id,
            assistant_message_id: self.assistant_message_id,
            operation_id: self.operation_id,
            request_index: self.request_index,
            purpose: self.purpose,
            model: self.model,
            api_style: self.api_style,
            status,
            tool_set: self.tool_set,
            estimate: self.estimate,
            actual_usage: usage
                .map(|usage| ModelRequestActualUsage::from_usage(self.api_style, usage)),
            finish_reason,
            error_code,
            error_message,
            started_at: self.started_at,
            completed_at,
        };
        observation.validate()?;
        Ok(observation)
    }
}

fn truncate_error(value: &str) -> String {
    value
        .chars()
        .take(MAXIMUM_OBSERVATION_ERROR_CHARACTERS)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn usage(input: Option<u64>, cached: Option<u64>, created: Option<u64>) -> AgentUsage {
        AgentUsage {
            input_tokens: input,
            output_tokens: Some(10),
            output_thinking_tokens: None,
            total_tokens: None,
            cached_input_tokens: cached,
            cache_creation_input_tokens: created,
            billable_request_count: Some(1),
        }
    }

    #[test]
    fn normalizes_openai_cached_tokens_as_a_subset() {
        let actual = ModelRequestActualUsage::from_usage(
            AgentApiStyle::OpenAiCompatible,
            usage(Some(100), Some(80), None),
        );
        assert_eq!(actual.normalized_input_tokens, Some(100));
        assert_eq!(
            actual.normalization,
            ModelRequestUsageNormalization::OpenAiInputTokens
        );
    }

    #[test]
    fn normalizes_anthropic_cache_counters_as_additional_input() {
        let actual = ModelRequestActualUsage::from_usage(
            AgentApiStyle::AnthropicCompatible,
            usage(Some(20), Some(70), Some(10)),
        );
        assert_eq!(actual.normalized_input_tokens, Some(100));
        assert_eq!(
            actual.normalization,
            ModelRequestUsageNormalization::AnthropicInputPlusCache
        );
    }

    fn tool_set(api_style: AgentApiStyle) -> ModelRequestToolSetObservation {
        ModelRequestToolSetObservation::new(
            api_style,
            "stable-tool-set-v1:stable",
            "dynamic-tool-set-v1:dynamic",
            "effective-tool-set-v1:effective",
            20,
            2,
        )
    }

    fn observation_builder(
        purpose: ModelRequestPurpose,
        api_style: AgentApiStyle,
    ) -> ModelRequestObservationBuilder {
        ModelRequestObservationBuilder::new(
            "request-1",
            "run-1",
            Some("conversation-1".to_string()),
            Some("assistant-1".to_string()),
            (purpose == ModelRequestPurpose::ContextCompaction).then(|| "operation-1".to_string()),
            1,
            purpose,
            "model-a",
            api_style,
            None,
            1,
        )
    }

    #[test]
    fn records_provider_cache_topology_without_claiming_a_cache_hit() {
        let open_ai = observation_builder(
            ModelRequestPurpose::AgentLoop,
            AgentApiStyle::OpenAiCompatible,
        )
        .with_tool_set(tool_set(AgentApiStyle::OpenAiCompatible))
        .completed(None, Some("stop".to_string()), 2)
        .unwrap();
        assert_eq!(
            open_ai.tool_set.as_ref().unwrap().provider_cache_topology,
            ProviderCacheTopology::ProviderDefinedSeparateFields
        );

        let anthropic = observation_builder(
            ModelRequestPurpose::AgentLoop,
            AgentApiStyle::AnthropicCompatible,
        )
        .with_tool_set(tool_set(AgentApiStyle::AnthropicCompatible))
        .completed(None, Some("end_turn".to_string()), 2)
        .unwrap();
        assert_eq!(
            anthropic.tool_set.as_ref().unwrap().provider_cache_topology,
            ProviderCacheTopology::ToolsBeforeSystemMessages
        );

        let serialized = serde_json::to_value(open_ai).unwrap();
        assert_eq!(
            serialized["toolSet"]["providerCacheTopology"],
            "provider_defined_separate_fields"
        );
        assert!(serialized["toolSet"].get("cacheHit").is_none());
    }

    #[test]
    fn rejects_tool_set_topology_that_disagrees_with_api_style() {
        let mut mismatched = tool_set(AgentApiStyle::AnthropicCompatible);
        mismatched.provider_cache_topology = ProviderCacheTopology::ProviderDefinedSeparateFields;
        let result = observation_builder(
            ModelRequestPurpose::AgentLoop,
            AgentApiStyle::AnthropicCompatible,
        )
        .with_tool_set(mismatched)
        .completed(None, Some("end_turn".to_string()), 2);
        assert!(result.is_err());
    }

    #[test]
    fn context_compaction_cannot_carry_agent_tool_set_observation() {
        let result = observation_builder(
            ModelRequestPurpose::ContextCompaction,
            AgentApiStyle::OpenAiCompatible,
        )
        .with_tool_set(tool_set(AgentApiStyle::OpenAiCompatible))
        .completed(None, Some("stop".to_string()), 2);
        assert!(result.is_err());
    }

    #[test]
    fn current_observation_round_trips_and_previous_schema_fails_closed() {
        let current = observation_builder(
            ModelRequestPurpose::AgentLoop,
            AgentApiStyle::OpenAiCompatible,
        )
        .completed(None, Some("stop".to_string()), 2)
        .unwrap();
        let encoded = serde_json::to_value(&current).unwrap();
        assert_eq!(
            encoded["schemaVersion"],
            MODEL_REQUEST_OBSERVATION_SCHEMA_VERSION
        );
        let decoded: ModelRequestObservation = serde_json::from_value(encoded.clone()).unwrap();
        decoded.validate().unwrap();

        let mut previous = encoded;
        previous["schemaVersion"] =
            serde_json::json!(MODEL_REQUEST_OBSERVATION_SCHEMA_VERSION.saturating_sub(1));
        let decoded: ModelRequestObservation = serde_json::from_value(previous).unwrap();
        assert!(decoded.validate().is_err());
    }

    #[test]
    fn removed_semantic_breakdown_field_is_not_accepted() {
        let mut estimate = serde_json::json!({
            "estimatorId": "heuristic-v1",
            "estimatorVersion": 1,
            "measurementMode": "full_recount",
            "capacityStatus": "within_budget",
            "contextRevision": "context-1",
            "persistentRevision": "persistent-1",
            "fixedInputTokens": 0,
            "durableInputTokens": 0,
            "runTransientInputTokens": 0,
            "requestOnlyInputTokens": 0,
            "additiveInputTokens": 0,
            "estimatedInputTokens": 0,
            "systemTokens": 0,
            "toolSchemaTokens": 0,
            "summaryTokens": 0,
            "worldStateTokens": 0,
            "todoTokens": 0,
            "providerContinuationTokens": 0,
            "recentHistoryTokens": 0,
            "totalInputTokens": 0,
            "reservedOutputTokens": 0,
            "safetyMarginTokens": 0
        });
        estimate
            .as_object_mut()
            .unwrap()
            .insert("removedSemanticCategory".to_string(), serde_json::json!(0));

        assert!(serde_json::from_value::<ModelRequestEstimate>(estimate).is_err());
    }
}
