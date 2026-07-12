use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Component, Path, PathBuf};

use crate::storage::models::{
    AgentActionAuditRecord, AgentFileDraftChunkRecord, AgentFileDraftOperationRecord,
    AgentFileDraftRecord, AgentPendingActionRecord, AgentPromptPreferencesRecord,
    AgentUsageRecordInsert, AttachmentImageRecord, AttachmentRecord, ChatConversationMetaRecord,
    ChatConversationRecord, ChatMessageAttachmentRecord, ChatMessageRecord, ChatMessageStateRecord,
    ChatSearchInput, ChatSearchResult, ComposerDraftRecord, ModelSettingsRecord, ProjectRecord,
    UiPreferencesRecord,
};
use crate::storage::{
    agent_action_audit_repository, agent_prompt_preferences_repository, attachment_repository,
    chat_repository, chat_search_repository, composer_draft_repository, config_repository,
    file_draft_repository, now_ms, pending_action_repository, preferences_repository,
    project_repository, storage_error, usage_repository, StorageState,
};
use crate::{
    AgentAttachmentLibraryContext, AgentAttachmentReference, AgentInputAttachment,
    AgentInputAttachmentEncoding, AgentInputAttachmentKind, AgentToolResult, AgentUsageClearInput,
    AgentUsageClearOutput, AgentUsageSummaryInput, AgentUsageSummaryOutput,
};
use base64::Engine;

pub struct StorageService {
    state: StorageState,
    attachment_root: PathBuf,
}

fn validate_model_settings(settings: &ModelSettingsRecord) -> Result<(), String> {
    let mut model_ids = HashSet::new();
    for model in &settings.models {
        let model_id = model.id.trim();
        if model_id.is_empty() {
            return Err("模型 ID 不能为空。".to_string());
        }
        if !model_ids.insert(model_id) {
            return Err(format!("模型 ID 重复：{model_id}"));
        }
        if !usage_repository::is_valid_price_per_1k(&model.input_price) {
            return Err(format!(
                "模型 {model_id} 的输入价格必须是大于或等于 0 的有效数字。"
            ));
        }
        if !usage_repository::is_valid_price_per_1k(&model.output_price) {
            return Err(format!(
                "模型 {model_id} 的输出价格必须是大于或等于 0 的有效数字。"
            ));
        }
    }
    Ok(())
}

impl StorageService {
    pub fn open(database_path: &Path) -> Result<Self, Box<dyn std::error::Error>> {
        let attachment_root = database_path
            .parent()
            .map(|parent| parent.join("attachments"))
            .unwrap_or_else(|| PathBuf::from("attachments"));

        let service = Self {
            state: StorageState::open(database_path)?,
            attachment_root,
        };

        match service.state.connection() {
            Ok(mut connection) => {
                if let Err(error) =
                    file_draft_repository::expire_and_prune_drafts(&mut connection, now_ms())
                {
                    eprintln!("failed to prune expired file drafts: {error}");
                }
                if let Err(error) = service.cleanup_orphan_attachment_files(&connection) {
                    eprintln!("failed to cleanup orphan attachment files: {error}");
                }
            }
            Err(error) => eprintln!("failed to open storage for startup maintenance: {error}"),
        }

        Ok(service)
    }

    pub fn load_model_settings(&self) -> Result<Option<ModelSettingsRecord>, String> {
        let connection = self.state.connection()?;
        config_repository::load_model_settings(&connection).map_err(storage_error)
    }

    pub fn save_model_settings(&self, settings: ModelSettingsRecord) -> Result<(), String> {
        validate_model_settings(&settings)?;
        let mut connection = self.state.connection()?;
        config_repository::save_model_settings(&mut connection, settings).map_err(storage_error)
    }

    pub fn load_agent_prompt_preferences(&self) -> Result<AgentPromptPreferencesRecord, String> {
        let connection = self.state.connection()?;
        agent_prompt_preferences_repository::load_agent_prompt_preferences(&connection)
            .map_err(storage_error)
    }

    pub fn save_agent_prompt_preferences(
        &self,
        preferences: AgentPromptPreferencesRecord,
    ) -> Result<AgentPromptPreferencesRecord, String> {
        let connection = self.state.connection()?;
        agent_prompt_preferences_repository::save_agent_prompt_preferences(&connection, preferences)
            .map_err(storage_error)
    }

    pub fn load_projects(&self) -> Result<Vec<ProjectRecord>, String> {
        let connection = self.state.connection()?;
        project_repository::list_projects(&connection).map_err(storage_error)
    }

    pub fn save_project(&self, project: ProjectRecord) -> Result<ProjectRecord, String> {
        let connection = self.state.connection()?;
        project_repository::save_project(&connection, project.clone()).map_err(storage_error)?;
        Ok(project)
    }

    pub fn delete_project(&self, project_id: &str) -> Result<(), String> {
        let mut connection = self.state.connection()?;
        let attachments =
            attachment_repository::list_project_deletion_attachments(&connection, project_id)
                .map_err(storage_error)?;
        {
            let transaction = connection.transaction().map_err(storage_error)?;
            usage_repository::roll_up_deleted_usage_for_project(&transaction, project_id, now_ms())
                .map_err(storage_error)?;
            pending_action_repository::delete_pending_actions_for_project(&transaction, project_id)
                .map_err(storage_error)?;
            agent_action_audit_repository::delete_action_audit_for_project(
                &transaction,
                project_id,
            )
            .map_err(storage_error)?;
            composer_draft_repository::delete_project_composer_drafts(&transaction, project_id)
                .map_err(storage_error)?;
            project_repository::delete_project(&transaction, project_id).map_err(storage_error)?;
            transaction.commit().map_err(storage_error)?;
        }
        if let Err(error) = self.cleanup_attachment_files(attachments) {
            eprintln!("failed to remove deleted project attachment files: {error}");
        }
        Ok(())
    }

