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

    /// Validated semantic write boundary shared by model tools and a future explicit user UI.
    /// Callers must supply a trusted host-side actor; lifecycle code must never call this method.
    pub fn create_conversation_goal(
        &self,
        actor: ConversationGoalMutationActor,
        conversation_id: &str,
        objective: &str,
        created_at: i64,
    ) -> Result<ConversationGoal, String> {
        crate::goal::validate_objective(objective).map_err(|error| error.to_string())?;
        let connection = self.state.connection()?;
        let goal = conversation_goal_repository::create_goal(
            &connection,
            actor,
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

    /// Changes Goal status only through an explicit model or user action.
    pub fn update_conversation_goal_status(
        &self,
        actor: ConversationGoalMutationActor,
        conversation_id: &str,
        status: ConversationGoalStatus,
        stopped_reason: Option<&str>,
        updated_at: i64,
    ) -> Result<ConversationGoal, String> {
        let connection = self.state.connection()?;
        let goal = conversation_goal_repository::update_goal_status(
            &connection,
            actor,
            conversation_id,
            status,
            stopped_reason,
            updated_at,
        )
        .map_err(storage_error)?
        .ok_or_else(|| "当前对话没有允许执行该状态转换的 Goal。".to_string())?;
        goal.validate().map_err(|error| error.to_string())?;
        Ok(goal)
    }

    /// Reserved for an explicit model or user edit. The renderer RPC is intentionally not exposed
    /// yet, but it will share this journaled write path when added.
    pub fn update_conversation_goal_objective(
        &self,
        actor: ConversationGoalMutationActor,
        conversation_id: &str,
        objective: &str,
        updated_at: i64,
    ) -> Result<ConversationGoal, String> {
        crate::goal::validate_objective(objective).map_err(|error| error.to_string())?;
        let connection = self.state.connection()?;
        let goal = conversation_goal_repository::update_goal_objective(
            &connection,
            actor,
            conversation_id,
            objective.trim(),
            updated_at,
        )
        .map_err(storage_error)?
        .ok_or_else(|| "当前对话没有允许编辑的 active 或 blocked Goal。".to_string())?;
        goal.validate().map_err(|error| error.to_string())?;
        Ok(goal)
    }

    pub fn load_conversation_goal_revisions(
        &self,
        conversation_id: &str,
        goal_id: &str,
    ) -> Result<Vec<ConversationGoalRevision>, String> {
        let connection = self.state.connection()?;
        let revisions = conversation_goal_repository::list_goal_revisions(
            &connection,
            conversation_id,
            goal_id,
        )
        .map_err(storage_error)?;
        if !revisions.is_empty() {
            crate::fold_conversation_goal_revisions(&revisions)
                .map_err(|error| error.to_string())?;
        }
        Ok(revisions)
    }
}
