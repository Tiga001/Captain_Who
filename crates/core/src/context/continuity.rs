//! Deterministic, bounded references retained beside a lossy compaction summary.
//!
//! V1 copied previews and projected metadata for nearly every historical record. V2 keeps only a
//! small backend-selected index into the authoritative message, trace and exact-history stores.
//! V1 remains readable so existing conversations can be upgraded by their next successful
//! compaction without an eager history rewrite.

use super::compaction_summary::{
    ContextCompactionPrefix, ContextCompactionSourceItem, ContextJournalCursor,
};
use crate::conversation_trace::{
    ConversationTraceToolResultStatus, ConversationTurnTraceItem,
    ConversationTurnTraceTerminalStatus,
};
use crate::protocol::{AgentApprovalStatus, AgentError, AgentResult};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet, VecDeque};

pub const CONTEXT_CONTINUITY_SCHEMA_VERSION: u32 = 2;
pub const CONTEXT_CONTINUITY_V1_SCHEMA_VERSION: u32 = 1;
pub const CONTEXT_CONTINUITY_TARGET_TOKENS: u64 = 800;
pub const CONTEXT_CONTINUITY_HARD_MAX_TOKENS: u64 = 1_500;

const MAX_TASK_EVIDENCE_REFS: usize = 6;
const MAX_UNRESOLVED_FAILURE_REFS: usize = 6;
const MAX_APPROVAL_REFS: usize = 4;
const MAX_IMPORTANT_DECISION_REFS: usize = 4;
const MAX_RECENT_REFS: usize = 8;
const MAX_REF_ID_BYTES: usize = 512;
const MAX_ARCHIVED_COUNT_KEYS: usize = 12;

const COUNT_MESSAGES: &str = "messages";
const COUNT_TRACE_ITEMS: &str = "trace_items";
const COUNT_TOOL_CALLS: &str = "tool_calls";
const COUNT_TOOL_RESULTS: &str = "tool_results";
const COUNT_FAILURES: &str = "failures";
const COUNT_APPROVALS: &str = "approvals";
const COUNT_GUIDANCE: &str = "guidance";
const COUNT_NARRATION: &str = "narration";
const COUNT_ARCHIVED_RESULTS: &str = "archived_results";

const ALLOWED_COUNT_KEYS: [&str; 9] = [
    COUNT_MESSAGES,
    COUNT_TRACE_ITEMS,
    COUNT_TOOL_CALLS,
    COUNT_TOOL_RESULTS,
    COUNT_FAILURES,
    COUNT_APPROVALS,
    COUNT_GUIDANCE,
    COUNT_NARRATION,
    COUNT_ARCHIVED_RESULTS,
];

/// The V2 continuity index. `entries` exists only for deserializing an old V1 snapshot and is
/// omitted from every V2 serialization.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ContextContinuitySnapshot {
    pub schema_version: u32,
    pub covered_through: ContextJournalCursor,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub task_evidence_refs: Vec<ContextHistoryRef>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub unresolved_failure_refs: Vec<ContextHistoryRef>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub approval_refs: Vec<ContextHistoryRef>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub important_decision_refs: Vec<ContextHistoryRef>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub recent_refs: Vec<ContextHistoryRef>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub archived_counts: BTreeMap<String, u64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub entries: Vec<ContextContinuityEntry>,
}

/// Public name for consumers that want to explicitly bind to the V2 contract.
pub type ContinuityIndexV2 = ContextContinuitySnapshot;

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq, Hash)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum ContextHistoryRef {
    Message {
        message_id: String,
    },
    TraceItem {
        assistant_message_id: String,
        sequence: u64,
    },
    Archive {
        archive_ref: String,
    },
}

/// V1 compatibility payload. New snapshots never create these entries.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ContextContinuityText {
    pub text: String,
    pub total_chars: usize,
    pub content_revision: String,
    pub truncated: bool,
}

/// V1 compatibility payload. New snapshots never create these entries.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(
    tag = "type",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum ContextContinuityEntry {
    UserMessage {
        cursor: ContextJournalCursor,
        created_at: String,
        text: ContextContinuityText,
        #[serde(skip_serializing_if = "Option::is_none")]
        status: Option<String>,
    },
    AssistantMessage {
        cursor: ContextJournalCursor,
        created_at: String,
        text: ContextContinuityText,
        #[serde(skip_serializing_if = "Option::is_none")]
        status: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        terminal_status: Option<ConversationTurnTraceTerminalStatus>,
        #[serde(skip_serializing_if = "Option::is_none")]
        terminal_error: Option<String>,
    },
    AssistantNarration {
        cursor: ContextJournalCursor,
        run_id: String,
        created_at: String,
        preview: String,
        total_chars: usize,
        truncated: bool,
    },
    ToolExchange {
        call_cursor: ContextJournalCursor,
        result_cursor: ContextJournalCursor,
        run_id: String,
        created_at: String,
        call_id: String,
        tool: String,
        operation: Value,
        status: ConversationTraceToolResultStatus,
        success: bool,
        outcome: Value,
        approval_status: AgentApprovalStatus,
        #[serde(skip_serializing_if = "Option::is_none")]
        error: Option<String>,
        truncated: bool,
    },
}