    pub fn load_conversations(&self) -> Result<Vec<ChatConversationRecord>, String> {
        let connection = self.state.connection()?;
        let mut conversations =
            chat_repository::list_conversations(&connection).map_err(storage_error)?;
        self.attach_message_attachments(&connection, &mut conversations)?;
        Ok(conversations)
    }

    pub fn search_chats(&self, input: &ChatSearchInput) -> Result<Vec<ChatSearchResult>, String> {
        let connection = self.state.connection()?;
        chat_search_repository::search_chats(&connection, input).map_err(storage_error)
    }

    pub fn load_attachment_image(
        &self,
        attachment_id: &str,
    ) -> Result<Option<AttachmentImageRecord>, String> {
        let attachment_id = attachment_id.trim();
        if attachment_id.is_empty() {
            return Ok(None);
        }

        let connection = self.state.connection()?;
        let Some(attachment) = attachment_repository::get_attachment(&connection, attachment_id)
            .map_err(storage_error)?
        else {
            return Ok(None);
        };

        let Some(mime_type) = image_preview_mime_type(&attachment) else {
            return Ok(None);
        };
        let Some(storage_path) = safe_existing_attachment_storage_path(
            &self.attachment_root,
            &attachment.storage_rel_path,
        ) else {
            return Ok(None);
        };

        let bytes = match fs::read(storage_path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(format!("读取图片附件失败：{error}")),
        };

        Ok(Some(AttachmentImageRecord {
            id: attachment.id,
            name: attachment.original_name,
            mime_type,
            size_bytes: bytes.len() as u64,
            data: base64::engine::general_purpose::STANDARD.encode(bytes),
            created_at: attachment.created_at,
        }))
    }

    pub fn save_conversation(
        &self,
        conversation: ChatConversationRecord,
    ) -> Result<ChatConversationRecord, String> {
        let mut connection = self.state.connection()?;
        ensure_project_reference_exists(&connection, conversation.project_id.as_deref())?;
        chat_repository::save_conversation(&mut connection, conversation.clone())
            .map_err(storage_error)?;
        Ok(conversation)
    }

    pub fn save_conversation_meta(
        &self,
        conversation: ChatConversationMetaRecord,
    ) -> Result<ChatConversationMetaRecord, String> {
        let connection = self.state.connection()?;
        ensure_project_reference_exists(&connection, conversation.project_id.as_deref())?;
        chat_repository::save_conversation_meta(&connection, &conversation)
            .map_err(storage_error)?;
        Ok(conversation)
    }

    pub fn delete_conversation(&self, conversation_id: &str) -> Result<(), String> {
        let mut connection = self.state.connection()?;
        let attachments =
            attachment_repository::list_conversation_attachments(&connection, conversation_id)
                .map_err(storage_error)?;
        {
            let transaction = connection.transaction().map_err(storage_error)?;
            usage_repository::roll_up_deleted_usage_for_conversation(
                &transaction,
                conversation_id,
                now_ms(),
            )
            .map_err(storage_error)?;
            pending_action_repository::delete_pending_actions_for_conversation(
                &transaction,
                conversation_id,
            )
            .map_err(storage_error)?;
            agent_action_audit_repository::delete_action_audit_for_conversation(
                &transaction,
                conversation_id,
            )
            .map_err(storage_error)?;
            chat_repository::delete_conversation(&transaction, conversation_id)
                .map_err(storage_error)?;
            composer_draft_repository::delete_composer_draft(&transaction, conversation_id)
                .map_err(storage_error)?;
            transaction.commit().map_err(storage_error)?;
        }
        if let Err(error) = self.cleanup_attachment_files(attachments) {
            eprintln!("failed to remove deleted conversation attachment files: {error}");
        }
        Ok(())
    }

    pub fn delete_chat_messages(
        &self,
        conversation_id: &str,
        message_ids: &[String],
    ) -> Result<(), String> {
        if message_ids.is_empty() {
            return Ok(());
        }

        let mut connection = self.state.connection()?;
        let attachments = attachment_repository::list_message_attachments(
            &connection,
            conversation_id,
            message_ids,
        )
        .map_err(storage_error)?;
        attachment_repository::delete_message_attachments(
            &connection,
            conversation_id,
            message_ids,
        )
        .map_err(storage_error)?;
        chat_repository::delete_messages(&mut connection, conversation_id, message_ids)
            .map_err(storage_error)?;
        if let Err(error) = self.cleanup_attachment_files(attachments) {
            eprintln!("failed to remove deleted message attachment files: {error}");
        }
        Ok(())
    }

    pub fn save_input_attachments(
        &self,
        conversation_id: &str,
        message_id: &str,
        project_id: Option<&str>,
        attachments: &[AgentInputAttachment],
        created_at: i64,
    ) -> Result<(), String> {
        if attachments.is_empty() {
            return Ok(());
        }

        fs::create_dir_all(&self.attachment_root)
            .map_err(|error| format!("创建附件库目录失败：{error}"))?;
        let connection = self.state.connection()?;
        ensure_conversation_exists(&connection, conversation_id)?;
        ensure_project_reference_exists(&connection, project_id)?;

        for attachment in attachments {
            let bytes = input_attachment_bytes(attachment)?;
            let attachment_id = safe_path_component(&attachment.id, "attachment");
            let storage_rel_path = attachment_storage_rel_path(
                conversation_id,
                message_id,
                &attachment_id,
                &attachment.name,
            );
            let storage_path = self.attachment_root.join(&storage_rel_path);
            let parent = storage_path
                .parent()
                .ok_or_else(|| "附件存储路径无效。".to_string())?;
            fs::create_dir_all(parent).map_err(|error| format!("创建附件目录失败：{error}"))?;
            fs::write(&storage_path, &bytes).map_err(|error| format!("写入附件失败：{error}"))?;

            let record = AttachmentRecord {
                id: attachment_id,
                conversation_id: conversation_id.to_string(),
                message_id: message_id.to_string(),
                project_id: project_id.map(ToString::to_string),
                kind: input_attachment_kind_label(attachment.kind).to_string(),
                original_name: attachment.name.clone(),
                mime_type: attachment.mime_type.clone(),
                size_bytes: bytes.len() as u64,
                storage_rel_path: slash_path(&storage_rel_path),
                created_at,
            };
            attachment_repository::save_attachment(&connection, &record).map_err(storage_error)?;
        }

        Ok(())
    }

