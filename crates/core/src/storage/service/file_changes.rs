use super::*;

impl StorageService {
    pub fn create_agent_file_change(&self, change: AgentFileChangeRecord) -> Result<(), String> {
        let connection = self.state.connection()?;
        file_change_repository::insert_file_change(&connection, &change).map_err(storage_error)
    }

    pub fn get_agent_file_change(
        &self,
        transaction_id: &str,
    ) -> Result<Option<AgentFileChangeRecord>, String> {
        let connection = self.state.connection()?;
        file_change_repository::get_file_change(&connection, transaction_id).map_err(storage_error)
    }

    pub fn get_agent_file_change_for_owner(
        &self,
        transaction_id: &str,
        conversation_id: &str,
        project_id: Option<&str>,
        run_id: &str,
        source_tool_name: &str,
    ) -> Result<Option<AgentFileChangeRecord>, String> {
        let connection = self.state.connection()?;
        file_change_repository::get_file_change_for_owner(
            &connection,
            transaction_id,
            conversation_id,
            project_id,
            run_id,
            source_tool_name,
        )
        .map_err(storage_error)
    }

    pub fn get_agent_file_change_for_source_call(
        &self,
        conversation_id: &str,
        project_id: Option<&str>,
        run_id: &str,
        source_tool_call_id: &str,
    ) -> Result<Option<AgentFileChangeRecord>, String> {
        let connection = self.state.connection()?;
        file_change_repository::get_file_change_for_source_call(
            &connection,
            conversation_id,
            project_id,
            run_id,
            source_tool_call_id,
        )
        .map_err(storage_error)
    }

    pub fn list_agent_file_changes_for_run(
        &self,
        run_id: &str,
    ) -> Result<Vec<AgentFileChangeRecord>, String> {
        let connection = self.state.connection()?;
        file_change_repository::list_file_changes_for_run(&connection, run_id)
            .map_err(storage_error)
    }

    pub fn list_agent_tool_results_for_run(
        &self,
        run_id: &str,
        tool_name: &str,
    ) -> Result<Vec<AgentToolResult>, String> {
        let connection = self.state.connection()?;
        agent_action_audit_repository::list_tool_result_json_for_run(&connection, run_id, tool_name)
            .map_err(storage_error)?
            .into_iter()
            .map(|payload| {
                serde_json::from_str(&payload)
                    .map_err(|error| format!("本地工具结果记录无法解析：{error}"))
            })
            .collect()
    }

    /// Loads the authoritative command observations persisted in the action audit for a run.
    ///
    /// This is intentionally distinct from the model-facing Tool Result projection: callers that
    /// audit failed, timed-out, or cancelled commands receive the complete execution structure,
    /// including artifact observation coverage and effects.
    pub fn list_agent_command_results_for_run(
        &self,
        run_id: &str,
    ) -> Result<Vec<AgentCommandExecutionResult>, String> {
        let connection = self.state.connection()?;
        agent_action_audit_repository::list_command_result_json_for_run(&connection, run_id)
            .map_err(storage_error)?
            .into_iter()
            .map(|payload| {
                serde_json::from_str(&payload)
                    .map_err(|error| format!("本地命令执行记录无法解析：{error}"))
            })
            .collect()
    }

    pub fn settle_unresolved_agent_file_changes_for_run(
        &self,
        run_id: &str,
        status: &str,
    ) -> Result<usize, String> {
        if !matches!(status, "failed" | "aborted") {
            return Err(format!("不支持的文件变更结算状态：{status}"));
        }
        let connection = self.state.connection()?;
        // Only mutable drafts are safe to settle from a Run guard. waiting_approval is owned by
        // Pending Action recovery; applying may represent a commit-unknown side effect and must
        // remain available for digest/receipt reconciliation.
        file_change_repository::settle_unresolved_file_changes_for_run(
            &connection,
            run_id,
            status,
            now_ms(),
        )
        .map_err(storage_error)
    }

    pub fn save_agent_file_change_progress(
        &self,
        expected_draft_revision: u64,
        change: &AgentFileChangeRecord,
        chunk: Option<&AgentFileChangeChunkRecord>,
        operation: &AgentFileChangeOperationRecord,
    ) -> Result<file_change_repository::AgentFileChangeProgressSaveOutcome, String> {
        let mut connection = self.state.connection()?;
        file_change_repository::save_file_change_progress(
            &mut connection,
            expected_draft_revision,
            change,
            chunk,
            operation,
        )
        .map_err(storage_error)
    }

    pub fn update_agent_file_change(&self, change: &AgentFileChangeRecord) -> Result<(), String> {
        let connection = self.state.connection()?;
        file_change_repository::update_file_change(&connection, change).map_err(storage_error)
    }

    pub fn transition_agent_file_change(
        &self,
        expected_status: &str,
        expected_draft_revision: u64,
        expected_next_mutation_index: u64,
        change: &AgentFileChangeRecord,
    ) -> Result<bool, String> {
        let connection = self.state.connection()?;
        file_change_repository::transition_file_change(
            &connection,
            expected_status,
            expected_draft_revision,
            expected_next_mutation_index,
            change,
        )
        .map_err(storage_error)
    }

    pub fn get_agent_file_change_operation(
        &self,
        transaction_id: &str,
        mutation_index: u64,
    ) -> Result<Option<AgentFileChangeOperationRecord>, String> {
        let connection = self.state.connection()?;
        file_change_repository::get_operation(&connection, transaction_id, mutation_index)
            .map_err(storage_error)
    }
}