impl ContextContinuitySnapshot {
    pub fn from_prefix(prefix: &ContextCompactionPrefix) -> AgentResult<Self> {
        prefix.validate()?;
        let mut selector = ContinuitySelector::from_previous(prefix.previous_summary.as_ref())?;

        for source in &prefix.source_items {
            selector.observe(source);
        }

        let snapshot = selector.finish(prefix.covered_through.clone());
        snapshot.validate()?;
        Ok(snapshot)
    }

    pub fn validate(&self) -> AgentResult<()> {
        self.covered_through.validate()?;
        match self.schema_version {
            CONTEXT_CONTINUITY_V1_SCHEMA_VERSION => self.validate_v1(),
            CONTEXT_CONTINUITY_SCHEMA_VERSION => self.validate_v2(),
            version => Err(AgentError::new(format!(
                "不支持的 ContextContinuitySnapshot schema version：{version}。"
            ))),
        }
    }

    pub fn is_v2(&self) -> bool {
        self.schema_version == CONTEXT_CONTINUITY_SCHEMA_VERSION
    }

    pub fn all_refs(&self) -> impl Iterator<Item = &ContextHistoryRef> {
        self.task_evidence_refs
            .iter()
            .chain(&self.unresolved_failure_refs)
            .chain(&self.approval_refs)
            .chain(&self.important_decision_refs)
            .chain(&self.recent_refs)
    }

    pub(crate) fn render_json(&self) -> AgentResult<String> {
        self.validate()?;
        serde_json::to_string(self)
            .map_err(|error| AgentError::new(format!("无法序列化上下文连续性索引：{error}")))
    }

    fn validate_v2(&self) -> AgentResult<()> {
        if !self.entries.is_empty() {
            return Err(AgentError::new("Continuity V2 不能携带 V1 entries。"));
        }
        for (label, refs, maximum) in [
            (
                "taskEvidenceRefs",
                self.task_evidence_refs.as_slice(),
                MAX_TASK_EVIDENCE_REFS,
            ),
            (
                "unresolvedFailureRefs",
                self.unresolved_failure_refs.as_slice(),
                MAX_UNRESOLVED_FAILURE_REFS,
            ),
            (
                "approvalRefs",
                self.approval_refs.as_slice(),
                MAX_APPROVAL_REFS,
            ),
            (
                "importantDecisionRefs",
                self.important_decision_refs.as_slice(),
                MAX_IMPORTANT_DECISION_REFS,
            ),
            ("recentRefs", self.recent_refs.as_slice(), MAX_RECENT_REFS),
        ] {
            if refs.len() > maximum {
                return Err(AgentError::new(format!(
                    "Continuity V2 的 {label} 超过固定上限 {maximum}。"
                )));
            }
        }
        let mut unique = BTreeSet::new();
        for reference in self.all_refs() {
            reference.validate_identity()?;
            if !unique.insert(reference.key()) {
                return Err(AgentError::new("Continuity V2 包含重复历史引用。"));
            }
        }
        if self.archived_counts.len() > MAX_ARCHIVED_COUNT_KEYS
            || self
                .archived_counts
                .keys()
                .any(|key| !ALLOWED_COUNT_KEYS.contains(&key.as_str()))
        {
            return Err(AgentError::new(
                "Continuity V2 archivedCounts 包含非受控或过多的计数键。",
            ));
        }
        Ok(())
    }

    fn validate_v1(&self) -> AgentResult<()> {
        if self.entries.is_empty() {
            return Err(AgentError::new("V1 上下文连续性骨架不能为空。"));
        }
        if !self.task_evidence_refs.is_empty()
            || !self.unresolved_failure_refs.is_empty()
            || !self.approval_refs.is_empty()
            || !self.important_decision_refs.is_empty()
            || !self.recent_refs.is_empty()
            || !self.archived_counts.is_empty()
        {
            return Err(AgentError::new("V1 连续性记录不能混入 V2 字段。"));
        }
        let mut references = BTreeSet::new();
        for entry in &self.entries {
            entry.validate_v1()?;
            for cursor in entry.cursors() {
                if !references.insert(cursor_key(cursor)) {
                    return Err(AgentError::new("V1 连续性记录包含重复的原始记录引用。"));
                }
            }
        }
        if self
            .entries
            .last()
            .and_then(ContextContinuityEntry::last_cursor)
            != Some(&self.covered_through)
        {
            return Err(AgentError::new("V1 连续性记录末尾与摘要覆盖边界不一致。"));
        }
        Ok(())
    }
}