    pub fn load_input_attachments(
        &self,
        attachment_ids: &[String],
    ) -> Result<Vec<AgentInputAttachment>, String> {
        let connection = self.state.connection()?;
        let mut attachments = Vec::new();

        for attachment_id in attachment_ids {
            let attachment = attachment_repository::get_attachment(&connection, attachment_id)
                .map_err(storage_error)?
                .ok_or_else(|| format!("附件不存在：{attachment_id}"))?;
            let storage_path = safe_existing_attachment_storage_path(
                &self.attachment_root,
                &attachment.storage_rel_path,
            )
            .ok_or_else(|| format!("附件文件不存在：{}", attachment.original_name))?;
            let bytes = fs::read(&storage_path)
                .map_err(|error| format!("读取附件失败 {}: {error}", storage_path.display()))?;

            attachments.push(AgentInputAttachment {
                id: attachment.id,
                kind: agent_attachment_kind(&attachment.kind),
                name: attachment.original_name,
                mime_type: attachment.mime_type,
                size_bytes: attachment.size_bytes,
                encoding: AgentInputAttachmentEncoding::Base64,
                data: base64::engine::general_purpose::STANDARD.encode(bytes),
                truncated: None,
            });
        }

        Ok(attachments)
    }

    pub fn build_attachment_library_context(
        &self,
        conversation_id: &str,
        project_id: Option<&str>,
    ) -> Result<AgentAttachmentLibraryContext, String> {
        let connection = self.state.connection()?;
        let conversation_attachments =
            attachment_repository::list_conversation_attachments(&connection, conversation_id)
                .map_err(storage_error)?
                .into_iter()
                .map(agent_attachment_reference)
                .collect::<Vec<_>>();
        let project_attachments = if let Some(project_id) = project_id {
            attachment_repository::list_project_attachments_excluding_conversation(
                &connection,
                project_id,
                conversation_id,
            )
            .map_err(storage_error)?
            .into_iter()
            .map(agent_attachment_reference)
            .collect::<Vec<_>>()
        } else {
            Vec::new()
        };

        Ok(AgentAttachmentLibraryContext {
            root_path: Some(self.attachment_root.to_string_lossy().to_string()),
            conversation_id: Some(conversation_id.to_string()),
            project_id: project_id.map(ToString::to_string),
            conversation_attachments,
            project_attachments,
        })
    }

    pub fn upsert_chat_messages(
        &self,
        conversation_id: &str,
        messages: Vec<ChatMessageRecord>,
        position_offset: i64,
    ) -> Result<Vec<ChatMessageRecord>, String> {
        let mut connection = self.state.connection()?;
        ensure_conversation_exists(&connection, conversation_id)?;
        chat_repository::upsert_messages(
            &mut connection,
            conversation_id,
            &messages,
            position_offset,
        )
        .map_err(storage_error)?;
        Ok(messages)
    }

    pub fn save_chat_message_state(
        &self,
        conversation_id: &str,
        message: ChatMessageStateRecord,
    ) -> Result<(), String> {
        let connection = self.state.connection()?;
        chat_repository::update_message_state(&connection, conversation_id, &message)
            .map_err(storage_error)
    }

    pub fn update_chat_message_status_and_content(
        &self,
        conversation_id: &str,
        message_id: &str,
        content: &str,
        status: Option<&str>,
        updated_at: i64,
    ) -> Result<(), String> {
        let connection = self.state.connection()?;
        chat_repository::update_message_status_and_content(
            &connection,
            conversation_id,
            message_id,
            content,
            status,
            updated_at,
        )
        .map_err(storage_error)
    }

    pub fn update_chat_message_run_terminal_state(
        &self,
        conversation_id: &str,
        message_id: &str,
        message_status: Option<&str>,
        run_status: &str,
        completed_at: i64,
    ) -> Result<(), String> {
        let connection = self.state.connection()?;
        chat_repository::update_message_run_terminal_state(
            &connection,
            conversation_id,
            message_id,
            message_status,
            run_status,
            completed_at,
        )
        .map_err(storage_error)
    }

    pub fn load_composer_drafts(&self) -> Result<Vec<ComposerDraftRecord>, String> {
        let connection = self.state.connection()?;
        composer_draft_repository::list_composer_drafts(&connection).map_err(storage_error)
    }

    pub fn save_composer_draft(
        &self,
        draft: ComposerDraftRecord,
    ) -> Result<ComposerDraftRecord, String> {
        let connection = self.state.connection()?;
        ensure_project_reference_exists(&connection, draft.project_id.as_deref())?;
        composer_draft_repository::save_composer_draft(&connection, draft.clone())
            .map_err(storage_error)?;
        Ok(draft)
    }

    pub fn load_ui_preferences(&self) -> Result<UiPreferencesRecord, String> {
        let connection = self.state.connection()?;
        preferences_repository::load_ui_preferences(&connection).map_err(storage_error)
    }

    pub fn save_ui_preferences(
        &self,
        preferences: UiPreferencesRecord,
    ) -> Result<UiPreferencesRecord, String> {
        let connection = self.state.connection()?;
        preferences_repository::save_ui_preferences(&connection, preferences).map_err(storage_error)
    }

    pub fn upsert_agent_usage(&self, record: AgentUsageRecordInsert) -> Result<(), String> {
        let connection = self.state.connection()?;
        usage_repository::upsert_usage_record(&connection, &record).map_err(storage_error)
    }

