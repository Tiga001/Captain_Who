//! Versioned context-log compaction artifacts.
//!
//! Messages and conversation trace items remain the append-only audit source. A summary advances
//! one cursor over that logical log and changes only the projection sent to the model.

use crate::conversation_trace::{ConversationTurnTraceItem, ConversationTurnTraceTerminalStatus};
use crate::protocol::{AgentError, AgentResult};
use serde::{Deserialize, Serialize};

pub const CONTEXT_COMPACTION_SUMMARY_SCHEMA_VERSION: u32 = 3;

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

/// Stable position in the logical conversation context log.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum ContextJournalCursor {
    /// Covers the complete message, including a terminal assistant record when present.
    Message { message_id: String },
    /// Covers an assistant trace through one narration or one closed tool result.
    TraceItem {
        assistant_message_id: String,
        sequence: u64,
    },
}

impl ContextJournalCursor {
    pub fn message(message_id: impl Into<String>) -> Self {
        Self::Message {
            message_id: message_id.into(),
        }
    }

    pub fn trace_item(assistant_message_id: impl Into<String>, sequence: u64) -> Self {
        Self::TraceItem {
            assistant_message_id: assistant_message_id.into(),
            sequence,
        }
    }

    pub fn message_id(&self) -> &str {
        match self {
            Self::Message { message_id } => message_id,
            Self::TraceItem {
                assistant_message_id,
                ..
            } => assistant_message_id,
        }
    }

    pub fn trace_sequence(&self) -> Option<u64> {
        match self {
            Self::Message { .. } => None,
            Self::TraceItem { sequence, .. } => Some(*sequence),
        }
    }

    pub(crate) fn validate(&self) -> AgentResult<()> {
        if self.message_id().trim().is_empty() {
            return Err(AgentError::new("上下文日志游标缺少消息 ID。"));
        }
        Ok(())
    }
}

/// Exact newly covered source segment supplied to the summary model.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(
    tag = "type",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum ContextCompactionSourceItem {
    Message {
        cursor: ContextJournalCursor,
        role: String,
        content: String,
        created_at: i64,
        #[serde(skip_serializing_if = "Option::is_none")]
        status: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        terminal_status: Option<ConversationTurnTraceTerminalStatus>,
        #[serde(skip_serializing_if = "Option::is_none")]
        terminal_error: Option<String>,
    },
    TraceItem {
        cursor: ContextJournalCursor,
        run_id: String,
        item: ConversationTurnTraceItem,
    },
}

impl ContextCompactionSourceItem {
    pub fn cursor(&self) -> &ContextJournalCursor {
        match self {
            Self::Message { cursor, .. } | Self::TraceItem { cursor, .. } => cursor,
        }
    }

    pub fn is_safe_boundary(&self) -> bool {
        match self {
            Self::Message { .. } => true,
            Self::TraceItem { item, .. } => item.is_safe_compaction_boundary(),
        }
    }

    pub(crate) fn validate(&self) -> AgentResult<()> {
        self.cursor().validate()?;
        match self {
            Self::Message {
                cursor,
                role,
                created_at,
                ..
            } => {
                if !matches!(cursor, ContextJournalCursor::Message { .. }) {
                    return Err(AgentError::new("消息日志项必须使用消息游标。"));
                }
                if !matches!(role.as_str(), "user" | "assistant") {
                    return Err(AgentError::new("上下文压缩消息包含未知角色。"));
                }
                if *created_at < 0 {
                    return Err(AgentError::new("上下文压缩消息的创建时间无效。"));
                }
            }
            Self::TraceItem {
                cursor,
                run_id,
                item,
            } => {
                let ContextJournalCursor::TraceItem {
                    assistant_message_id: _,
                    sequence,
                } = cursor
                else {
                    return Err(AgentError::new("trace 日志项必须使用 trace 游标。"));
                };
                if run_id.trim().is_empty() || *sequence != item.sequence() {
                    return Err(AgentError::new("trace 日志项身份与原始记录不一致。"));
                }
            }
        }
        Ok(())
    }
}

