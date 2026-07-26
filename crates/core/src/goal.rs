//! Optional, user-owned conversation goals.
//!
//! A goal is deliberately much smaller than conversation history, compaction summaries, or a
//! runtime todo. It only preserves an explicitly requested long-running objective and its coarse
//! lifecycle. Ordinary conversations do not have a goal.

use crate::{AgentError, AgentResult};
use serde::{Deserialize, Serialize};

pub const MAX_CONVERSATION_GOAL_OBJECTIVE_CHARS: usize = 4_000;

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
            "## Active Goal\n\nObjective:\n{}\n\nStatus:\n{}",
            self.objective,
            self.status.as_str()
        );
        if let Some(reason) = self
            .stopped_reason
            .as_deref()
            .map(str::trim)
            .filter(|reason| !reason.is_empty())
        {
            rendered.push_str("\n\nStopped reason:\n");
            rendered.push_str(reason);
        }
        rendered.push_str(
            "\n\nThis goal exists only because the user explicitly requested persistent \
             tracking. The latest user message has priority. The goal does not authorize \
             automatic continuation.",
        );
        Ok(rendered)
    }
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
        assert!(rendered.contains("latest user message has priority"));
        assert!(rendered.contains("does not authorize automatic continuation"));
        assert!(!rendered.contains("revision"));
    }
}