impl ContextHistoryRef {
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

    pub fn archive(archive_ref: impl Into<String>) -> Self {
        Self::Archive {
            archive_ref: archive_ref.into(),
        }
    }

    pub(crate) fn validate_identity(&self) -> AgentResult<()> {
        let value = match self {
            Self::Message { message_id } => message_id,
            Self::TraceItem {
                assistant_message_id,
                ..
            } => assistant_message_id,
            Self::Archive { archive_ref } => archive_ref,
        };
        if value.trim().is_empty() || value.len() > MAX_REF_ID_BYTES {
            return Err(AgentError::new("Continuity V2 历史引用身份无效。"));
        }
        Ok(())
    }

    fn key(&self) -> String {
        match self {
            Self::Message { message_id } => format!("message:{message_id}"),
            Self::TraceItem {
                assistant_message_id,
                sequence,
            } => format!("trace:{assistant_message_id}:{sequence}"),
            Self::Archive { archive_ref } => format!("archive:{archive_ref}"),
        }
    }
}

#[derive(Default)]
struct ContinuitySelector {
    task_evidence: VecDeque<ContextHistoryRef>,
    unresolved_failures: VecDeque<ContextHistoryRef>,
    approvals: VecDeque<ContextHistoryRef>,
    important_decisions: VecDeque<ContextHistoryRef>,
    recent: VecDeque<ContextHistoryRef>,
    archived_counts: BTreeMap<String, u64>,
}

impl ContinuitySelector {
    fn from_previous(
        previous: Option<&super::compaction_summary::ContextCompactionSummary>,
    ) -> AgentResult<Self> {
        let Some(previous) = previous else {
            return Ok(Self::default());
        };
        previous.continuity.validate()?;
        let mut selector = Self::default();
        if previous.continuity.is_v2() {
            selector.task_evidence = previous.continuity.task_evidence_refs.clone().into();
            selector.unresolved_failures =
                previous.continuity.unresolved_failure_refs.clone().into();
            selector.approvals = previous.continuity.approval_refs.clone().into();
            selector.important_decisions =
                previous.continuity.important_decision_refs.clone().into();
            selector.recent = previous.continuity.recent_refs.clone().into();
            selector.archived_counts = previous.continuity.archived_counts.clone();
        } else {
            selector.import_v1(&previous.continuity.entries);
        }
        Ok(selector)
    }

    fn import_v1(&mut self, entries: &[ContextContinuityEntry]) {
        for entry in entries {
            match entry {
                ContextContinuityEntry::UserMessage { cursor, .. } => {
                    increment(&mut self.archived_counts, COUNT_MESSAGES);
                    push_bounded(
                        &mut self.task_evidence,
                        history_ref_from_cursor(cursor),
                        MAX_TASK_EVIDENCE_REFS,
                    );
                }
                ContextContinuityEntry::AssistantMessage {
                    cursor,
                    terminal_status,
                    terminal_error,
                    ..
                } => {
                    increment(&mut self.archived_counts, COUNT_MESSAGES);
                    let reference = history_ref_from_cursor(cursor);
                    if terminal_error.is_some()
                        || terminal_status.is_some_and(|status| {
                            matches!(
                                status,
                                ConversationTurnTraceTerminalStatus::Failed
                                    | ConversationTurnTraceTerminalStatus::Cancelled
                            )
                        })
                    {
                        increment(&mut self.archived_counts, COUNT_FAILURES);
                        push_bounded(
                            &mut self.unresolved_failures,
                            reference,
                            MAX_UNRESOLVED_FAILURE_REFS,
                        );
                    } else {
                        push_bounded(&mut self.recent, reference, MAX_RECENT_REFS);
                    }
                }
                ContextContinuityEntry::AssistantNarration { .. } => {
                    increment(&mut self.archived_counts, COUNT_NARRATION);
                    increment(&mut self.archived_counts, COUNT_TRACE_ITEMS);
                }
                ContextContinuityEntry::ToolExchange {
                    result_cursor,
                    tool,
                    status,
                    approval_status,
                    ..
                } => {
                    if is_continuity_excluded_tool(tool) {
                        continue;
                    }
                    increment(&mut self.archived_counts, COUNT_TOOL_CALLS);
                    increment(&mut self.archived_counts, COUNT_TOOL_RESULTS);
                    increment_by(&mut self.archived_counts, COUNT_TRACE_ITEMS, 2);
                    let reference = history_ref_from_cursor(result_cursor);
                    if *status != ConversationTraceToolResultStatus::Succeeded {
                        increment(&mut self.archived_counts, COUNT_FAILURES);
                        push_bounded(
                            &mut self.unresolved_failures,
                            reference.clone(),
                            MAX_UNRESOLVED_FAILURE_REFS,
                        );
                    }
                    if *approval_status != AgentApprovalStatus::NotRequired {
                        increment(&mut self.archived_counts, COUNT_APPROVALS);
                        push_bounded(&mut self.approvals, reference.clone(), MAX_APPROVAL_REFS);
                    }
                    push_bounded(&mut self.recent, reference, MAX_RECENT_REFS);
                }
            }
        }
    }