    pub fn get_usage_summary(
        &self,
        input: &AgentUsageSummaryInput,
        now_ms: i64,
    ) -> Result<AgentUsageSummaryOutput, String> {
        let connection = self.state.connection()?;
        usage_repository::usage_summary(&connection, input, now_ms).map_err(storage_error)
    }

    pub fn clear_usage_records(
        &self,
        input: &AgentUsageClearInput,
    ) -> Result<AgentUsageClearOutput, String> {
        let connection = self.state.connection()?;
        usage_repository::clear_usage_records(&connection, input).map_err(storage_error)
    }

    pub fn estimate_usage_cost(
        &self,
        input_tokens: Option<u64>,
        output_tokens: Option<u64>,
        input_price: &str,
        output_price: &str,
    ) -> Option<f64> {
        usage_repository::estimate_usage_cost(
            input_tokens,
            output_tokens,
            input_price,
            output_price,
        )
    }

    pub fn upsert_agent_action_audit(&self, record: AgentActionAuditRecord) -> Result<(), String> {
        let connection = self.state.connection()?;
        agent_action_audit_repository::upsert_action_audit_record(&connection, &record)
            .map_err(storage_error)
    }

    pub fn upsert_pending_agent_action(
        &self,
        record: AgentPendingActionRecord,
    ) -> Result<(), String> {
        let connection = self.state.connection()?;
        pending_action_repository::upsert_pending_action(&connection, &record)
            .map_err(storage_error)
    }

    pub fn list_pending_agent_actions(&self) -> Result<Vec<AgentPendingActionRecord>, String> {
        let connection = self.state.connection()?;
        pending_action_repository::list_pending_actions(&connection).map_err(storage_error)
    }

    pub fn update_pending_agent_action_status(
        &self,
        action_id: &str,
        status: &str,
        updated_at: i64,
    ) -> Result<(), String> {
        let connection = self.state.connection()?;
        pending_action_repository::update_pending_action_status(
            &connection,
            action_id,
            status,
            updated_at,
        )
        .map(|_| ())
        .map_err(storage_error)
    }

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

    fn attach_message_attachments(
        &self,
        connection: &rusqlite::Connection,
        conversations: &mut [ChatConversationRecord],
    ) -> Result<(), String> {
        for conversation in conversations {
            let attachments =
                attachment_repository::list_conversation_attachments(connection, &conversation.id)
                    .map_err(storage_error)?;
            if attachments.is_empty() {
                continue;
            }

            let mut attachments_by_message_id: HashMap<String, Vec<ChatMessageAttachmentRecord>> =
                HashMap::new();
            for attachment in attachments {
                attachments_by_message_id
                    .entry(attachment.message_id.clone())
                    .or_default()
                    .push(self.chat_message_attachment_record(attachment));
            }

            for message in &mut conversation.messages {
                if let Some(attachments) = attachments_by_message_id.remove(&message.id) {
                    message.attachments = attachments;
                }
            }
        }

        Ok(())
    }

    fn chat_message_attachment_record(
        &self,
        attachment: AttachmentRecord,
    ) -> ChatMessageAttachmentRecord {
        let preview_mime_type = image_preview_mime_type(&attachment);
        let preview_data = preview_mime_type
            .as_ref()
            .and_then(|_| self.read_attachment_preview_data(&attachment));

        ChatMessageAttachmentRecord {
            id: attachment.id,
            kind: attachment.kind,
            name: attachment.original_name,
            mime_type: attachment.mime_type,
            size_bytes: attachment.size_bytes,
            preview_data,
            preview_mime_type,
            created_at: attachment.created_at,
        }
    }

    fn read_attachment_preview_data(&self, attachment: &AttachmentRecord) -> Option<String> {
        let storage_path = safe_existing_attachment_storage_path(
            &self.attachment_root,
            &attachment.storage_rel_path,
        )?;
        let bytes = fs::read(storage_path).ok()?;
        Some(base64::engine::general_purpose::STANDARD.encode(bytes))
    }

    fn cleanup_attachment_files(&self, attachments: Vec<AttachmentRecord>) -> Result<(), String> {
        let mut errors = Vec::new();

        for attachment in attachments {
            let Some(storage_path) =
                safe_attachment_storage_path(&self.attachment_root, &attachment.storage_rel_path)
            else {
                errors.push(format!("附件路径无效：{}", attachment.storage_rel_path));
                continue;
            };

            match fs::remove_file(&storage_path) {
                Ok(()) => self.prune_empty_attachment_dirs(storage_path.parent(), &mut errors),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    self.prune_empty_attachment_dirs(storage_path.parent(), &mut errors);
                }
                Err(error) => errors.push(format!("{}: {error}", attachment.storage_rel_path)),
            }
        }

        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors.join("; "))
        }
    }

    fn cleanup_orphan_attachment_files(
        &self,
        connection: &rusqlite::Connection,
    ) -> Result<(), String> {
        if !self.attachment_root.exists() {
            return Ok(());
        }

        let referenced_paths = attachment_repository::list_attachment_storage_rel_paths(connection)
            .map_err(storage_error)?
            .into_iter()
            .collect::<HashSet<_>>();
        let mut errors = Vec::new();
        self.cleanup_orphan_attachment_dir(&self.attachment_root, &referenced_paths, &mut errors);

        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors.join("; "))
        }
    }

    fn cleanup_orphan_attachment_dir(
        &self,
        current_dir: &Path,
        referenced_paths: &HashSet<String>,
        errors: &mut Vec<String>,
    ) {
        let entries = match fs::read_dir(current_dir) {
            Ok(entries) => entries,
            Err(error) => {
                errors.push(format!(
                    "读取附件目录失败 {}: {error}",
                    current_dir.display()
                ));
                return;
            }
        };

        for entry in entries {
            let entry = match entry {
                Ok(entry) => entry,
                Err(error) => {
                    errors.push(format!(
                        "读取附件目录项失败 {}: {error}",
                        current_dir.display()
                    ));
                    continue;
                }
            };
            let path = entry.path();
            let metadata = match fs::symlink_metadata(&path) {
                Ok(metadata) => metadata,
                Err(error) => {
                    errors.push(format!("读取附件路径失败 {}: {error}", path.display()));
                    continue;
                }
            };
            let file_type = metadata.file_type();

            if file_type.is_dir() {
                self.cleanup_orphan_attachment_dir(&path, referenced_paths, errors);
                self.remove_empty_attachment_dir(&path, errors);
                continue;
            }

            if !file_type.is_file() && !file_type.is_symlink() {
                continue;
            }

            let Some(relative_path) = orphan_scan_relative_path(&self.attachment_root, &path)
            else {
                errors.push(format!("附件路径不在附件目录内：{}", path.display()));
                continue;
            };

            if referenced_paths.contains(&relative_path) {
                continue;
            }

            match fs::remove_file(&path) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => errors.push(format!("删除孤儿附件失败 {relative_path}: {error}")),
            }
        }
    }

    fn prune_empty_attachment_dirs(&self, start: Option<&Path>, errors: &mut Vec<String>) {
        let Some(mut current) = start.map(Path::to_path_buf) else {
            return;
        };

        while current != self.attachment_root {
            match fs::remove_dir(&current) {
                Ok(()) => {}
                Err(error)
                    if matches!(
                        error.kind(),
                        std::io::ErrorKind::NotFound | std::io::ErrorKind::DirectoryNotEmpty
                    ) =>
                {
                    break;
                }
                Err(error) => {
                    errors.push(format!("{}: {error}", current.display()));
                    break;
                }
            }

            if !current.pop() {
                break;
            }
        }
    }

    fn remove_empty_attachment_dir(&self, path: &Path, errors: &mut Vec<String>) {
        if path == self.attachment_root {
            return;
        }

        match fs::remove_dir(path) {
            Ok(()) => {}
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::NotFound | std::io::ErrorKind::DirectoryNotEmpty
                ) => {}
            Err(error) => errors.push(format!("删除空附件目录失败 {}: {error}", path.display())),
        }
    }
}

