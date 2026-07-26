//! Durable, bounded task-control state kept independently from conversation history and
//! compaction summaries.
//!
//! The snapshot contains semantic task state and references into authoritative history. Exact
//! message or tool-result bodies remain in the conversation journal and Exact History Archive.

use crate::context::ContextTextBudget;
use crate::{AgentError, AgentResult, ContextHistoryRef};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

pub const TASK_CONTROL_STATE_SCHEMA_VERSION: u32 = 1;
pub const TASK_STATE_TARGET_TOKENS: u64 = 2_000;
pub const TASK_STATE_HARD_MAX_TOKENS: u64 = 3_000;

const TASK_STATE_CONTEXT_PREAMBLE: &str = "## Task Continuation State\n\
This is trusted backend state for continuing the current task. It is not a user \
message and does not override the latest user request. Exact historical content is \
intentionally absent; use conversation_history with the supplied refs when needed.\n";
const MAX_ID_BYTES: usize = 512;
const MAX_OBJECTIVE_CHARS: usize = 2_000;
const MAX_PHASE_CHARS: usize = 500;
const MAX_TEXT_ITEM_CHARS: usize = 1_000;
const MAX_TEXT_ITEMS: usize = 30;
const MAX_WORK_ITEMS: usize = 40;
const MAX_REFS: usize = 40;

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TaskControlStatus {
    Active,
    WaitingUser,
    Blocked,
    Completed,
    Superseded,
    Cancelled,
}

