use super::StorageService;
use crate::{
    storage::agent_workspace_repository, workspace::WorkspaceResolver, AgentWorkspaceContext,
};
use rusqlite::OptionalExtension;

impl StorageService {
    /// Resolve an attachment from the originating Turn's authorized library, including its Agent
    /// tree. Return its exact identity so the Host can apply the ordinary user-read authorization.
    pub fn resolve_run_attachment_file(
        &self,
        conversation_id: &str,
        assistant_message_id: &str,
        file_path: &str,
    ) -> Result<(String, crate::protocol::ResolvedAttachmentFile), String> {
        if conversation_id.trim().is_empty()
            || assistant_message_id.trim().is_empty()
            || !file_path.starts_with("@attachments/")
            || file_path.contains(['\0', '\r', '\n'])
        {
            return Err("历史读取附件不可用。".into());
        }
        let connection = self.state.connection()?;
        let identity: Option<(String, Option<String>)> = connection
            .query_row(
                "SELECT t.run_id, c.project_id
                 FROM conversation_turn_traces t
                 JOIN conversations c ON c.id = t.conversation_id
                 WHERE t.assistant_message_id = ?1 AND t.conversation_id = ?2",
                rusqlite::params![assistant_message_id, conversation_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .map_err(|error| error.to_string())?;
        let (run_id, project_id) = identity.ok_or("历史读取附件不属于此回复。")?;
        drop(connection);
        let library = self.build_attachment_library_context_for_active_run(
            conversation_id,
            project_id.as_deref(),
            &run_id,
        )?;
        let reference = library
            .conversation_attachments
            .iter()
            .chain(library.project_attachments.iter())
            .find(|attachment| attachment.read_path == file_path)
            .ok_or("历史读取附件不在此回复的授权范围内。")?;
        let file = self.resolve_attachment_file(&reference.id, &reference.message_id)?;
        Ok((reference.id.clone(), file))
    }

    pub fn load_agent_workspace_for_run(
        &self,
        run_id: &str,
    ) -> Result<Option<Option<AgentWorkspaceContext>>, String> {
        let connection = self.state.connection()?;
        agent_workspace_repository::load_run(&connection, run_id).map_err(|e| e.to_string())
    }

    pub fn load_agent_workspace_for_wake(
        &self,
        wake_id: &str,
    ) -> Result<Option<Option<AgentWorkspaceContext>>, String> {
        let connection = self.state.connection()?;
        agent_workspace_repository::load_wake(&connection, wake_id).map_err(|e| e.to_string())
    }

    pub fn load_run_workspace(
        &self,
        assistant_message_id: &str,
        project_id: Option<&str>,
    ) -> Result<Option<AgentWorkspaceContext>, String> {
        let connection = self.state.connection()?;
        let identity: Option<(String,Option<String>)> = connection.query_row(
            "SELECT t.run_id,c.project_id FROM conversation_turn_traces t JOIN conversations c ON c.id=t.conversation_id WHERE t.assistant_message_id=?1",
            [assistant_message_id], |r| Ok((r.get(0)?,r.get(1)?))).optional().map_err(|e|e.to_string())?;
        let (run_id, owner_project) = identity.ok_or("历史回复的工作区快照不可用。")?;
        if project_id.is_some() && owner_project.as_deref() != project_id {
            return Err("历史回复不属于此项目。".into());
        }
        let workspace = agent_workspace_repository::load_run(&connection, &run_id)
            .map_err(|e| e.to_string())?
            .ok_or("历史回复的工作区快照不可用。")?;
        if workspace.as_ref().and_then(|w| w.project_id.as_deref()) != owner_project.as_deref() {
            return Err("历史回复的工作区身份不一致。".into());
        }
        Ok(workspace)
    }

    pub fn resolve_run_workspace_path(
        &self,
        assistant_message_id: &str,
        project_id: Option<&str>,
        file_path: &str,
    ) -> Result<String, String> {
        let workspace = self.load_run_workspace(assistant_message_id, project_id)?;
        let resolver = WorkspaceResolver::from_context(workspace.as_ref());
        let target = resolver
            .resolve_input(file_path)?
            .canonicalize()
            .map_err(|_| "历史文件不可用。")?;
        // Revalidate a matching frozen root even for absolute paths. Existing explicit external
        // paths retain the Host's reveal/read semantics; no current-project alias is consulted.
        let root = resolver.containing_root(&target)?;
        if !std::path::Path::new(file_path).is_absolute()
            && crate::expand_system_path(file_path)?.is_none()
            && root.is_none()
        {
            return Err("历史文件已超出原工作区。".into());
        }
        Ok(target.to_string_lossy().into_owned())
    }
}
