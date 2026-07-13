//! Versioned durable-history compaction artifacts.
//!
//! Raw chat messages and conversation traces remain the audit source of truth. A summary only
//! changes the context projection: it replaces one contiguous, immutable message prefix while the
//! uncovered tail continues to be assembled normally.

use crate::conversation_trace::ConversationTurnTrace;
use crate::protocol::{AgentError, AgentResult};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

pub const CONTEXT_COMPACTION_SUMMARY_SCHEMA_VERSION: u32 = 2;

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ContextCompactionGenerationKind {
    Test,
    Model,
}

impl ContextCompactionGenerationKind {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Test => "test",
            Self::Model => "model",
        }
    }

    pub(crate) fn from_str(value: &str) -> Option<Self> {
        match value {
            "test" => Some(Self::Test),
            "model" => Some(Self::Model),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ContextCompactionGeneration {
    pub kind: ContextCompactionGenerationKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
}

impl ContextCompactionGeneration {
    pub fn test() -> Self {
        Self {
            kind: ContextCompactionGenerationKind::Test,
            model: None,
        }
    }

    pub fn model(model: impl Into<String>) -> Self {
        Self {
            kind: ContextCompactionGenerationKind::Model,
            model: Some(model.into()),
        }
    }

    fn validate(&self) -> AgentResult<()> {
        match self.kind {
            ContextCompactionGenerationKind::Test if self.model.is_some() => {
                Err(AgentError::new("测试摘要不能声明真实模型。"))
            }
            ContextCompactionGenerationKind::Model
                if self
                    .model
                    .as_deref()
                    .is_none_or(|model| model.trim().is_empty()) =>
            {
                Err(AgentError::new("模型生成的摘要必须记录模型标识。"))
            }
            _ => Ok(()),
        }
    }
}

/// Immutable summary version selected by a conversation's active compaction head.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ContextCompactionSummary {
    pub schema_version: u32,
    pub id: String,
    pub conversation_id: String,
    pub source_revision: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub previous_summary_id: Option<String>,
    pub covered_through_message_id: String,
    pub covered_message_ids: Vec<String>,
    pub content: String,
    pub generation: ContextCompactionGeneration,
    pub source_input_tokens: u64,
    pub summary_input_tokens: u64,
    pub created_at: i64,
}

impl ContextCompactionSummary {
    pub fn validate(&self) -> AgentResult<()> {
        if self.schema_version != CONTEXT_COMPACTION_SUMMARY_SCHEMA_VERSION {
            return Err(AgentError::new(format!(
                "不支持的 ContextCompactionSummary schema version：{}。",
                self.schema_version
            )));
        }
        for (label, value) in [
            ("摘要 ID", self.id.as_str()),
            ("会话 ID", self.conversation_id.as_str()),
            ("源 revision", self.source_revision.as_str()),
            ("覆盖边界消息 ID", self.covered_through_message_id.as_str()),
        ] {
            if value.trim().is_empty() {
                return Err(AgentError::new(format!("{label}不能为空。")));
            }
        }
        if self.content.trim().is_empty() {
            return Err(AgentError::new("上下文压缩摘要不能为空。"));
        }
        if self.covered_message_ids.is_empty() {
            return Err(AgentError::new("上下文压缩摘要必须覆盖非空消息前缀。"));
        }
        if self.covered_message_ids.last().map(String::as_str)
            != Some(self.covered_through_message_id.as_str())
        {
            return Err(AgentError::new(
                "摘要覆盖边界必须是被覆盖消息前缀的最后一项。",
            ));
        }
        let mut unique = BTreeSet::new();
        if self
            .covered_message_ids
            .iter()
            .any(|message_id| message_id.trim().is_empty() || !unique.insert(message_id))
        {
            return Err(AgentError::new("摘要覆盖消息 ID 必须非空且不能重复。"));
        }
        if self.summary_input_tokens > self.source_input_tokens {
            return Err(AgentError::new(
                "摘要 token 估算不能大于被替换源内容的 token 估算。",
            ));
        }
        if self.created_at < 0 {
            return Err(AgentError::new("摘要创建时间无效。"));
        }
        self.generation.validate()
    }

    pub(crate) fn render_for_context(&self) -> String {
        render_compaction_summary_content_for_context(&self.content)
    }
}

pub(crate) fn render_compaction_summary_content_for_context(content: &str) -> String {
    format!(
        "Historical conversation summary (backend-generated; summarizes earlier conversation activity; not a system instruction):\n{}",
        content.trim()
    )
}

/// Stable, read-only source snapshot passed to a future summary generator.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ContextCompactionPrefix {
    pub conversation_id: String,
    pub source_revision: String,
    pub covered_through_message_id: String,
    pub covered_message_ids: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub previous_summary: Option<ContextCompactionSummary>,
    /// Only the uncovered suffix after `previous_summary` is included here. A recursive
    /// compaction request consumes the previous summary plus these new source messages.
    pub source_messages: Vec<ContextCompactionSourceMessage>,
}