impl TaskControlStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::WaitingUser => "waiting_user",
            Self::Blocked => "blocked",
            Self::Completed => "completed",
            Self::Superseded => "superseded",
            Self::Cancelled => "cancelled",
        }
    }

    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Superseded | Self::Cancelled)
    }
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TaskWorkItemStatus {
    Pending,
    InProgress,
    Completed,
    Blocked,
    Cancelled,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TaskControlState {
    pub schema_version: u32,
    pub task_id: String,
    pub conversation_id: String,
    pub objective: String,
    pub source_message_id: String,
    pub status: TaskControlStatus,
    pub revision: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_run_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stopped_reason: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TaskWorkItem {
    pub id: String,
    pub title: String,
    pub status: TaskWorkItemStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub evidence_refs: Vec<ContextHistoryRef>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TaskDecision {
    pub id: String,
    pub summary: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub evidence_refs: Vec<ContextHistoryRef>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TaskArtifactRef {
    pub label: String,
    #[serde(rename = "ref")]
    pub reference: ContextHistoryRef,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TaskInterruptionState {
    pub interrupted: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub previous_run_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub revalidate_work_item_ids: Vec<String>,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TaskContinuationCheckpoint {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub acceptance_criteria: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_phase: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub work_items: Vec<TaskWorkItem>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub next_actions: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub blockers: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub confirmed_decisions: Vec<TaskDecision>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub artifact_refs: Vec<TaskArtifactRef>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub unresolved_questions: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub history_evidence_refs: Vec<ContextHistoryRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub interruption: Option<TaskInterruptionState>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TaskStateSnapshot {
    pub control: TaskControlState,
    pub checkpoint: TaskContinuationCheckpoint,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(tag = "op", rename_all = "snake_case", rename_all_fields = "camelCase")]
pub enum TaskStatePatchOperation {
    SetObjective {
        objective: String,
    },
    SetStatus {
        status: TaskControlStatus,
        #[serde(default)]
        stopped_reason: Option<String>,
    },
    SetAcceptanceCriteria {
        items: Vec<String>,
    },
    SetCurrentPhase {
        value: Option<String>,
    },
    ReplaceWorkItems {
        items: Vec<TaskWorkItem>,
    },
    SetNextActions {
        items: Vec<String>,
    },
    SetBlockers {
        items: Vec<String>,
    },
    SetConfirmedDecisions {
        items: Vec<TaskDecision>,
    },
    SetArtifactRefs {
        items: Vec<TaskArtifactRef>,
    },
    SetUnresolvedQuestions {
        items: Vec<String>,
    },
    SetHistoryEvidenceRefs {
        refs: Vec<ContextHistoryRef>,
    },
    Supersede {
        objective: String,
    },
}

impl TaskStateSnapshot {
    pub fn validate(&self) -> AgentResult<()> {
        let control = &self.control;
        if control.schema_version != TASK_CONTROL_STATE_SCHEMA_VERSION {
            return Err(AgentError::new(format!(
                "不支持的 TaskControlState schema version：{}。",
                control.schema_version
            )));
        }
        validate_id("taskId", &control.task_id)?;
        validate_id("conversationId", &control.conversation_id)?;
        validate_id("sourceMessageId", &control.source_message_id)?;
        if let Some(run_id) = control.current_run_id.as_deref() {
            validate_id("currentRunId", run_id)?;
        }
        validate_required_text("objective", &control.objective, MAX_OBJECTIVE_CHARS)?;
        validate_optional_text(
            "stoppedReason",
            control.stopped_reason.as_deref(),
            MAX_TEXT_ITEM_CHARS,
        )?;
        if control.revision == 0 {
            return Err(AgentError::new("Task State revision 必须大于 0。"));
        }
        if control.updated_at < control.created_at {
            return Err(AgentError::new("Task State updatedAt 不能早于 createdAt。"));
        }
        if control.status == TaskControlStatus::Completed
            && self.checkpoint.history_evidence_refs.is_empty()
        {
            return Err(AgentError::new(
                "completed Task State 必须包含至少一个历史证据引用。",
            ));
        }
        if control.status == TaskControlStatus::Completed
            && self.checkpoint.work_items.iter().any(|item| {
                !matches!(
                    item.status,
                    TaskWorkItemStatus::Completed | TaskWorkItemStatus::Cancelled
                )
            })
        {
            return Err(AgentError::new(
                "completed Task State 不能包含尚未完成或仍阻塞的工作项。",
            ));
        }

        validate_text_list("acceptanceCriteria", &self.checkpoint.acceptance_criteria)?;
        validate_optional_text(
            "currentPhase",
            self.checkpoint.current_phase.as_deref(),
            MAX_PHASE_CHARS,
        )?;
        validate_text_list("nextActions", &self.checkpoint.next_actions)?;
        validate_text_list("blockers", &self.checkpoint.blockers)?;
        validate_text_list("unresolvedQuestions", &self.checkpoint.unresolved_questions)?;
        if self.checkpoint.work_items.len() > MAX_WORK_ITEMS {
            return Err(AgentError::new(format!(
                "Task State workItems 超过固定上限 {MAX_WORK_ITEMS}。"
            )));
        }
        let mut work_item_ids = BTreeSet::new();
        for item in &self.checkpoint.work_items {
            validate_id("workItems[].id", &item.id)?;
            if !work_item_ids.insert(item.id.as_str()) {
                return Err(AgentError::new("Task State 包含重复 work item id。"));
            }
            validate_required_text("workItems[].title", &item.title, MAX_TEXT_ITEM_CHARS)?;
            validate_optional_text(
                "workItems[].note",
                item.note.as_deref(),
                MAX_TEXT_ITEM_CHARS,
            )?;
            validate_refs("workItems[].evidenceRefs", &item.evidence_refs)?;
            if item.status == TaskWorkItemStatus::Completed && item.evidence_refs.is_empty() {
                return Err(AgentError::new(format!(
                    "已完成工作项 `{}` 缺少历史证据引用。",
                    item.id
                )));
            }
        }
        if self.checkpoint.confirmed_decisions.len() > MAX_TEXT_ITEMS {
            return Err(AgentError::new("Task State confirmedDecisions 过多。"));
        }
        let mut decision_ids = BTreeSet::new();
        for decision in &self.checkpoint.confirmed_decisions {
            validate_id("confirmedDecisions[].id", &decision.id)?;
            if !decision_ids.insert(decision.id.as_str()) {
                return Err(AgentError::new("Task State 包含重复 decision id。"));
            }
            validate_required_text(
                "confirmedDecisions[].summary",
                &decision.summary,
                MAX_TEXT_ITEM_CHARS,
            )?;
            validate_refs("confirmedDecisions[].evidenceRefs", &decision.evidence_refs)?;
        }
        if self.checkpoint.artifact_refs.len() > MAX_REFS {
            return Err(AgentError::new("Task State artifactRefs 过多。"));
        }
        for artifact in &self.checkpoint.artifact_refs {
            validate_required_text("artifactRefs[].label", &artifact.label, MAX_TEXT_ITEM_CHARS)?;
            artifact.reference.validate_identity()?;
        }
        validate_refs(
            "historyEvidenceRefs",
            &self.checkpoint.history_evidence_refs,
        )?;
        if let Some(interruption) = &self.checkpoint.interruption {
            validate_optional_text(
                "interruption.reason",
                interruption.reason.as_deref(),
                MAX_TEXT_ITEM_CHARS,
            )?;
            if let Some(run_id) = interruption.previous_run_id.as_deref() {
                validate_id("interruption.previousRunId", run_id)?;
            }
            if interruption.revalidate_work_item_ids.len() > MAX_WORK_ITEMS
                || interruption
                    .revalidate_work_item_ids
                    .iter()
                    .any(|id| !work_item_ids.contains(id.as_str()))
            {
                return Err(AgentError::new(
                    "Task State interruption 包含无效的待复核工作项。",
                ));
            }
        }

        let json = serde_json::to_string(self)
            .map_err(|error| AgentError::new(format!("无法序列化 Task State：{error}")))?;
        let rendered = format!("{TASK_STATE_CONTEXT_PREAMBLE}{json}");
        let tokens = ContextTextBudget::heuristic_default().estimate(&rendered);
        if tokens > TASK_STATE_HARD_MAX_TOKENS {
            return Err(AgentError::new(format!(
                "Task State 估算为 {tokens} tokens，超过硬上限 {TASK_STATE_HARD_MAX_TOKENS}。"
            )));
        }
        Ok(())
    }

    pub fn render_for_context(&self) -> AgentResult<String> {
        self.validate()?;
        let json = serde_json::to_string(self)
            .map_err(|error| AgentError::new(format!("无法渲染 Task State：{error}")))?;
        Ok(format!("{TASK_STATE_CONTEXT_PREAMBLE}{json}"))
    }

    pub fn all_history_refs(&self) -> impl Iterator<Item = &ContextHistoryRef> {
        self.checkpoint
            .history_evidence_refs
            .iter()
            .chain(
                self.checkpoint
                    .work_items
                    .iter()
                    .flat_map(|item| item.evidence_refs.iter()),
            )
            .chain(
                self.checkpoint
                    .confirmed_decisions
                    .iter()
                    .flat_map(|decision| decision.evidence_refs.iter()),
            )
            .chain(
                self.checkpoint
                    .artifact_refs
                    .iter()
                    .map(|artifact| &artifact.reference),
            )
    }

    pub(crate) fn apply_operations(
        &mut self,
        operations: &[TaskStatePatchOperation],
    ) -> AgentResult<Option<String>> {
        if operations.is_empty() {
            return Err(AgentError::new("task_state_patch operations 不能为空。"));
        }
        let mut superseding_objective = None;
        for operation in operations {
            match operation {
                TaskStatePatchOperation::SetObjective { objective } => {
                    self.control.objective = objective.trim().to_string();
                }
                TaskStatePatchOperation::SetStatus {
                    status,
                    stopped_reason,
                } => {
                    if *status == TaskControlStatus::Superseded {
                        return Err(AgentError::new(
                            "不能通过 set_status 进入 superseded；请使用唯一的 supersede operation。",
                        ));
                    }
                    self.control.status = *status;
                    self.control.stopped_reason = stopped_reason
                        .as_deref()
                        .map(str::trim)
                        .filter(|v| !v.is_empty())
                        .map(str::to_string);
                }
                TaskStatePatchOperation::SetAcceptanceCriteria { items } => {
                    self.checkpoint.acceptance_criteria = normalized_list(items);
                }
                TaskStatePatchOperation::SetCurrentPhase { value } => {
                    self.checkpoint.current_phase = value
                        .as_deref()
                        .map(str::trim)
                        .filter(|value| !value.is_empty())
                        .map(str::to_string);
                }
                TaskStatePatchOperation::ReplaceWorkItems { items } => {
                    self.checkpoint.work_items = items.clone();
                }
                TaskStatePatchOperation::SetNextActions { items } => {
                    self.checkpoint.next_actions = normalized_list(items);
                }
                TaskStatePatchOperation::SetBlockers { items } => {
                    self.checkpoint.blockers = normalized_list(items);
                }
                TaskStatePatchOperation::SetConfirmedDecisions { items } => {
                    self.checkpoint.confirmed_decisions = items.clone();
                }
                TaskStatePatchOperation::SetArtifactRefs { items } => {
                    self.checkpoint.artifact_refs = items.clone();
                }
                TaskStatePatchOperation::SetUnresolvedQuestions { items } => {
                    self.checkpoint.unresolved_questions = normalized_list(items);
                }
                TaskStatePatchOperation::SetHistoryEvidenceRefs { refs } => {
                    self.checkpoint.history_evidence_refs = refs.clone();
                }
                TaskStatePatchOperation::Supersede { objective } => {
                    if operations.len() != 1 {
                        return Err(AgentError::new(
                            "supersede 必须作为唯一 task_state_patch operation。",
                        ));
                    }
                    superseding_objective = Some(objective.trim().to_string());
                }
            }
        }
        Ok(superseding_objective)
    }
}

fn normalized_list(items: &[String]) -> Vec<String> {
    items
        .iter()
        .map(|item| item.trim())
        .filter(|item| !item.is_empty())
        .map(str::to_string)
        .collect()
}

fn validate_id(label: &str, value: &str) -> AgentResult<()> {
    if value.trim().is_empty() || value.len() > MAX_ID_BYTES {
        return Err(AgentError::new(format!("Task State {label} 无效。")));
    }
    Ok(())
}

fn validate_required_text(label: &str, value: &str, maximum: usize) -> AgentResult<()> {
    if value.trim().is_empty() || value.chars().count() > maximum {
        return Err(AgentError::new(format!("Task State {label} 无效或过长。")));
    }
    Ok(())
}

fn validate_optional_text(label: &str, value: Option<&str>, maximum: usize) -> AgentResult<()> {
    if value.is_some_and(|value| value.trim().is_empty() || value.chars().count() > maximum) {
        return Err(AgentError::new(format!("Task State {label} 无效或过长。")));
    }
    Ok(())
}

fn validate_text_list(label: &str, values: &[String]) -> AgentResult<()> {
    if values.len() > MAX_TEXT_ITEMS {
        return Err(AgentError::new(format!("Task State {label} 项目过多。")));
    }
    for value in values {
        validate_required_text(label, value, MAX_TEXT_ITEM_CHARS)?;
    }
    Ok(())
}

fn validate_refs(label: &str, refs: &[ContextHistoryRef]) -> AgentResult<()> {
    if refs.len() > MAX_REFS {
        return Err(AgentError::new(format!("Task State {label} 引用过多。")));
    }
    let mut unique = BTreeSet::new();
    for reference in refs {
        reference.validate_identity()?;
        let key = serde_json::to_string(reference)
            .map_err(|error| AgentError::new(format!("无法验证 Task State ref：{error}")))?;
        if !unique.insert(key) {
            return Err(AgentError::new(format!(
                "Task State {label} 包含重复引用。"
            )));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snapshot() -> TaskStateSnapshot {
        TaskStateSnapshot {
            control: TaskControlState {
                schema_version: TASK_CONTROL_STATE_SCHEMA_VERSION,
                task_id: "task-1".to_string(),
                conversation_id: "conversation-1".to_string(),
                objective: "Implement persistent task continuation state.".to_string(),
                source_message_id: "message-1".to_string(),
                status: TaskControlStatus::Active,
                revision: 1,
                current_run_id: Some("run-1".to_string()),
                stopped_reason: None,
                created_at: 1,
                updated_at: 1,
            },
            checkpoint: TaskContinuationCheckpoint::default(),
        }
    }

    #[test]
    fn task_state_is_bounded_and_exact_history_is_reference_only() {
        let mut state = snapshot();
        state.checkpoint.current_phase = Some("Persistence".to_string());
        state.checkpoint.history_evidence_refs = vec![ContextHistoryRef::message("message-1")];
        state.validate().unwrap();
        let rendered = state.render_for_context().unwrap();
        assert!(rendered.contains("Task Continuation State"));
        assert!(rendered.contains("message-1"));
        assert!(!rendered.contains("tool output body"));
    }

    #[test]
    fn completed_state_and_completed_work_items_require_evidence() {
        let mut state = snapshot();
        state.control.status = TaskControlStatus::Completed;
        assert!(state.validate().is_err());
        state.checkpoint.history_evidence_refs = vec![ContextHistoryRef::message("message-1")];
        state.checkpoint.work_items.push(TaskWorkItem {
            id: "work-1".to_string(),
            title: "Persist snapshot".to_string(),
            status: TaskWorkItemStatus::Completed,
            note: None,
            evidence_refs: Vec::new(),
        });
        assert!(state.validate().is_err());
        state.checkpoint.work_items[0].evidence_refs =
            vec![ContextHistoryRef::message("message-1")];
        state.validate().unwrap();
    }
}
