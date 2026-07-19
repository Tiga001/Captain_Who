use super::*;

impl StorageService {
    pub fn create_agent_file_draft(&self, draft: AgentFileDraftRecord) -> Result<(), String> {
        let connection = self.state.connection()?;
        file_draft_repository::insert_draft(&connection, &draft).map_err(storage_error)
    }

    pub fn get_agent_file_draft(
        &self,
        draft_id: &str,
    ) -> Result<Option<AgentFileDraftRecord>, String> {
        let connection = self.state.connection()?;
        file_draft_repository::get_draft(&connection, draft_id).map_err(storage_error)
    }

    pub fn list_agent_file_drafts_for_run(
        &self,
        run_id: &str,
    ) -> Result<Vec<AgentFileDraftRecord>, String> {
        let connection = self.state.connection()?;
        file_draft_repository::list_drafts_for_run(&connection, run_id).map_err(storage_error)
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

    pub fn settle_unresolved_agent_file_drafts_for_run(
        &self,
        run_id: &str,
        status: &str,
    ) -> Result<usize, String> {
        if !matches!(status, "failed" | "aborted") {
            return Err(format!("不支持的文件草稿结算状态：{status}"));
        }
        let connection = self.state.connection()?;
        file_draft_repository::settle_unresolved_drafts_for_run(
            &connection,
            run_id,
            status,
            now_ms(),
        )
        .map_err(storage_error)
    }

    pub fn save_agent_file_draft_progress(
        &self,
        draft: &AgentFileDraftRecord,
        chunk: Option<&AgentFileDraftChunkRecord>,
        operation: Option<&AgentFileDraftOperationRecord>,
    ) -> Result<(), String> {
        let mut connection = self.state.connection()?;
        file_draft_repository::save_draft_progress(&mut connection, draft, chunk, operation)
            .map_err(storage_error)
    }

    pub fn update_agent_file_draft(&self, draft: &AgentFileDraftRecord) -> Result<(), String> {
        let connection = self.state.connection()?;
        file_draft_repository::update_draft(&connection, draft).map_err(storage_error)
    }

    pub fn get_agent_file_draft_chunk_hash(
        &self,
        draft_id: &str,
        chunk_index: u64,
    ) -> Result<Option<String>, String> {
        let connection = self.state.connection()?;
        file_draft_repository::get_chunk_hash(&connection, draft_id, chunk_index)
            .map_err(storage_error)
    }

    pub fn next_agent_file_draft_operation_sequence(&self, draft_id: &str) -> Result<u64, String> {
        let connection = self.state.connection()?;
        file_draft_repository::next_operation_sequence(&connection, draft_id).map_err(storage_error)
    }
}
