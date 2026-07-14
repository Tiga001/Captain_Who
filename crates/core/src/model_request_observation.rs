//! Provider request accounting captured at the exact send boundary.
//!
//! Observations intentionally contain measurements and provider usage only. Prompt bodies,
//! credentials and tool payloads remain outside this durable diagnostic record.

use crate::context::{ContextBudgetReport, ContextBudgetStatus, ContextMeasurementMode};
use crate::protocol::{AgentApiStyle, AgentError, AgentResult, AgentUsage};
use serde::{Deserialize, Serialize};

pub const MODEL_REQUEST_OBSERVATION_SCHEMA_VERSION: u32 = 1;
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

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context_window_tokens: Option<u64>,
    pub reserved_output_tokens: u64,
    pub safety_margin_tokens: u64,
}

impl ModelRequestEstimate {
    pub(crate) fn from_budget_report(report: &ContextBudgetReport) -> Self {
        let breakdown = &report.usage.breakdown;
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
            estimate,
            started_at,
        }
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
}