    fn observe(&mut self, source: &ContextCompactionSourceItem) {
        match source {
            ContextCompactionSourceItem::Message {
                cursor,
                role,
                content,
                terminal_status,
                terminal_error,
                ..
            } => {
                increment(&mut self.archived_counts, COUNT_MESSAGES);
                let reference = history_ref_from_cursor(cursor);
                if role == "user" {
                    push_bounded(
                        &mut self.task_evidence,
                        reference.clone(),
                        MAX_TASK_EVIDENCE_REFS,
                    );
                    if looks_like_explicit_correction_or_decision(content) {
                        push_bounded(
                            &mut self.important_decisions,
                            reference.clone(),
                            MAX_IMPORTANT_DECISION_REFS,
                        );
                    }
                    push_bounded(&mut self.recent, reference, MAX_RECENT_REFS);
                } else if terminal_error.is_some()
                    || terminal_status.is_some_and(|status| {
                        matches!(
                            status,
                            ConversationTurnTraceTerminalStatus::Failed
                                | ConversationTurnTraceTerminalStatus::Cancelled
                        )
                    })
                {
                    increment(&mut self.archived_counts, COUNT_FAILURES);
                    push_bounded(
                        &mut self.unresolved_failures,
                        reference.clone(),
                        MAX_UNRESOLVED_FAILURE_REFS,
                    );
                    push_bounded(&mut self.recent, reference, MAX_RECENT_REFS);
                }
            }
            ContextCompactionSourceItem::TraceItem { cursor, item, .. } => {
                if matches!(
                    &**item,
                    ConversationTurnTraceItem::CommandSessionLifecycle { .. }
                ) {
                    // Session lifecycle remains available in durable Trace/Exact History but is
                    // operational audit metadata, not semantic continuity evidence.
                    return;
                }
                if matches!(
                    &**item,
                    ConversationTurnTraceItem::ToolCall { tool, .. }
                        | ConversationTurnTraceItem::ToolResult { tool, .. }
                        if is_continuity_excluded_tool(tool)
                ) {
                    return;
                }
                increment(&mut self.archived_counts, COUNT_TRACE_ITEMS);
                match &**item {
                    ConversationTurnTraceItem::AssistantNarration { .. } => {
                        increment(&mut self.archived_counts, COUNT_NARRATION);
                    }
                    ConversationTurnTraceItem::UserGuidance { .. } => {
                        increment(&mut self.archived_counts, COUNT_GUIDANCE);
                        let reference = history_ref_from_cursor(cursor);
                        push_bounded(
                            &mut self.important_decisions,
                            reference.clone(),
                            MAX_IMPORTANT_DECISION_REFS,
                        );
                        push_bounded(
                            &mut self.task_evidence,
                            reference.clone(),
                            MAX_TASK_EVIDENCE_REFS,
                        );
                        push_bounded(&mut self.recent, reference, MAX_RECENT_REFS);
                    }
                    ConversationTurnTraceItem::ToolCall {
                        approval_status, ..
                    } => {
                        increment(&mut self.archived_counts, COUNT_TOOL_CALLS);
                        if *approval_status != AgentApprovalStatus::NotRequired {
                            increment(&mut self.archived_counts, COUNT_APPROVALS);
                            push_bounded(
                                &mut self.approvals,
                                history_ref_from_cursor(cursor),
                                MAX_APPROVAL_REFS,
                            );
                        }
                    }
                    ConversationTurnTraceItem::ToolResult {
                        status,
                        approval_status,
                        archive,
                        ..
                    } => {
                        increment(&mut self.archived_counts, COUNT_TOOL_RESULTS);
                        let reference = archive
                            .archive_ref
                            .as_deref()
                            .map(ContextHistoryRef::archive)
                            .unwrap_or_else(|| history_ref_from_cursor(cursor));
                        if archive.archive_ref.is_some() {
                            increment(&mut self.archived_counts, COUNT_ARCHIVED_RESULTS);
                        }
                        if *status != ConversationTraceToolResultStatus::Succeeded {
                            increment(&mut self.archived_counts, COUNT_FAILURES);
                            push_bounded(
                                &mut self.unresolved_failures,
                                reference.clone(),
                                MAX_UNRESOLVED_FAILURE_REFS,
                            );
                        }
                        if *approval_status != AgentApprovalStatus::NotRequired {
                            increment(&mut self.archived_counts, COUNT_APPROVALS);
                            push_bounded(&mut self.approvals, reference.clone(), MAX_APPROVAL_REFS);
                        }
                        push_bounded(&mut self.recent, reference, MAX_RECENT_REFS);
                    }
                    ConversationTurnTraceItem::CommandSessionLifecycle { .. } => {}
                }
            }
        }
    }

