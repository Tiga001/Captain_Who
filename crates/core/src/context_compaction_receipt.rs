//! Durable audit receipt for one context-compaction attempt.
//!
//! A receipt records orchestration facts and token diagnostics. The immutable semantic summary
//! remains in `ContextCompactionSummary`; its content is never duplicated into this audit record.

use crate::context::{
    ContextCompactionPlan, ContextCompactionPlanStatus, ContextCompactionPrefix,
    ContextCompactionSummaryDraft, ContextJournalCursor,
};
use crate::{
    AgentApiStyle, AgentError, AgentResult, ModelRequestObservation, ModelRequestObservationStatus,
    ModelRequestPurpose,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub const CONTEXT_COMPACTION_RECEIPT_SCHEMA_VERSION: u32 = 1;
const MAXIMUM_RECEIPT_ERROR_CHARACTERS: usize = 2_000;

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ContextCompactionReceiptStatus {
    InProgress,
    Applied,
    Refreshed,
    Failed,
    Cancelled,
    Interrupted,
}

impl ContextCompactionReceiptStatus {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::InProgress => "in_progress",
            Self::Applied => "applied",
            Self::Refreshed => "refreshed",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
            Self::Interrupted => "interrupted",
        }
    }

    pub(crate) fn from_str(value: &str) -> Option<Self> {
        match value {
            "in_progress" => Some(Self::InProgress),
            "applied" => Some(Self::Applied),
            "refreshed" => Some(Self::Refreshed),
            "failed" => Some(Self::Failed),
            "cancelled" => Some(Self::Cancelled),
            "interrupted" => Some(Self::Interrupted),
            _ => None,
        }
    }

    pub fn is_terminal(self) -> bool {
        self != Self::InProgress
    }
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum ContextCompactionReceiptStage {
    Planned,
    Preparing,
    Generating,
    Committing,
    Completed,
}

