//! Optional, user-owned conversation goals.
//!
//! A goal is deliberately much smaller than conversation history, compaction summaries, or a
//! runtime todo. It only preserves an explicitly requested long-running objective and its coarse
//! lifecycle. Ordinary conversations do not have a goal.

use crate::{AgentError, AgentResult};
use serde::{Deserialize, Serialize};

pub const MAX_CONVERSATION_GOAL_OBJECTIVE_CHARS: usize = 1_000;
pub const CONVERSATION_GOAL_CONTEXT_TARGET_TOKENS: u64 = 200;
pub const CONVERSATION_GOAL_CONTEXT_HARD_MAX_TOKENS: u64 = 256;

/// Trusted host-side origin for a semantic Goal mutation.
///
/// This value is never accepted from model-authored tool arguments. Today production writes use
/// `Model`; `User` reserves the same validated backend boundary for a future explicit UI action.
#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ConversationGoalMutationActor {
    Model,
    User,
}

impl ConversationGoalMutationActor {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Model => "model",
            Self::User => "user",
        }
    }
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ConversationGoalStatus {
    Active,
    Blocked,
    Completed,
    Cancelled,
}

impl ConversationGoalStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Blocked => "blocked",
            Self::Completed => "completed",
            Self::Cancelled => "cancelled",
        }
    }

    pub fn is_visible_in_context(self) -> bool {
        matches!(self, Self::Active | Self::Blocked)
    }
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ConversationGoal {
    pub goal_id: String,
    pub conversation_id: String,
    pub objective: String,
    pub source_message_id: String,
    pub status: ConversationGoalStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stopped_reason: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
}

pub const CONVERSATION_GOAL_REVISION_SCHEMA_VERSION: u32 = 1;

/// One immutable semantic change in the durable Goal journal.
///
/// `Initial` is the only full snapshot. Later records contain only the values required to apply
/// and verify one semantic change. The folded `ConversationGoal` remains the only projection sent
/// to the model.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ConversationGoalRevisionEvent {
    Initial {
        goal: ConversationGoal,
    },
    ObjectiveChanged {
        previous_objective: String,
        objective: String,
        updated_at: i64,
    },
    StatusChanged {
        previous_status: ConversationGoalStatus,
        status: ConversationGoalStatus,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        previous_stopped_reason: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        stopped_reason: Option<String>,
        updated_at: i64,
    },
}

impl ConversationGoalRevisionEvent {
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Initial { .. } => "initial",
            Self::ObjectiveChanged { .. } => "objective_changed",
            Self::StatusChanged { .. } => "status_changed",
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ConversationGoalRevision {
    pub schema_version: u32,
    pub conversation_id: String,
    pub goal_id: String,
    pub sequence: u64,
    /// Legacy rows imported from the pre-journal current-state table have no attributable actor.
    /// All new semantic writes must identify either the model or the user.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub actor: Option<ConversationGoalMutationActor>,
    pub event: ConversationGoalRevisionEvent,
    pub created_at: i64,
}

impl ConversationGoal {
    pub fn validate(&self) -> AgentResult<()> {
        validate_id("goalId", &self.goal_id)?;
        validate_id("conversationId", &self.conversation_id)?;
        validate_id("sourceMessageId", &self.source_message_id)?;
        validate_objective(&self.objective)?;
        if self.updated_at < self.created_at {
            return Err(AgentError::new("Goal updatedAt 不能早于 createdAt。"));
        }
        if self
            .stopped_reason
            .as_deref()
            .is_some_and(|reason| reason.chars().count() > 1_000)
        {
            return Err(AgentError::new("Goal stoppedReason 不能超过 1000 个字符。"));
        }
        Ok(())
    }