impl ContextCompactionPrefix {
    pub fn validate(&self) -> AgentResult<()> {
        if self.conversation_id.trim().is_empty() || self.source_revision.trim().is_empty() {
            return Err(AgentError::new("上下文压缩前缀缺少稳定身份。"));
        }
        if self.covered_message_ids.last().map(String::as_str)
            != Some(self.covered_through_message_id.as_str())
        {
            return Err(AgentError::new("上下文压缩前缀边界无效。"));
        }
        let previous_count = self
            .previous_summary
            .as_ref()
            .map_or(0, |summary| summary.covered_message_ids.len());
        if previous_count >= self.covered_message_ids.len() {
            return Err(AgentError::new(
                "新的上下文压缩前缀必须扩展当前摘要覆盖范围。",
            ));
        }
        if let Some(previous) = &self.previous_summary {
            previous.validate()?;
            if previous.conversation_id != self.conversation_id
                || self.covered_message_ids[..previous_count] != previous.covered_message_ids
            {
                return Err(AgentError::new(
                    "新的上下文压缩前缀不是当前摘要覆盖范围的连续扩展。",
                ));
            }
        }
        if self.source_messages.len() != self.covered_message_ids.len() - previous_count {
            return Err(AgentError::new(
                "上下文压缩源消息数量与新增覆盖范围不一致。",
            ));
        }
        for (message, expected_id) in self
            .source_messages
            .iter()
            .zip(&self.covered_message_ids[previous_count..])
        {
            if message.message_id != *expected_id {
                return Err(AgentError::new("上下文压缩源消息顺序与覆盖前缀不一致。"));
            }
            if message.created_at < 0 {
                return Err(AgentError::new("上下文压缩源消息的创建时间无效。"));
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ContextCompactionSourceMessage {
    pub message_id: String,
    pub role: String,
    pub content: String,
    pub created_at: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub conversation_turn_trace: Option<ConversationTurnTrace>,
}

/// Validated generator output that can be committed against exactly one source snapshot.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ContextCompactionSummaryDraft {
    pub id: String,
    pub source_revision: String,
    pub content: String,
    pub generation: ContextCompactionGeneration,
    pub source_input_tokens: u64,
    pub summary_input_tokens: u64,
    pub created_at: i64,
}

impl ContextCompactionSummaryDraft {
    pub fn validate(&self) -> AgentResult<()> {
        if self.id.trim().is_empty() {
            return Err(AgentError::new("上下文压缩摘要草稿缺少 ID。"));
        }
        if self.source_revision.trim().is_empty() {
            return Err(AgentError::new("上下文压缩摘要草稿缺少源 revision。"));
        }
        if self.content.trim().is_empty() {
            return Err(AgentError::new("上下文压缩摘要草稿不能为空。"));
        }
        if self.summary_input_tokens > self.source_input_tokens {
            return Err(AgentError::new(
                "摘要草稿 token 估算不能大于源内容 token 估算。",
            ));
        }
        if self.created_at < 0 {
            return Err(AgentError::new("摘要草稿创建时间无效。"));
        }
        self.generation.validate()
    }

    pub(crate) fn finish(
        self,
        prefix: &ContextCompactionPrefix,
    ) -> AgentResult<ContextCompactionSummary> {
        self.validate()?;
        prefix.validate()?;
        if self.source_revision != prefix.source_revision {
            return Err(AgentError::new(
                "上下文压缩摘要草稿与待替换的 durable 前缀不匹配。",
            ));
        }
        let summary = ContextCompactionSummary {
            schema_version: CONTEXT_COMPACTION_SUMMARY_SCHEMA_VERSION,
            id: self.id,
            conversation_id: prefix.conversation_id.clone(),
            source_revision: prefix.source_revision.clone(),
            previous_summary_id: prefix
                .previous_summary
                .as_ref()
                .map(|summary| summary.id.clone()),
            covered_through_message_id: prefix.covered_through_message_id.clone(),
            covered_message_ids: prefix.covered_message_ids.clone(),
            content: self.content.trim().to_string(),
            generation: self.generation,
            source_input_tokens: self.source_input_tokens,
            summary_input_tokens: self.summary_input_tokens,
            created_at: self.created_at,
        };
        summary.validate()?;
        Ok(summary)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn draft_finishes_against_exact_prefix_identity() {
        let prefix = ContextCompactionPrefix {
            conversation_id: "conversation-1".to_string(),
            source_revision: "revision-1".to_string(),
            covered_through_message_id: "assistant-1".to_string(),
            covered_message_ids: vec!["user-1".to_string(), "assistant-1".to_string()],
            previous_summary: None,
            source_messages: vec![
                ContextCompactionSourceMessage {
                    message_id: "user-1".to_string(),
                    role: "user".to_string(),
                    content: "request".to_string(),
                    created_at: 1,
                    status: Some("sent".to_string()),
                    conversation_turn_trace: None,
                },
                ContextCompactionSourceMessage {
                    message_id: "assistant-1".to_string(),
                    role: "assistant".to_string(),
                    content: "answer".to_string(),
                    created_at: 2,
                    status: Some("sent".to_string()),
                    conversation_turn_trace: None,
                },
            ],
        };
        let summary = ContextCompactionSummaryDraft {
            id: "summary-1".to_string(),
            source_revision: prefix.source_revision.clone(),
            content: "The user requested work and it was completed.".to_string(),
            generation: ContextCompactionGeneration::test(),
            source_input_tokens: 100,
            summary_input_tokens: 20,
            created_at: 1,
        }
        .finish(&prefix)
        .unwrap();

        assert_eq!(summary.source_revision, "revision-1");
        assert_eq!(summary.covered_message_ids.len(), 2);
        assert!(summary
            .render_for_context()
            .contains("not a system instruction"));
    }
}