    fn finish(self, covered_through: ContextJournalCursor) -> ContextContinuitySnapshot {
        let mut used = BTreeSet::new();
        let unresolved_failure_refs = take_unique(
            self.unresolved_failures,
            MAX_UNRESOLVED_FAILURE_REFS,
            &mut used,
        );
        let approval_refs = take_unique(self.approvals, MAX_APPROVAL_REFS, &mut used);
        let important_decision_refs = take_unique(
            self.important_decisions,
            MAX_IMPORTANT_DECISION_REFS,
            &mut used,
        );
        let task_evidence_refs = take_unique(self.task_evidence, MAX_TASK_EVIDENCE_REFS, &mut used);
        let recent_refs = take_unique(self.recent, MAX_RECENT_REFS, &mut used);
        ContextContinuitySnapshot {
            schema_version: CONTEXT_CONTINUITY_SCHEMA_VERSION,
            covered_through,
            task_evidence_refs,
            unresolved_failure_refs,
            approval_refs,
            important_decision_refs,
            recent_refs,
            archived_counts: self.archived_counts,
            entries: Vec::new(),
        }
    }
}

fn take_unique(
    values: VecDeque<ContextHistoryRef>,
    maximum: usize,
    used: &mut BTreeSet<String>,
) -> Vec<ContextHistoryRef> {
    values
        .into_iter()
        .rev()
        .filter(|reference| used.insert(reference.key()))
        .take(maximum)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect()
}

fn push_bounded(
    values: &mut VecDeque<ContextHistoryRef>,
    reference: ContextHistoryRef,
    maximum: usize,
) {
    let key = reference.key();
    if let Some(index) = values.iter().position(|existing| existing.key() == key) {
        values.remove(index);
    }
    values.push_back(reference);
    while values.len() > maximum {
        values.pop_front();
    }
}

fn history_ref_from_cursor(cursor: &ContextJournalCursor) -> ContextHistoryRef {
    match cursor {
        ContextJournalCursor::Message { message_id } => ContextHistoryRef::message(message_id),
        ContextJournalCursor::TraceItem {
            assistant_message_id,
            sequence,
        } => ContextHistoryRef::trace_item(assistant_message_id, *sequence),
    }
}

fn is_continuity_excluded_tool(tool: &str) -> bool {
    matches!(tool, "conversation_history" | "todo_update")
}

fn increment(counts: &mut BTreeMap<String, u64>, key: &str) {
    increment_by(counts, key, 1);
}

fn increment_by(counts: &mut BTreeMap<String, u64>, key: &str, amount: u64) {
    let value = counts.entry(key.to_string()).or_default();
    *value = value.saturating_add(amount);
}

fn looks_like_explicit_correction_or_decision(content: &str) -> bool {
    let normalized = content.to_lowercase();
    [
        "更正",
        "纠正",
        "改为",
        "不要",
        "必须",
        "决定",
        "批准",
        "correction",
        "instead",
        "must",
        "do not",
        "decided",
        "approved",
    ]
    .iter()
    .any(|marker| normalized.contains(marker))
}

impl ContextContinuityEntry {
    fn cursors(&self) -> Vec<&ContextJournalCursor> {
        match self {
            Self::UserMessage { cursor, .. }
            | Self::AssistantMessage { cursor, .. }
            | Self::AssistantNarration { cursor, .. } => vec![cursor],
            Self::ToolExchange {
                call_cursor,
                result_cursor,
                ..
            } => vec![call_cursor, result_cursor],
        }
    }

    fn last_cursor(&self) -> Option<&ContextJournalCursor> {
        match self {
            Self::UserMessage { cursor, .. }
            | Self::AssistantMessage { cursor, .. }
            | Self::AssistantNarration { cursor, .. } => Some(cursor),
            Self::ToolExchange { result_cursor, .. } => Some(result_cursor),
        }
    }