    pub fn render_for_context(&self) -> AgentResult<String> {
        self.validate()?;
        let mut rendered = format!(
            "## Explicit Goal\nobjective: {}\nstatus: {}",
            self.objective,
            self.status.as_str()
        );
        if let Some(reason) = self
            .stopped_reason
            .as_deref()
            .map(str::trim)
            .filter(|reason| !reason.is_empty())
        {
            let prefix = "\nstoppedReason: ";
            let budget = crate::context::ContextTextBudget::heuristic(
                CONVERSATION_GOAL_CONTEXT_HARD_MAX_TOKENS,
            );
            let instruction =
                "\nLatest user instructions override this goal. It never authorizes automatic continuation.";
            let used = budget
                .estimate(&rendered)
                .saturating_add(budget.estimate(prefix))
                .saturating_add(budget.estimate(instruction));
            let remaining = CONVERSATION_GOAL_CONTEXT_HARD_MAX_TOKENS.saturating_sub(used);
            if remaining > 0 {
                let reason_budget = crate::context::ContextTextBudget::heuristic(remaining);
                let prefix_len = reason_budget.fitting_prefix_len(reason);
                rendered.push_str(prefix);
                rendered.push_str(&reason[..prefix_len]);
            }
        }
        rendered.push_str(
            "\nLatest user instructions override this goal. It never authorizes automatic continuation.",
        );
        let budget =
            crate::context::ContextTextBudget::heuristic(CONVERSATION_GOAL_CONTEXT_HARD_MAX_TOKENS);
        if !budget.fits(&rendered) {
            return Err(AgentError::new(format!(
                "Goal 上下文超过 {CONVERSATION_GOAL_CONTEXT_HARD_MAX_TOKENS} token 硬上限。"
            )));
        }
        Ok(rendered)
    }
}

pub fn fold_conversation_goal_revisions(
    revisions: &[ConversationGoalRevision],
) -> AgentResult<ConversationGoal> {
    let Some(first) = revisions.first() else {
        return Err(AgentError::new("Goal revision journal 为空。"));
    };
    let ConversationGoalRevisionEvent::Initial { goal } = &first.event else {
        return Err(AgentError::new(
            "Goal revision journal 必须从完整 initial snapshot 开始。",
        ));
    };
    if first.sequence != 1
        || first.schema_version != CONVERSATION_GOAL_REVISION_SCHEMA_VERSION
        || first.conversation_id != goal.conversation_id
        || first.goal_id != goal.goal_id
        || first.created_at != goal.created_at
    {
        return Err(AgentError::new("Goal initial revision 身份或版本无效。"));
    }
    goal.validate()?;
    let mut folded = goal.clone();
    let mut expected_sequence = 2_u64;
    for revision in &revisions[1..] {
        if revision.schema_version != CONVERSATION_GOAL_REVISION_SCHEMA_VERSION
            || revision.conversation_id != folded.conversation_id
            || revision.goal_id != folded.goal_id
            || revision.sequence != expected_sequence
            || revision.actor.is_none()
        {
            return Err(AgentError::new(
                "Goal revision journal 的身份、顺序、版本或 actor 无效。",
            ));
        }
        match &revision.event {
            ConversationGoalRevisionEvent::Initial { .. } => {
                return Err(AgentError::new(
                    "一个 Goal revision journal 只能包含一个 initial snapshot。",
                ));
            }
            ConversationGoalRevisionEvent::ObjectiveChanged {
                previous_objective,
                objective,
                updated_at,
            } => {
                if !folded.status.is_visible_in_context()
                    || previous_objective != &folded.objective
                    || *updated_at < folded.updated_at
                    || revision.created_at != *updated_at
                {
                    return Err(AgentError::new(
                        "Goal objective diff 的前置状态或时间无效。",
                    ));
                }
                validate_objective(objective)?;
                folded.objective = objective.clone();
                folded.updated_at = *updated_at;
            }
            ConversationGoalRevisionEvent::StatusChanged {
                previous_status,
                status,
                previous_stopped_reason,
                stopped_reason,
                updated_at,
            } => {
                let allowed_transition = matches!(
                    (folded.status, *status),
                    (
                        ConversationGoalStatus::Active,
                        ConversationGoalStatus::Blocked
                            | ConversationGoalStatus::Completed
                            | ConversationGoalStatus::Cancelled
                    ) | (
                        ConversationGoalStatus::Blocked,
                        ConversationGoalStatus::Active | ConversationGoalStatus::Cancelled
                    )
                );
                if !allowed_transition
                    || *previous_status != folded.status
                    || previous_stopped_reason != &folded.stopped_reason
                    || *updated_at < folded.updated_at
                    || revision.created_at != *updated_at
                {
                    return Err(AgentError::new("Goal status diff 的前置状态或时间无效。"));
                }
                folded.status = *status;
                folded.stopped_reason = stopped_reason.clone();
                folded.updated_at = *updated_at;
            }
        }
        folded.validate()?;
        expected_sequence = expected_sequence.saturating_add(1);
    }
    Ok(folded)
}

pub(crate) fn validate_objective(objective: &str) -> AgentResult<()> {
    let objective = objective.trim();
    if objective.is_empty() {
        return Err(AgentError::new("Goal objective 不能为空。"));
    }
    if objective.chars().count() > MAX_CONVERSATION_GOAL_OBJECTIVE_CHARS {
        return Err(AgentError::new(format!(
            "Goal objective 不能超过 {MAX_CONVERSATION_GOAL_OBJECTIVE_CHARS} 个字符。"
        )));
    }
    let rendered = format!(
        "## Explicit Goal\nobjective: {objective}\nstatus: active\nLatest user instructions override this goal. It never authorizes automatic continuation."
    );
    if !crate::context::ContextTextBudget::heuristic(CONVERSATION_GOAL_CONTEXT_HARD_MAX_TOKENS)
        .fits(&rendered)
    {
        return Err(AgentError::new(format!(
            "Goal objective 过长，无法放入 {CONVERSATION_GOAL_CONTEXT_HARD_MAX_TOKENS} token 上下文预算。"
        )));
    }
    Ok(())
}

fn validate_id(field: &str, value: &str) -> AgentResult<()> {
    let value = value.trim();
    if value.is_empty() || value.len() > 512 {
        return Err(AgentError::new(format!("Goal {field} 无效。")));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn goal_context_is_small_and_latest_user_wins() {
        let goal = ConversationGoal {
            goal_id: "goal-1".to_string(),
            conversation_id: "conversation-1".to_string(),
            objective: "Finish the migration and keep tests green.".to_string(),
            source_message_id: "message-1".to_string(),
            status: ConversationGoalStatus::Active,
            stopped_reason: None,
            created_at: 1,
            updated_at: 2,
        };

        let rendered = goal.render_for_context().unwrap();
        assert!(rendered.contains("Finish the migration"));
        assert!(rendered.contains("Latest user instructions override"));
        assert!(rendered.contains("never authorizes automatic continuation"));
        assert!(!rendered.contains("revision"));
        assert!(crate::context::ContextTextBudget::heuristic(
            CONVERSATION_GOAL_CONTEXT_HARD_MAX_TOKENS
        )
        .fits(&rendered));
    }
}