fn input_attachment_bytes(attachment: &AgentInputAttachment) -> Result<Vec<u8>, String> {
    match attachment.encoding {
        AgentInputAttachmentEncoding::Utf8 => Ok(attachment.data.as_bytes().to_vec()),
        AgentInputAttachmentEncoding::Base64 => base64::engine::general_purpose::STANDARD
            .decode(attachment.data.as_bytes())
            .map_err(|error| format!("附件 base64 数据无效：{error}")),
    }
}

fn input_attachment_kind_label(kind: AgentInputAttachmentKind) -> &'static str {
    match kind {
        AgentInputAttachmentKind::File => "file",
        AgentInputAttachmentKind::Image => "image",
    }
}

fn agent_attachment_kind(kind: &str) -> AgentInputAttachmentKind {
    match kind {
        "image" => AgentInputAttachmentKind::Image,
        _ => AgentInputAttachmentKind::File,
    }
}

fn agent_attachment_reference(record: AttachmentRecord) -> AgentAttachmentReference {
    let read_path = attachment_read_path(&record.id, &record.original_name);
    AgentAttachmentReference {
        id: record.id,
        conversation_id: record.conversation_id,
        message_id: record.message_id,
        project_id: record.project_id,
        kind: agent_attachment_kind(&record.kind),
        name: record.original_name,
        mime_type: record.mime_type,
        size_bytes: record.size_bytes,
        read_path,
        storage_rel_path: record.storage_rel_path,
        created_at: record.created_at,
    }
}

fn attachment_storage_rel_path(
    conversation_id: &str,
    message_id: &str,
    attachment_id: &str,
    original_name: &str,
) -> PathBuf {
    PathBuf::from("conversations")
        .join(safe_path_component(conversation_id, "conversation"))
        .join(safe_path_component(message_id, "message"))
        .join(safe_path_component(attachment_id, "attachment"))
        .join(safe_file_name(original_name, attachment_id))
}

fn attachment_read_path(attachment_id: &str, original_name: &str) -> String {
    format!(
        "@attachments/{}/{}",
        safe_path_component(attachment_id, "attachment"),
        safe_file_name(original_name, attachment_id)
    )
}

fn image_preview_mime_type(attachment: &AttachmentRecord) -> Option<String> {
    if attachment.kind != "image" {
        return None;
    }

    attachment
        .mime_type
        .as_deref()
        .filter(|mime_type| mime_type.starts_with("image/"))
        .map(ToString::to_string)
}

fn safe_existing_attachment_storage_path(
    attachment_root: &Path,
    storage_rel_path: &str,
) -> Option<PathBuf> {
    let storage_path = safe_attachment_storage_path(attachment_root, storage_rel_path)?;
    let root = attachment_root.canonicalize().ok()?;
    let canonical = storage_path.canonicalize().ok()?;
    if canonical.starts_with(root) {
        Some(canonical)
    } else {
        None
    }
}

fn safe_attachment_storage_path(attachment_root: &Path, storage_rel_path: &str) -> Option<PathBuf> {
    let relative_path = Path::new(storage_rel_path);
    if relative_path.is_absolute() {
        return None;
    }
    if relative_path.components().any(|component| {
        matches!(
            component,
            Component::ParentDir | Component::Prefix(_) | Component::RootDir
        )
    }) {
        return None;
    }

    Some(attachment_root.join(relative_path))
}