    fn validate_v1(&self) -> AgentResult<()> {
        for cursor in self.cursors() {
            cursor.validate()?;
        }
        match self {
            Self::UserMessage {
                created_at, text, ..
            }
            | Self::AssistantMessage {
                created_at, text, ..
            } => {
                validate_v1_identity("createdAt", created_at)?;
                text.validate_v1()
            }
            Self::AssistantNarration {
                run_id,
                created_at,
                preview,
                total_chars,
                ..
            } => {
                validate_v1_identity("runId", run_id)?;
                validate_v1_identity("createdAt", created_at)?;
                if preview.trim().is_empty() || preview.chars().count() > *total_chars {
                    return Err(AgentError::new("V1 连续性叙述预览无效。"));
                }
                Ok(())
            }
            Self::ToolExchange {
                run_id,
                created_at,
                call_id,
                tool,
                status,
                success,
                ..
            } => {
                for (label, value) in [
                    ("runId", run_id.as_str()),
                    ("createdAt", created_at.as_str()),
                    ("callId", call_id.as_str()),
                    ("tool", tool.as_str()),
                ] {
                    validate_v1_identity(label, value)?;
                }
                if *success != (*status == ConversationTraceToolResultStatus::Succeeded) {
                    return Err(AgentError::new("V1 工具结果 success/status 不一致。"));
                }
                Ok(())
            }
        }
    }
}

impl ContextContinuityText {
    fn validate_v1(&self) -> AgentResult<()> {
        if self.content_revision.trim().is_empty()
            || self.text.chars().count() > self.total_chars
            || self.truncated != (self.text.chars().count() < self.total_chars)
        {
            return Err(AgentError::new("V1 连续性消息文本投影无效。"));
        }
        Ok(())
    }
}

fn validate_v1_identity(label: &str, value: &str) -> AgentResult<()> {
    if value.trim().is_empty() {
        Err(AgentError::new(format!("V1 连续性记录缺少 {label}。")))
    } else {
        Ok(())
    }
}