impl ContextCompactionReceiptStage {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Planned => "planned",
            Self::Preparing => "preparing",
            Self::Generating => "generating",
            Self::Committing => "committing",
            Self::Completed => "completed",
        }
    }

    pub(crate) fn from_str(value: &str) -> Option<Self> {
        match value {
            "planned" => Some(Self::Planned),
            "preparing" => Some(Self::Preparing),
            "generating" => Some(Self::Generating),
            "committing" => Some(Self::Committing),
            "completed" => Some(Self::Completed),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ContextCompactionReceiptPlan {
    pub context_revision: String,
    pub persistent_revision: String,
    pub request_input_tokens: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub available_input_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request_trigger_input_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request_target_input_tokens: Option<u64>,
    pub request_pressure: bool,
    pub durable_input_tokens: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub durable_capacity_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub durable_trigger_input_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub durable_target_input_tokens: Option<u64>,
    pub durable_pressure: bool,
    pub source_input_tokens: u64,
    pub target_replacement_tokens: u64,
    pub expected_reclaimed_tokens: u64,
    pub planned_reclaimed_tokens: u64,
    pub projected_request_input_tokens: u64,
    pub projected_durable_input_tokens: u64,
    pub best_effort: bool,
    pub protected_input_tokens: u64,
    pub protected_reasons: BTreeMap<String, u64>,
    pub atomic_unit_count: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub previous_summary_id: Option<String>,
    pub covered_through: ContextJournalCursor,
}

impl ContextCompactionReceiptPlan {
    fn from_plan(plan: &ContextCompactionPlan) -> AgentResult<Self> {
        if plan.status != ContextCompactionPlanStatus::Required {
            return Err(AgentError::new(
                "只有 required 压缩计划可以创建审计 receipt。",
            ));
        }
        let step = plan
            .steps
            .first()
            .ok_or_else(|| AgentError::new("压缩计划没有可执行步骤。"))?;
        let prefix = step
            .durable_prefix
            .as_ref()
            .ok_or_else(|| AgentError::new("压缩计划没有稳定 durable 前缀。"))?;
        let durable_input_tokens = plan
            .projected_durable_input_tokens
            .saturating_add(plan.planned_reclaimed_tokens);
        let request_pressure = plan
            .soft_trigger_input_tokens
            .is_some_and(|trigger| plan.request_input_tokens >= trigger);
        let durable_pressure = plan
            .durable_trigger_input_tokens
            .is_some_and(|trigger| durable_input_tokens >= trigger);
        Ok(Self {
            context_revision: format!("{:016x}", plan.context_revision),
            persistent_revision: format!("{:016x}", plan.persistent_revision),
            request_input_tokens: plan.request_input_tokens,
            available_input_tokens: plan.available_input_tokens,
            request_trigger_input_tokens: plan.soft_trigger_input_tokens,
            request_target_input_tokens: plan.target_input_tokens,
            request_pressure,
            durable_input_tokens,
            durable_capacity_tokens: plan.durable_capacity_tokens,
            durable_trigger_input_tokens: plan.durable_trigger_input_tokens,
            durable_target_input_tokens: plan.durable_target_input_tokens,
            durable_pressure,
            source_input_tokens: step.source_input_tokens,
            target_replacement_tokens: step.target_replacement_tokens,
            expected_reclaimed_tokens: step.expected_reclaimed_tokens,
            planned_reclaimed_tokens: plan.planned_reclaimed_tokens,
            projected_request_input_tokens: plan.projected_request_input_tokens,
            projected_durable_input_tokens: plan.projected_durable_input_tokens,
            best_effort: plan.best_effort,
            protected_input_tokens: plan.protected.input_tokens,
            protected_reasons: plan.protected.reasons.clone(),
            atomic_unit_count: step.atomic_unit_count,
            previous_summary_id: prefix.previous_summary_id.clone(),
            covered_through: prefix.covered_through.clone(),
        })
    }

    fn validate(&self) -> AgentResult<()> {
        self.covered_through.validate()?;
        if self.context_revision.trim().is_empty() || self.persistent_revision.trim().is_empty() {
            return Err(AgentError::new("压缩 receipt 的上下文 revision 不能为空。"));
        }
        if !self.request_pressure && !self.durable_pressure {
            return Err(AgentError::new("压缩 receipt 没有记录有效的触发压力。"));
        }
        if self.source_input_tokens == 0
            || self.target_replacement_tokens >= self.source_input_tokens
            || self.expected_reclaimed_tokens
                != self
                    .source_input_tokens
                    .saturating_sub(self.target_replacement_tokens)
            || self.atomic_unit_count == 0
        {
            return Err(AgentError::new("压缩 receipt 的计划计量无效。"));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ContextCompactionReceiptResult {
    pub summary_id: String,
    pub source_input_tokens: u64,
    pub summary_input_tokens: u64,
    pub continuity_input_tokens: u64,
    pub replacement_input_tokens: u64,
    pub reclaimed_input_tokens: u64,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ContextCompactionReceiptError {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    pub message: String,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ContextCompactionReceipt {
    pub schema_version: u32,
    pub operation_id: String,
    pub run_id: String,
    pub conversation_id: String,
    pub assistant_message_id: String,
    pub request_index: u64,
    pub attempt_index: u64,
    pub model: String,
    pub api_style: AgentApiStyle,
    pub status: ContextCompactionReceiptStatus,
    pub stage: ContextCompactionReceiptStage,
    pub plan: ContextCompactionReceiptPlan,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_revision: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub generation_observation_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<ContextCompactionReceiptResult>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<ContextCompactionReceiptError>,
    pub started_at: i64,
    pub updated_at: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub completed_at: Option<i64>,
}

impl ContextCompactionReceipt {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn begin(
        operation_id: impl Into<String>,
        run_id: impl Into<String>,
        conversation_id: impl Into<String>,
        assistant_message_id: impl Into<String>,
        request_index: u64,
        attempt_index: u64,
        model: impl Into<String>,
        api_style: AgentApiStyle,
        plan: &ContextCompactionPlan,
        started_at: i64,
    ) -> AgentResult<Self> {
        let receipt = Self {
            schema_version: CONTEXT_COMPACTION_RECEIPT_SCHEMA_VERSION,
            operation_id: operation_id.into(),
            run_id: run_id.into(),
            conversation_id: conversation_id.into(),
            assistant_message_id: assistant_message_id.into(),
            request_index,
            attempt_index,
            model: model.into(),
            api_style,
            status: ContextCompactionReceiptStatus::InProgress,
            stage: ContextCompactionReceiptStage::Planned,
            plan: ContextCompactionReceiptPlan::from_plan(plan)?,
            source_revision: None,
            generation_observation_id: None,
            summary_id: None,
            result: None,
            error: None,
            started_at,
            updated_at: started_at,
            completed_at: None,
        };
        receipt.validate()?;
        Ok(receipt)
    }

    pub fn validate(&self) -> AgentResult<()> {
        if self.schema_version != CONTEXT_COMPACTION_RECEIPT_SCHEMA_VERSION {
            return Err(AgentError::new(format!(
                "不支持的 ContextCompactionReceipt schema version：{}。",
                self.schema_version
            )));
        }
        for (label, value) in [
            ("operation ID", self.operation_id.as_str()),
            ("run ID", self.run_id.as_str()),
            ("会话 ID", self.conversation_id.as_str()),
            ("助手消息 ID", self.assistant_message_id.as_str()),
            ("模型", self.model.as_str()),
        ] {
            if value.trim().is_empty() {
                return Err(AgentError::new(format!("压缩 receipt 的{label}不能为空。")));
            }
        }
        if self.request_index == 0 || self.attempt_index == 0 {
            return Err(AgentError::new("压缩 receipt 的请求或尝试序号无效。"));
        }
        self.plan.validate()?;
        if self.started_at < 0 || self.updated_at < self.started_at {
            return Err(AgentError::new("压缩 receipt 的时间范围无效。"));
        }
        match self.stage {
            ContextCompactionReceiptStage::Planned | ContextCompactionReceiptStage::Preparing
                if self.source_revision.is_some() =>
            {
                return Err(AgentError::new(
                    "压缩 receipt 在前缀准备完成前不能绑定 source revision。",
                ));
            }
            ContextCompactionReceiptStage::Generating
            | ContextCompactionReceiptStage::Committing
            | ContextCompactionReceiptStage::Completed
                if self.source_revision.is_none() =>
            {
                return Err(AgentError::new(
                    "压缩 receipt 在生成阶段后必须绑定 source revision。",
                ));
            }
            _ => {}
        }
        if self.stage == ContextCompactionReceiptStage::Completed
            && self.status != ContextCompactionReceiptStatus::Applied
        {
            return Err(AgentError::new(
                "只有成功应用的压缩 receipt 使用 completed 阶段。",
            ));
        }
        if self.status.is_terminal()
            != self
                .completed_at
                .is_some_and(|completed| completed >= self.started_at)
        {
            return Err(AgentError::new("压缩 receipt 的终态与完成时间不一致。"));
        }
        match self.status {
            ContextCompactionReceiptStatus::InProgress => {
                if self.error.is_some()
                    || self.result.is_some()
                    || self.summary_id.is_some()
                    || self.generation_observation_id.is_some()
                {
                    return Err(AgentError::new("进行中的压缩 receipt 包含终态数据。"));
                }
            }
            ContextCompactionReceiptStatus::Applied => {
                let result = self
                    .result
                    .as_ref()
                    .ok_or_else(|| AgentError::new("已应用的压缩 receipt 缺少结果。"))?;
                if self.stage != ContextCompactionReceiptStage::Completed
                    || self.summary_id.as_deref() != Some(result.summary_id.as_str())
                    || self.generation_observation_id.is_none()
                    || self.source_revision.is_none()
                    || self.error.is_some()
                    || result.source_input_tokens != self.plan.source_input_tokens
                    || result.replacement_input_tokens >= result.source_input_tokens
                    || result.reclaimed_input_tokens
                        != result
                            .source_input_tokens
                            .saturating_sub(result.replacement_input_tokens)
                {
                    return Err(AgentError::new("已应用的压缩 receipt 结果不一致。"));
                }
            }
            ContextCompactionReceiptStatus::Refreshed => {
                if self.result.is_some() || self.summary_id.is_some() || self.error.is_some() {
                    return Err(AgentError::new("已刷新的压缩 receipt 包含错误的终态数据。"));
                }
            }
            ContextCompactionReceiptStatus::Failed
            | ContextCompactionReceiptStatus::Cancelled
            | ContextCompactionReceiptStatus::Interrupted => {
                if self.error.is_none() || self.result.is_some() || self.summary_id.is_some() {
                    return Err(AgentError::new(
                        "失败的压缩 receipt 缺少错误或包含成功结果。",
                    ));
                }
            }
        }
        Ok(())
    }

    pub(crate) fn advance_stage(
        &mut self,
        stage: ContextCompactionReceiptStage,
        updated_at: i64,
    ) -> AgentResult<()> {
        if self.status != ContextCompactionReceiptStatus::InProgress
            || stage < self.stage
            || stage == ContextCompactionReceiptStage::Completed
            || updated_at < self.updated_at
        {
            return Err(AgentError::new("压缩 receipt 阶段迁移无效。"));
        }
        self.stage = stage;
        self.updated_at = updated_at;
        self.validate()
    }

    pub(crate) fn attach_prepared_prefix(
        &mut self,
        prefix: &ContextCompactionPrefix,
        updated_at: i64,
    ) -> AgentResult<()> {
        if prefix.conversation_id != self.conversation_id
            || prefix.covered_through != self.plan.covered_through
            || prefix
                .previous_summary
                .as_ref()
                .map(|summary| summary.id.as_str())
                != self.plan.previous_summary_id.as_deref()
        {
            return Err(AgentError::new(
                "压缩 receipt 与准备后的 durable 前缀不一致。",
            ));
        }
        self.source_revision = Some(prefix.source_revision.clone());
        self.advance_stage(ContextCompactionReceiptStage::Generating, updated_at)
    }

    pub(crate) fn complete_applied(
        &mut self,
        draft: &ContextCompactionSummaryDraft,
        observation: &ModelRequestObservation,
        completed_at: i64,
    ) -> AgentResult<()> {
        self.validate_generation_observation(observation)?;
        if observation.status != ModelRequestObservationStatus::Completed
            || self.source_revision.as_deref() != Some(draft.source_revision.as_str())
        {
            return Err(AgentError::new("压缩 receipt 与生成结果不一致。"));
        }
        self.generation_observation_id = Some(observation.id.clone());
        self.summary_id = Some(draft.id.clone());
        self.result = Some(ContextCompactionReceiptResult {
            summary_id: draft.id.clone(),
            source_input_tokens: draft.source_input_tokens,
            summary_input_tokens: draft.summary_input_tokens,
            continuity_input_tokens: draft.continuity_input_tokens,
            replacement_input_tokens: draft.replacement_input_tokens,
            reclaimed_input_tokens: draft
                .source_input_tokens
                .saturating_sub(draft.replacement_input_tokens),
        });
        self.status = ContextCompactionReceiptStatus::Applied;
        self.stage = ContextCompactionReceiptStage::Completed;
        self.updated_at = completed_at;
        self.completed_at = Some(completed_at);
        self.validate()
    }

    pub(crate) fn complete_refreshed(
        &mut self,
        observation: Option<&ModelRequestObservation>,
        completed_at: i64,
    ) -> AgentResult<()> {
        if let Some(observation) = observation {
            self.validate_generation_observation(observation)?;
            if observation.status != ModelRequestObservationStatus::Completed {
                return Err(AgentError::new(
                    "只有已完成的压缩模型请求才能产生 refreshed 结果。",
                ));
            }
            self.generation_observation_id = Some(observation.id.clone());
        }
        self.status = ContextCompactionReceiptStatus::Refreshed;
        self.updated_at = completed_at;
        self.completed_at = Some(completed_at);
        self.validate()
    }

    pub(crate) fn complete_error(
        &mut self,
        error: &AgentError,
        observation: Option<&ModelRequestObservation>,
        completed_at: i64,
    ) -> AgentResult<()> {
        if let Some(observation) = observation {
            self.validate_generation_observation(observation)?;
            self.generation_observation_id = Some(observation.id.clone());
        }
        self.status = if error.is_cancelled() {
            ContextCompactionReceiptStatus::Cancelled
        } else {
            ContextCompactionReceiptStatus::Failed
        };
        self.error = Some(ContextCompactionReceiptError {
            code: error.code().map(str::to_string),
            message: truncate_error(&error.to_string()),
        });
        self.updated_at = completed_at;
        self.completed_at = Some(completed_at);
        self.validate()
    }

    pub(crate) fn mark_interrupted(&mut self, completed_at: i64) -> AgentResult<()> {
        if self.status != ContextCompactionReceiptStatus::InProgress {
            return Ok(());
        }
        self.status = ContextCompactionReceiptStatus::Interrupted;
        self.error = Some(ContextCompactionReceiptError {
            code: Some("context_compaction_interrupted".to_string()),
            message: "应用退出时上下文压缩尚未完成。".to_string(),
        });
        self.updated_at = completed_at;
        self.completed_at = Some(completed_at);
        self.validate()
    }

    pub(crate) fn validate_generation_observation(
        &self,
        observation: &ModelRequestObservation,
    ) -> AgentResult<()> {
        observation.validate()?;
        if observation.purpose != ModelRequestPurpose::ContextCompaction
            || observation.operation_id.as_deref() != Some(self.operation_id.as_str())
            || observation.run_id != self.run_id
            || observation.conversation_id.as_deref() != Some(self.conversation_id.as_str())
            || observation.assistant_message_id.as_deref()
                != Some(self.assistant_message_id.as_str())
            || observation.model != self.model
            || observation.api_style != self.api_style
        {
            return Err(AgentError::new("压缩 receipt 与模型请求观测的身份不一致。"));
        }
        Ok(())
    }
}

fn truncate_error(value: &str) -> String {
    value
        .chars()
        .take(MAXIMUM_RECEIPT_ERROR_CHARACTERS)
        .collect()
}