fn safe_file_name(name: &str, fallback: &str) -> String {
    let file_name = Path::new(name)
        .file_name()
        .and_then(|value| value.to_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(fallback);

    safe_path_component(file_name, fallback)
}

fn safe_path_component(value: &str, fallback: &str) -> String {
    let sanitized = value
        .trim()
        .chars()
        .map(|character| match character {
            '/' | '\\' | ':' | '\0' => '_',
            character if character.is_control() => '_',
            character => character,
        })
        .collect::<String>();

    let sanitized = sanitized.trim();
    if sanitized.is_empty() || sanitized == "." || sanitized == ".." {
        fallback.to_string()
    } else {
        sanitized.to_string()
    }
}

fn slash_path(path: &Path) -> String {
    path.components()
        .filter_map(|component| match component {
            Component::Normal(part) => Some(part.to_string_lossy().to_string()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("/")
}

fn ensure_project_reference_exists(
    connection: &rusqlite::Connection,
    project_id: Option<&str>,
) -> Result<(), String> {
    let Some(project_id) = project_id else {
        return Ok(());
    };
    if project_repository::project_exists(connection, project_id).map_err(storage_error)? {
        Ok(())
    } else {
        Err(format!("项目已不存在，拒绝保存关联数据：{project_id}"))
    }
}

fn ensure_conversation_exists(
    connection: &rusqlite::Connection,
    conversation_id: &str,
) -> Result<(), String> {
    if chat_repository::conversation_exists(connection, conversation_id).map_err(storage_error)? {
        Ok(())
    } else {
        Err(format!("对话已不存在，拒绝保存关联数据：{conversation_id}"))
    }
}

fn orphan_scan_relative_path(attachment_root: &Path, path: &Path) -> Option<String> {
    let relative_path = path.strip_prefix(attachment_root).ok()?;
    let parts = relative_path
        .components()
        .map(|component| match component {
            Component::Normal(part) => Some(part.to_string_lossy().to_string()),
            _ => None,
        })
        .collect::<Option<Vec<_>>>()?;
    Some(parts.join("/"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::models::{
        ChatConversationMetaRecord, ChatConversationRecord, ChatMessageRecord, ComposerDraftRecord,
        ModelConfigRecord, ModelSettingsRecord, ProjectRecord,
    };
    use std::sync::atomic::{AtomicU64, Ordering};

    static TEST_COUNTER: AtomicU64 = AtomicU64::new(1);

    #[test]
    fn rejects_invalid_model_prices_without_overwriting_saved_settings() {
        let fixture = StorageFixture::new();
        let service = fixture.service();
        let valid = ModelSettingsRecord {
            api_url: "https://example.com".to_string(),
            api_token: "token".to_string(),
            search_mode: "disabled".to_string(),
            tavily_api_key: String::new(),
            models: vec![ModelConfigRecord {
                id: "model-a".to_string(),
                display_name: "Model A".to_string(),
                short_name: None,
                provider_path: None,
                supports_image: false,
                input_price: "0.01".to_string(),
                output_price: "0.02".to_string(),
                enabled: true,
            }],
        };
        service.save_model_settings(valid.clone()).unwrap();

        let mut invalid = valid;
        invalid.models[0].input_price = "not-a-price".to_string();
        assert!(service.save_model_settings(invalid).is_err());

        let stored = service.load_model_settings().unwrap().unwrap();
        assert_eq!(stored.models[0].input_price, "0.01");
    }

    #[test]
    fn input_attachments_are_persisted_and_rehydrated() {
        let fixture = StorageFixture::new();
        let service = fixture.service();
        service
            .save_conversation(conversation(
                "conversation-1",
                Some("project-1"),
                "message-1",
            ))
            .unwrap();
        service
            .save_input_attachments(
                "conversation-1",
                "message-1",
                Some("project-1"),
                &[input_attachment(
                    "attachment-1",
                    AgentInputAttachmentKind::Image,
                    "pixel.png",
                    Some("image/png"),
                    b"png-bytes",
                )],
                10,
            )
            .unwrap();

        let conversations = service.load_conversations().unwrap();
        let attachment = &conversations[0].messages[0].attachments[0];

        assert_eq!(attachment.id, "attachment-1");
        assert_eq!(attachment.kind, "image");
        assert_eq!(attachment.name, "pixel.png");
        assert_eq!(attachment.preview_mime_type.as_deref(), Some("image/png"));
        assert!(attachment.preview_data.is_some());

        let library = service
            .build_attachment_library_context("conversation-1", Some("project-1"))
            .unwrap();
        assert_eq!(library.conversation_attachments.len(), 1);
        assert_eq!(
            library.conversation_attachments[0].read_path,
            "@attachments/attachment-1/pixel.png"
        );
        assert!(PathBuf::from(library.root_path.unwrap())
            .join(&library.conversation_attachments[0].storage_rel_path)
            .is_file());
    }

    #[test]
    fn opening_storage_removes_orphan_attachments_and_preserves_referenced_files() {
        let fixture = StorageFixture::new();
        let service = fixture.service();
        service
            .save_conversation(conversation("conversation-1", None, "message-1"))
            .unwrap();
        service
            .save_input_attachments(
                "conversation-1",
                "message-1",
                None,
                &[input_attachment(
                    "referenced",
                    AgentInputAttachmentKind::File,
                    "referenced.txt",
                    Some("text/plain"),
                    b"referenced",
                )],
                10,
            )
            .unwrap();
        let library = service
            .build_attachment_library_context("conversation-1", None)
            .unwrap();
        let referenced_path = PathBuf::from(library.root_path.unwrap())
            .join(&library.conversation_attachments[0].storage_rel_path);
        drop(service);

        let orphan_path = fixture.root.join("attachments/orphan/nested.txt");
        fs::create_dir_all(orphan_path.parent().unwrap()).unwrap();
        fs::write(&orphan_path, b"orphan").unwrap();

        let _reopened = fixture.service();
        assert!(referenced_path.is_file());
        assert!(!orphan_path.exists());
    }

    #[test]
    fn project_attachment_library_excludes_current_conversation_and_delete_cleans_files() {
        let fixture = StorageFixture::new();
        let service = fixture.service();
        service
            .save_conversation(conversation(
                "conversation-current",
                Some("project-1"),
                "message-current",
            ))
            .unwrap();
        service
            .save_conversation(conversation(
                "conversation-other",
                Some("project-1"),
                "message-other",
            ))
            .unwrap();
        service
            .save_input_attachments(
                "conversation-current",
                "message-current",
                Some("project-1"),
                &[input_attachment(
                    "current",
                    AgentInputAttachmentKind::File,
                    "current.txt",
                    Some("text/plain"),
                    b"current",
                )],
                10,
            )
            .unwrap();
        service
            .save_input_attachments(
                "conversation-other",
                "message-other",
                Some("project-1"),
                &[input_attachment(
                    "other",
                    AgentInputAttachmentKind::File,
                    "other.txt",
                    Some("text/plain"),
                    b"other",
                )],
                20,
            )
            .unwrap();

        let library = service
            .build_attachment_library_context("conversation-current", Some("project-1"))
            .unwrap();

        assert_eq!(library.conversation_attachments.len(), 1);
        assert_eq!(library.conversation_attachments[0].id, "current");
        assert_eq!(library.project_attachments.len(), 1);
        assert_eq!(library.project_attachments[0].id, "other");

        let other_path = PathBuf::from(library.root_path.unwrap())
            .join(&library.project_attachments[0].storage_rel_path);
        assert!(other_path.is_file());

        service.delete_conversation("conversation-other").unwrap();

        assert!(!other_path.exists());
        let library = service
            .build_attachment_library_context("conversation-current", Some("project-1"))
            .unwrap();
        assert!(library.project_attachments.is_empty());
    }

    #[test]
    fn deleting_conversation_and_project_removes_composer_drafts() {
        let fixture = StorageFixture::new();
        let service = fixture.service();
        service
            .save_conversation(conversation(
                "conversation-1",
                Some("project-1"),
                "message-1",
            ))
            .unwrap();
        service
            .save_conversation(conversation(
                "conversation-2",
                Some("project-1"),
                "message-2",
            ))
            .unwrap();
        service
            .save_conversation(conversation(
                "conversation-3",
                Some("project-2"),
                "message-3",
            ))
            .unwrap();

        service
            .save_composer_draft(composer_draft(
                "conversation-1",
                Some("project-1"),
                "draft 1",
            ))
            .unwrap();
        service
            .save_composer_draft(composer_draft(
                "conversation-2",
                Some("project-1"),
                "draft 2",
            ))
            .unwrap();
        service
            .save_composer_draft(composer_draft(
                "new-conversation-project-1",
                Some("project-1"),
                "new draft",
            ))
            .unwrap();
        service
            .save_composer_draft(composer_draft(
                "conversation-3",
                Some("project-2"),
                "draft 3",
            ))
            .unwrap();

        service.delete_conversation("conversation-1").unwrap();

        let mut scopes = service
            .load_composer_drafts()
            .unwrap()
            .into_iter()
            .map(|draft| draft.scope_id)
            .collect::<Vec<_>>();
        scopes.sort();
        assert_eq!(
            scopes,
            vec![
                "conversation-2".to_string(),
                "conversation-3".to_string(),
                "new-conversation-project-1".to_string()
            ]
        );

        service.delete_project("project-1").unwrap();

        let mut scopes = service
            .load_composer_drafts()
            .unwrap()
            .into_iter()
            .map(|draft| draft.scope_id)
            .collect::<Vec<_>>();
        scopes.sort();
        assert_eq!(scopes, vec!["conversation-3".to_string()]);
    }

    #[test]
    fn stale_saves_cannot_recreate_deleted_project_data() {
        let fixture = StorageFixture::new();
        let service = fixture.service();
        let stale_conversation =
            conversation("conversation-stale", Some("project-1"), "message-stale");
        service
            .save_conversation(stale_conversation.clone())
            .unwrap();
        service.delete_project("project-1").unwrap();

        assert!(service
            .save_conversation(stale_conversation.clone())
            .is_err());
        assert!(service
            .save_conversation_meta(ChatConversationMetaRecord {
                id: stale_conversation.id.clone(),
                project_id: stale_conversation.project_id.clone(),
                model_id: stale_conversation.model_id.clone(),
                title: stale_conversation.title.clone(),
                created_at: stale_conversation.created_at,
                updated_at: stale_conversation.updated_at,
                pinned_at: stale_conversation.pinned_at,
                archived_at: stale_conversation.archived_at,
                unread_at: stale_conversation.unread_at,
            })
            .is_err());
        assert!(service
            .upsert_chat_messages(
                &stale_conversation.id,
                stale_conversation.messages.clone(),
                0,
            )
            .is_err());
        assert!(service
            .save_composer_draft(composer_draft(
                &stale_conversation.id,
                Some("project-1"),
                "stale draft",
            ))
            .is_err());
        assert!(service.load_conversations().unwrap().is_empty());
        assert!(service.load_composer_drafts().unwrap().is_empty());
    }

    #[test]
    fn deleting_conversation_removes_agent_rows_and_keeps_usage_rollup() {
        let fixture = StorageFixture::new();
        let service = fixture.service();
        service
            .save_conversation(conversation(
                "conversation-1",
                Some("project-1"),
                "message-1",
            ))
            .unwrap();
        service
            .upsert_agent_usage(agent_usage_record("conversation-1", "message-1"))
            .unwrap();
        service
            .upsert_pending_agent_action(pending_action("action-1", "conversation-1"))
            .unwrap();
        service
            .upsert_agent_action_audit(action_audit("action-1", "conversation-1"))
            .unwrap();

        service.delete_conversation("conversation-1").unwrap();

        let summary = service
            .get_usage_summary(
                &crate::AgentUsageSummaryInput {
                    range: crate::AgentUsageSummaryRange::All,
                    from: None,
                    to: None,
                },
                200_000,
            )
            .unwrap();
        assert_eq!(summary.request_count, 1);
        assert_eq!(summary.message_count, 1);
        assert_eq!(summary.input_tokens, Some(12));
        assert_eq!(summary.output_tokens, Some(8));
        assert_eq!(summary.total_tokens, Some(20));

        let connection = service.state.connection().unwrap();
        let raw_usage_count: i64 = connection
            .query_row("SELECT COUNT(*) FROM agent_usage_records", [], |row| {
                row.get(0)
            })
            .unwrap();
        let rollup_count: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM agent_deleted_usage_daily_rollups",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let pending_count: i64 = connection
            .query_row("SELECT COUNT(*) FROM agent_pending_actions", [], |row| {
                row.get(0)
            })
            .unwrap();
        let audit_count: i64 = connection
            .query_row("SELECT COUNT(*) FROM agent_action_audit", [], |row| {
                row.get(0)
            })
            .unwrap();

        assert_eq!(raw_usage_count, 0);
        assert_eq!(rollup_count, 1);
        assert_eq!(pending_count, 0);
        assert_eq!(audit_count, 0);
    }

    struct StorageFixture {
        root: PathBuf,
    }

    impl StorageFixture {
        fn new() -> Self {
            let unique = TEST_COUNTER.fetch_add(1, Ordering::Relaxed);
            let root =
                std::env::temp_dir().join(format!("mycopilot-storage-attachment-test-{unique}"));
            let _ = fs::remove_dir_all(&root);
            fs::create_dir_all(&root).unwrap();
            Self { root }
        }

        fn service(&self) -> StorageService {
            let service = StorageService::open(&self.root.join("storage.sqlite")).unwrap();
            for project_id in ["project-1", "project-2"] {
                service
                    .save_project(ProjectRecord {
                        id: project_id.to_string(),
                        name: project_id.to_string(),
                        path: Some(self.root.join(project_id).to_string_lossy().to_string()),
                        created_at: 1,
                        pinned_at: None,
                    })
                    .unwrap();
            }
            service
        }
    }

    impl Drop for StorageFixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    fn conversation(
        id: &str,
        project_id: Option<&str>,
        message_id: &str,
    ) -> ChatConversationRecord {
        ChatConversationRecord {
            id: id.to_string(),
            project_id: project_id.map(ToString::to_string),
            model_id: Some("model-1".to_string()),
            title: id.to_string(),
            messages: vec![ChatMessageRecord {
                id: message_id.to_string(),
                role: "user".to_string(),
                content: "hello".to_string(),
                created_at: 1,
                status: Some("sent".to_string()),
                attachments: Vec::new(),
                agent_run_json: None,
                ui_state_json: None,
            }],
            created_at: 1,
            updated_at: 1,
            pinned_at: None,
            archived_at: None,
            unread_at: None,
        }
    }

    fn input_attachment(
        id: &str,
        kind: AgentInputAttachmentKind,
        name: &str,
        mime_type: Option<&str>,
        bytes: &[u8],
    ) -> AgentInputAttachment {
        AgentInputAttachment {
            id: id.to_string(),
            kind,
            name: name.to_string(),
            mime_type: mime_type.map(ToString::to_string),
            size_bytes: bytes.len() as u64,
            encoding: AgentInputAttachmentEncoding::Base64,
            data: base64::engine::general_purpose::STANDARD.encode(bytes),
            truncated: None,
        }
    }

    fn composer_draft(
        scope_id: &str,
        project_id: Option<&str>,
        message: &str,
    ) -> ComposerDraftRecord {
        ComposerDraftRecord {
            scope_id: scope_id.to_string(),
            message: message.to_string(),
            permission_mode: "workspace".to_string(),
            model_id: Some("model-1".to_string()),
            project_id: project_id.map(ToString::to_string),
            attachments_json: "[]".to_string(),
            updated_at: 1,
        }
    }

    fn agent_usage_record(conversation_id: &str, message_id: &str) -> AgentUsageRecordInsert {
        AgentUsageRecordInsert {
            id: format!("usage-{message_id}"),
            conversation_id: conversation_id.to_string(),
            message_id: message_id.to_string(),
            run_id: "run-1".to_string(),
            project_id: Some("project-1".to_string()),
            model_id: "model-1".to_string(),
            model_name: "Model 1".to_string(),
            provider_path: Some("provider/model-1".to_string()),
            started_at: Some(900),
            completed_at: Some(1_000),
            status: Some("completed".to_string()),
            error: None,
            created_at: 1_000,
            input_tokens: Some(12),
            output_tokens: Some(8),
            output_thinking_tokens: None,
            total_tokens: Some(20),
            cached_input_tokens: None,
            cache_creation_input_tokens: None,
            billable_request_count: 1,
            input_price: Some("0".to_string()),
            output_price: Some("0".to_string()),
            estimated_cost: Some(0.0),
        }
    }

    fn pending_action(action_id: &str, conversation_id: &str) -> AgentPendingActionRecord {
        AgentPendingActionRecord {
            action_id: action_id.to_string(),
            run_id: "run-1".to_string(),
            conversation_id: Some(conversation_id.to_string()),
            assistant_message_id: Some("message-1".to_string()),
            action_type: "command".to_string(),
            tool_name: "run_command".to_string(),
            tool_call_id: Some(action_id.to_string()),
            status: "pending".to_string(),
            action_json: "{}".to_string(),
            agent_input_json: "{}".to_string(),
            created_at: 1,
            updated_at: 1,
        }
    }

    fn action_audit(action_id: &str, conversation_id: &str) -> AgentActionAuditRecord {
        AgentActionAuditRecord {
            action_id: action_id.to_string(),
            run_id: "run-1".to_string(),
            conversation_id: Some(conversation_id.to_string()),
            assistant_message_id: Some("message-1".to_string()),
            action_type: "command".to_string(),
            tool_name: "run_command".to_string(),
            decision: Some("approved".to_string()),
            status: "completed".to_string(),
            action_json: "{}".to_string(),
            patch_result_json: None,
            command_result_json: None,
            tool_result_json: None,
            error: None,
            created_at: 1,
            decided_at: Some(2),
            completed_at: Some(3),
            effective_permissions_json: None,
            path_scope: None,
            command_cwd_scope: None,
            blocked_reason: None,
            decision_source: Some("manual".to_string()),
        }
    }
}