fn cursor_key(cursor: &ContextJournalCursor) -> String {
    match cursor {
        ContextJournalCursor::Message { message_id } => format!("message:{message_id}"),
        ContextJournalCursor::TraceItem {
            assistant_message_id,
            sequence,
        } => format!("trace:{assistant_message_id}:{sequence}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::{
        ContextCapacityDetector, ContextCompactionGeneration, ContextCompactionSummary,
        ContextFrame, ContextItem, ContextRetention, ContextScope, ContextSource,
        CONTEXT_COMPACTION_SUMMARY_SCHEMA_VERSION,
    };
    use crate::llm::LlmMessageRole;
    use crate::AgentApiStyle;
    use serde_json::json;

    fn tool_exchange(
        index: u64,
        status: ConversationTraceToolResultStatus,
    ) -> Vec<ContextCompactionSourceItem> {
        vec![
            ContextCompactionSourceItem::TraceItem {
                cursor: ContextJournalCursor::trace_item("assistant-1", index * 2),
                run_id: "run-1".to_string(),
                created_at: 2_000,
                item: Box::new(ConversationTurnTraceItem::ToolCall {
                    sequence: index * 2,
                    call_id: format!("call-{index}"),
                    tool: "read_file".to_string(),
                    provenance: None,
                    operation: json!({ "path": format!("file-{index}.txt") }),
                    approval_status: AgentApprovalStatus::NotRequired,
                    truncated: false,
                }),
            },
            ContextCompactionSourceItem::TraceItem {
                cursor: ContextJournalCursor::trace_item("assistant-1", index * 2 + 1),
                run_id: "run-1".to_string(),
                created_at: 2_000,
                item: Box::new(ConversationTurnTraceItem::ToolResult {
                    sequence: index * 2 + 1,
                    call_id: format!("call-{index}"),
                    tool: "read_file".to_string(),
                    status,
                    success: status == ConversationTraceToolResultStatus::Succeeded,
                    observation: json!({ "path": format!("file-{index}.txt") }),
                    approval_status: AgentApprovalStatus::NotRequired,
                    error: (status != ConversationTraceToolResultStatus::Succeeded)
                        .then(|| "failed".to_string()),
                    truncated: false,
                    archive: Default::default(),
                }),
            },
        ]
    }

    fn estimated_tokens(content: &str) -> u64 {
        let mut frame = ContextFrame::new(vec![ContextItem::text(
            LlmMessageRole::Assistant,
            content,
            ContextSource::ConversationSummary,
            ContextScope::Conversation,
            ContextRetention::Retained,
        )]);
        let detector =
            ContextCapacityDetector::for_model("gpt-5.4", AgentApiStyle::OpenAiCompatible, &[]);
        detector.prepare_frame(&mut frame);
        frame.planning_items().unwrap()[0].estimated_tokens
    }

    #[test]
    fn five_hundred_tool_calls_stay_bounded_and_do_not_copy_success_metadata() {
        let mut source_items = vec![ContextCompactionSourceItem::Message {
            cursor: ContextJournalCursor::message("user-1"),
            role: "user".to_string(),
            content: "inspect the workspace".to_string(),
            created_at: 1_000,
            status: Some("sent".to_string()),
            terminal_status: None,
            terminal_error: None,
        }];
        for index in 0..500 {
            source_items.extend(tool_exchange(
                index,
                ConversationTraceToolResultStatus::Succeeded,
            ));
        }
        let covered_through = ContextJournalCursor::trace_item("assistant-1", 999);
        let snapshot = ContextContinuitySnapshot::from_prefix(&ContextCompactionPrefix {
            conversation_id: "conversation-1".to_string(),
            source_revision: "source-1".to_string(),
            covered_through,
            previous_summary: None,
            source_items,
        })
        .unwrap();
        let rendered = snapshot.render_json().unwrap();

        assert_eq!(snapshot.schema_version, 2);
        assert!(snapshot.all_refs().count() <= 28);
        assert_eq!(snapshot.archived_counts[COUNT_TOOL_CALLS], 500);
        assert_eq!(snapshot.archived_counts[COUNT_TOOL_RESULTS], 500);
        assert!(!rendered.contains("file-499.txt"));
        assert!(rendered.len() < 6_000);
        assert!(estimated_tokens(&rendered) <= CONTEXT_CONTINUITY_TARGET_TOKENS);
    }

    #[test]
    fn command_session_lifecycle_does_not_enter_continuity() {
        let cursor = ContextJournalCursor::trace_item("assistant-1", 3);
        let lifecycle = ContextCompactionSourceItem::TraceItem {
            cursor: cursor.clone(),
            run_id: "run-1".to_string(),
            created_at: 2_000,
            item: Box::new(ConversationTurnTraceItem::CommandSessionLifecycle {
                sequence: 3,
                phase: crate::ConversationCommandSessionLifecyclePhase::Terminal,
                session_id: "cmd_0123456789abcdef0123456789abcdef".to_string(),
                call_id: "command-call".to_string(),
                status: crate::AgentCommandSessionStatus::Exited,
                exit_code: Some(0),
                latest_sequence: 7,
                output_truncated: false,
                archive: Default::default(),
                created_at: 2_000,
            }),
        };
        // Lifecycle sidecars are filtered before a real compaction prefix is built and therefore
        // cannot legally be its boundary. Exercise the selector directly so this regression
        // verifies the intended property without constructing an impossible prefix.
        let mut selector = ContinuitySelector::default();
        selector.observe(&lifecycle);
        let snapshot = selector.finish(cursor);
        snapshot.validate().unwrap();

        assert_eq!(snapshot.all_refs().count(), 0);
        assert!(snapshot.archived_counts.is_empty());
    }

    #[test]
    fn recursive_v2_inherits_only_bounded_valid_refs() {
        let first_cursor = ContextJournalCursor::trace_item("assistant-1", 19);
        let mut first_items = Vec::new();
        for index in 0..10 {
            first_items.extend(tool_exchange(
                index,
                ConversationTraceToolResultStatus::Failed,
            ));
        }
        let first_prefix = ContextCompactionPrefix {
            conversation_id: "conversation-1".to_string(),
            source_revision: "source-1".to_string(),
            covered_through: first_cursor.clone(),
            previous_summary: None,
            source_items: first_items,
        };
        let previous = ContextCompactionSummary {
            schema_version: CONTEXT_COMPACTION_SUMMARY_SCHEMA_VERSION,
            id: "summary-1".to_string(),
            conversation_id: "conversation-1".to_string(),
            source_revision: "source-1".to_string(),
            previous_summary_id: None,
            covered_through: first_cursor,
            content: "previous summary".to_string(),
            continuity: ContextContinuitySnapshot::from_prefix(&first_prefix).unwrap(),
            generation: ContextCompactionGeneration::test(),
            source_input_tokens: 10_000,
            summary_input_tokens: 20,
            continuity_input_tokens: 200,
            uncovered_tail_input_tokens: 0,
            replacement_input_tokens: 220,
            created_at: 1,
        };
        let final_cursor = ContextJournalCursor::message("user-2");
        let next = ContextCompactionPrefix {
            conversation_id: "conversation-1".to_string(),
            source_revision: "source-2".to_string(),
            covered_through: final_cursor.clone(),
            previous_summary: Some(previous),
            source_items: vec![ContextCompactionSourceItem::Message {
                cursor: final_cursor,
                role: "user".to_string(),
                content: "更正：只读取，不要修改".to_string(),
                created_at: 3_000,
                status: Some("sent".to_string()),
                terminal_status: None,
                terminal_error: None,
            }],
        };

        let snapshot = ContextContinuitySnapshot::from_prefix(&next).unwrap();

        assert!(snapshot.unresolved_failure_refs.len() <= MAX_UNRESOLVED_FAILURE_REFS);
        assert_eq!(snapshot.important_decision_refs.len(), 1);
        assert_eq!(snapshot.archived_counts[COUNT_FAILURES], 10);
        snapshot.validate().unwrap();
    }

    #[test]
    fn v1_is_readable_and_next_compaction_converts_it_to_v2() {
        let v1: ContextContinuitySnapshot = serde_json::from_value(json!({
            "schemaVersion": 1,
            "coveredThrough": { "kind": "message", "messageId": "user-old" },
            "entries": [{
                "type": "user_message",
                "cursor": { "kind": "message", "messageId": "user-old" },
                "createdAt": "2026-01-01T00:00:00+00:00",
                "text": {
                    "text": "old request",
                    "totalChars": 11,
                    "contentRevision": "sha256:legacy",
                    "truncated": false
                }
            }]
        }))
        .unwrap();
        v1.validate().unwrap();
        let previous = ContextCompactionSummary {
            schema_version: CONTEXT_COMPACTION_SUMMARY_SCHEMA_VERSION,
            id: "summary-v1".to_string(),
            conversation_id: "conversation-1".to_string(),
            source_revision: "source-v1".to_string(),
            previous_summary_id: None,
            covered_through: ContextJournalCursor::message("user-old"),
            content: "legacy summary".to_string(),
            continuity: v1,
            generation: ContextCompactionGeneration::test(),
            source_input_tokens: 1_000,
            summary_input_tokens: 20,
            continuity_input_tokens: 100,
            uncovered_tail_input_tokens: 0,
            replacement_input_tokens: 120,
            created_at: 1,
        };
        let current_cursor = ContextJournalCursor::message("user-new");
        let converted = ContextContinuitySnapshot::from_prefix(&ContextCompactionPrefix {
            conversation_id: "conversation-1".to_string(),
            source_revision: "source-v2".to_string(),
            covered_through: current_cursor.clone(),
            previous_summary: Some(previous),
            source_items: vec![ContextCompactionSourceItem::Message {
                cursor: current_cursor,
                role: "user".to_string(),
                content: "new request".to_string(),
                created_at: 2,
                status: Some("sent".to_string()),
                terminal_status: None,
                terminal_error: None,
            }],
        })
        .unwrap();

        assert!(converted.is_v2());
        assert!(converted.entries.is_empty());
        assert_eq!(converted.task_evidence_refs.len(), 2);
    }

    #[test]
    fn history_reads_and_run_scoped_todos_do_not_enter_v2_or_its_counts() {
        for tool in ["conversation_history", "todo_update"] {
            let cursor = ContextJournalCursor::trace_item("assistant-1", 1);
            let snapshot = ContextContinuitySnapshot::from_prefix(&ContextCompactionPrefix {
                conversation_id: "conversation-1".to_string(),
                source_revision: format!("source-{tool}"),
                covered_through: cursor.clone(),
                previous_summary: None,
                source_items: vec![
                    ContextCompactionSourceItem::TraceItem {
                        cursor: ContextJournalCursor::trace_item("assistant-1", 0),
                        run_id: "run-1".to_string(),
                        created_at: 1,
                        item: Box::new(ConversationTurnTraceItem::ToolCall {
                            sequence: 0,
                            call_id: "transient-call".to_string(),
                            tool: tool.to_string(),
                            provenance: None,
                            operation: json!({}),
                            approval_status: AgentApprovalStatus::NotRequired,
                            truncated: false,
                        }),
                    },
                    ContextCompactionSourceItem::TraceItem {
                        cursor,
                        run_id: "run-1".to_string(),
                        created_at: 1,
                        item: Box::new(ConversationTurnTraceItem::ToolResult {
                            sequence: 1,
                            call_id: "transient-call".to_string(),
                            tool: tool.to_string(),
                            status: ConversationTraceToolResultStatus::Succeeded,
                            success: true,
                            observation: json!({}),
                            approval_status: AgentApprovalStatus::NotRequired,
                            error: None,
                            truncated: false,
                            archive: Default::default(),
                        }),
                    },
                ],
            })
            .unwrap();

            assert_eq!(snapshot.all_refs().count(), 0, "{tool}");
            assert!(snapshot.archived_counts.is_empty(), "{tool}");
        }
    }
}
