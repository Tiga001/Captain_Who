use super::*;

impl StorageService {
    pub fn load_latest_task_state(
        &self,
        conversation_id: &str,
    ) -> Result<Option<TaskStateSnapshot>, String> {
        let connection = self.state.connection()?;
        task_state_repository::load_latest_task(&connection, conversation_id)
    }

    pub fn load_task_state(
        &self,
        conversation_id: &str,
        task_id: &str,
    ) -> Result<Option<TaskStateSnapshot>, String> {
        let connection = self.state.connection()?;
        task_state_repository::load_task(&connection, conversation_id, task_id)
    }

    pub fn list_task_state_revisions(
        &self,
        conversation_id: &str,
        task_id: &str,
    ) -> Result<Vec<task_state_repository::TaskStateRevisionRecord>, String> {
        let connection = self.state.connection()?;
        task_state_repository::list_revisions(&connection, conversation_id, task_id)
    }

    pub fn begin_or_resume_task_state(
        &self,
        conversation_id: &str,
        source_message_id: &str,
        objective: &str,
        run_id: &str,
        created_at: i64,
    ) -> Result<TaskStateSnapshot, String> {
        let mut connection = self.state.connection()?;
        task_state_repository::begin_or_resume_for_turn(
            &mut connection,
            conversation_id,
            source_message_id,
            objective,
            run_id,
            created_at,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn patch_task_state(
        &self,
        conversation_id: &str,
        task_id: &str,
        expected_revision: u64,
        operations: &[TaskStatePatchOperation],
        run_id: Option<&str>,
        mutation_id: &str,
        updated_at: i64,
    ) -> Result<TaskStateSnapshot, String> {
        let mut connection = self.state.connection()?;
        task_state_repository::patch_task(
            &mut connection,
            conversation_id,
            task_id,
            expected_revision,
            operations,
            run_id,
            mutation_id,
            updated_at,
        )
    }

    pub fn rollback_task_state(
        &self,
        conversation_id: &str,
        task_id: &str,
        expected_revision: u64,
        target_revision: u64,
        mutation_id: &str,
        updated_at: i64,
    ) -> Result<TaskStateSnapshot, String> {
        let mut connection = self.state.connection()?;
        task_state_repository::rollback_task(
            &mut connection,
            conversation_id,
            task_id,
            expected_revision,
            target_revision,
            mutation_id,
            updated_at,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn replace_task_work_items(
        &self,
        conversation_id: &str,
        task_id: &str,
        expected_revision: u64,
        work_items: Vec<TaskWorkItem>,
        run_id: &str,
        mutation_id: &str,
        updated_at: i64,
    ) -> Result<TaskStateSnapshot, String> {
        let mut connection = self.state.connection()?;
        task_state_repository::replace_work_items(
            &mut connection,
            conversation_id,
            task_id,
            expected_revision,
            work_items,
            run_id,
            mutation_id,
            updated_at,
        )
    }

    pub fn settle_task_state_run(
        &self,
        conversation_id: &str,
        task_id: &str,
        run_id: &str,
        waiting_for_approval: bool,
        stopped_reason: Option<&str>,
        updated_at: i64,
    ) -> Result<Option<TaskStateSnapshot>, String> {
        let mut connection = self.state.connection()?;
        task_state_repository::settle_run(
            &mut connection,
            conversation_id,
            task_id,
            run_id,
            waiting_for_approval,
            stopped_reason,
            updated_at,
        )
    }

    pub fn resume_task_state_after_approval(
        &self,
        conversation_id: &str,
        task_id: &str,
        run_id: &str,
        updated_at: i64,
    ) -> Result<Option<TaskStateSnapshot>, String> {
        let mut connection = self.state.connection()?;
        task_state_repository::resume_after_approval(
            &mut connection,
            conversation_id,
            task_id,
            run_id,
            updated_at,
        )
    }

    pub fn reconcile_interrupted_task_states(&self, interrupted_at: i64) -> Result<usize, String> {
        let mut connection = self.state.connection()?;
        task_state_repository::mark_interrupted_tasks(&mut connection, interrupted_at)
    }
}
