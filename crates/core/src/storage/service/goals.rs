use super::*;

impl StorageService {
    pub fn load_conversation_goal(
        &self,
        conversation_id: &str,
    ) -> Result<Option<ConversationGoal>, String> {
        let connection = self.state.connection()?;
        conversation_goal_repository::get_goal(&connection, conversation_id).map_err(storage_error)
    }

    pub fn load_visible_conversation_goal(
        &self,
        conversation_id: &str,
    ) -> Result<Option<ConversationGoal>, String> {
        let connection = self.state.connection()?;
        conversation_goal_repository::get_visible_goal(&connection, conversation_id)
            .map_err(storage_error)
    }

    pub fn create_conversation_goal(
        &self,
        conversation_id: &str,
        objective: &str,
        created_at: i64,
    ) -> Result<ConversationGoal, String> {
        crate::goal::validate_objective(objective).map_err(|error| error.to_string())?;
        let connection = self.state.connection()?;
        let goal = conversation_goal_repository::create_goal(
            &connection,
            conversation_id,
            objective.trim(),
            created_at,
        )
        .map_err(storage_error)?
        .ok_or_else(|| {
            "无法创建 Goal：当前对话没有用户消息，或仍存在尚未结束的 Goal。".to_string()
        })?;
        goal.validate().map_err(|error| error.to_string())?;
        Ok(goal)
    }

    pub fn update_conversation_goal_status(
        &self,
        conversation_id: &str,
        status: ConversationGoalStatus,
        stopped_reason: Option<&str>,
        updated_at: i64,
    ) -> Result<ConversationGoal, String> {
        if !matches!(
            status,
            ConversationGoalStatus::Completed | ConversationGoalStatus::Blocked
        ) {
            return Err("模型只能将 Goal 标记为 completed 或 blocked。".to_string());
        }
        let connection = self.state.connection()?;
        let goal = conversation_goal_repository::update_goal_status(
            &connection,
            conversation_id,
            status,
            stopped_reason,
            updated_at,
        )
        .map_err(storage_error)?
        .ok_or_else(|| "当前对话没有可更新的 active Goal。".to_string())?;
        goal.validate().map_err(|error| error.to_string())?;
        Ok(goal)
    }

    pub fn resume_blocked_conversation_goal_for_user_turn(
        &self,
        conversation_id: &str,
        updated_at: i64,
    ) -> Result<Option<ConversationGoal>, String> {
        let connection = self.state.connection()?;
        conversation_goal_repository::resume_blocked_goal_for_user_turn(
            &connection,
            conversation_id,
            updated_at,
        )
        .map_err(storage_error)
    }
}