/// Immutable summary version selected by a conversation's active compaction head.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ContextCompactionSummary {
    pub schema_version: u32,
    pub id: String,
    pub conversation_id: String,
    /// Revision of the complete raw prefix through `covered_through`.
    pub source_revision: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub previous_summary_id: Option<String>,
    pub covered_through: ContextJournalCursor,
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
        ] {
            if value.trim().is_empty() {
                return Err(AgentError::new(format!("{label}不能为空。")));
            }
        }
        self.covered_through.validate()?;
        if self.content.trim().is_empty() {
            return Err(AgentError::new("上下文压缩摘要不能为空。"));
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

/// Stable source snapshot passed to the summary generator and checked again at commit.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ContextCompactionPrefix {
    pub conversation_id: String,
    pub source_revision: String,
    pub covered_through: ContextJournalCursor,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub previous_summary: Option<ContextCompactionSummary>,
    /// Only the suffix after `previous_summary` is included. Recursive compaction consumes the
    /// previous summary plus these newly covered raw entries.
    pub source_items: Vec<ContextCompactionSourceItem>,
}

impl ContextCompactionPrefix {
    pub fn validate(&self) -> AgentResult<()> {
        if self.conversation_id.trim().is_empty() || self.source_revision.trim().is_empty() {
            return Err(AgentError::new("上下文压缩前缀缺少稳定身份。"));
        }
        self.covered_through.validate()?;
        if self.source_items.is_empty() {
            return Err(AgentError::new("新的上下文压缩必须覆盖至少一个日志项。"));
        }
        for item in &self.source_items {
            item.validate()?;
        }
        let last = self
            .source_items
            .last()
            .expect("source_items was checked as non-empty");
        if last.cursor() != &self.covered_through || !last.is_safe_boundary() {
            return Err(AgentError::new(
                "上下文压缩边界必须落在完整消息、叙述或闭合工具结果之后。",
            ));
        }
        if let Some(previous) = &self.previous_summary {
            previous.validate()?;
            if previous.conversation_id != self.conversation_id
                || previous.covered_through == self.covered_through
            {
                return Err(AgentError::new(
                    "新的上下文压缩前缀必须连续推进当前摘要游标。",
                ));
            }
        }
        Ok(())
    }
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
                "上下文压缩摘要草稿与待替换的日志前缀不匹配。",
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
            covered_through: prefix.covered_through.clone(),
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
    fn summary_can_advance_to_a_closed_tool_result_inside_an_active_turn() {
        let cursor = ContextJournalCursor::trace_item("assistant-current", 2);
        let prefix = ContextCompactionPrefix {
            conversation_id: "conversation-1".to_string(),
            source_revision: "revision-1".to_string(),
            covered_through: cursor.clone(),
            previous_summary: None,
            source_items: vec![ContextCompactionSourceItem::TraceItem {
                cursor: cursor.clone(),
                run_id: "run-1".to_string(),
                item: ConversationTurnTraceItem::ToolResult {
                    sequence: 2,
                    call_id: "call-1".to_string(),
                    tool: "read_file".to_string(),
                    status: crate::ConversationTraceToolResultStatus::Succeeded,
                    success: true,
                    observation: serde_json::json!({ "content": "file contents" }),
                    approval_status: crate::AgentApprovalStatus::NotRequired,
                    error: None,
                    truncated: false,
                },
            }],
        };
        let summary = ContextCompactionSummaryDraft {
            id: "summary-1".to_string(),
            source_revision: prefix.source_revision.clone(),
            content: "The file was read successfully.".to_string(),
            generation: ContextCompactionGeneration::test(),
            source_input_tokens: 100,
            summary_input_tokens: 20,
            created_at: 1,
        }
        .finish(&prefix)
        .unwrap();

        assert_eq!(summary.covered_through, cursor);
    }

    #[test]
    fn prefix_cannot_end_on_an_unresolved_tool_call() {
        let cursor = ContextJournalCursor::trace_item("assistant-current", 1);
        let prefix = ContextCompactionPrefix {
            conversation_id: "conversation-1".to_string(),
            source_revision: "revision-1".to_string(),
            covered_through: cursor.clone(),
            previous_summary: None,
            source_items: vec![ContextCompactionSourceItem::TraceItem {
                cursor,
                run_id: "run-1".to_string(),
                item: ConversationTurnTraceItem::ToolCall {
                    sequence: 1,
                    call_id: "call-1".to_string(),
                    tool: "read_file".to_string(),
                    operation: serde_json::json!({ "path": "README.md" }),
                    approval_status: crate::AgentApprovalStatus::NotRequired,
                    truncated: false,
                },
            }],
        };

        assert!(prefix.validate().is_err());
    }
}
