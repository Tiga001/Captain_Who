use super::*;

pub(super) fn cleanup_fork_files(staged: &[(PathBuf, PathBuf)], committed: &[PathBuf]) {
    for (staging_path, _) in staged {
        let _ = fs::remove_file(staging_path);
    }
    for target_path in committed {
        let _ = fs::remove_file(target_path);
    }
}

impl StorageService {
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

    pub fn load_conversation(
        &self,
        conversation_id: &str,
    ) -> Result<Option<ChatConversationRecord>, String> {
        let connection = self.state.connection()?;
        let mut conversation = chat_repository::get_conversation(&connection, conversation_id)
            .map_err(storage_error)?;
        if let Some(conversation) = &mut conversation {
            self.attach_message_attachments(&connection, std::slice::from_mut(conversation))?;
        }
        Ok(conversation)
    }

    pub fn fork_conversation(
        &self,
        input: ForkConversationInput,
    ) -> Result<ChatConversationRecord, String> {
        let mut connection = self.state.connection()?;
        if let Some(existing) =
            conversation_fork_repository::find_existing_fork(&connection, input.request_id.trim())
                .map_err(storage_error)?
        {
            if existing.source_conversation_id != input.source_conversation_id
                || existing.source_message_id != input.through_assistant_message_id
            {
                return Err("同一个分叉请求 ID 不能用于不同的历史快照。".to_string());
            }
            let mut conversation =
                chat_repository::get_conversation(&connection, &existing.target_conversation_id)
                    .map_err(storage_error)?
                    .ok_or_else(|| "分叉记录指向的新任务不存在。".to_string())?;
            self.attach_message_attachments(&connection, std::slice::from_mut(&mut conversation))?;
            return Ok(conversation);
        }

        let mut plan =
            conversation_fork_repository::build_fork_plan(&connection, &input, now_ms())?;
        ensure_project_reference_exists(&connection, plan.target.project_id.as_deref())?;

        let mut staged_files = Vec::new();
        let mut committed_files = Vec::new();
        let prepare_files = (|| -> Result<(), String> {
            for attachment in &mut plan.attachments {
                let source_path = safe_existing_attachment_storage_path(
                    &self.attachment_root,
                    &attachment.source.storage_rel_path,
                )
                .ok_or_else(|| {
                    format!("原任务附件文件不存在：{}", attachment.source.original_name)
                })?;
                let target_rel_path = attachment_storage_rel_path(
                    &attachment.target.conversation_id,
                    &attachment.target.message_id,
                    &attachment.target.id,
                    &attachment.target.original_name,
                );
                attachment.target.storage_rel_path = slash_path(&target_rel_path);
                let target_path = self.attachment_root.join(&target_rel_path);
                let parent = target_path
                    .parent()
                    .ok_or_else(|| "新任务附件路径无效。".to_string())?;
                fs::create_dir_all(parent)
                    .map_err(|error| format!("创建新任务附件目录失败：{error}"))?;
                let staging_path = parent.join(format!(
                    ".{}.forking-{}",
                    safe_path_component(&attachment.target.id, "attachment"),
                    Uuid::new_v4()
                ));
                staged_files.push((staging_path.clone(), target_path));
                let copied = fs::copy(&source_path, &staging_path)
                    .map_err(|error| format!("复制附件失败：{error}"))?;
                if copied != attachment.source.size_bytes {
                    return Err(format!(
                        "复制附件时大小不一致：{}",
                        attachment.source.original_name
                    ));
                }
            }
            for (staging_path, target_path) in &staged_files {
                fs::rename(staging_path, target_path)
                    .map_err(|error| format!("提交新任务附件失败：{error}"))?;
                committed_files.push(target_path.clone());
            }
            Ok(())
        })();
        if let Err(error) = prepare_files {
            cleanup_fork_files(&staged_files, &committed_files);
            return Err(error);
        }

        if let Err(error) = conversation_fork_repository::commit_fork_plan(&mut connection, &plan) {
            cleanup_fork_files(&staged_files, &committed_files);
            return Err(error);
        }
        let mut conversation = chat_repository::get_conversation(&connection, &plan.target.id)
            .map_err(storage_error)?
            .ok_or_else(|| "新任务创建后无法重新读取。".to_string())?;
        self.attach_message_attachments(&connection, std::slice::from_mut(&mut conversation))?;
        Ok(conversation)
    }

    pub fn search_chats(&self, input: &ChatSearchInput) -> Result<Vec<ChatSearchResult>, String> {
        let connection = self.state.connection()?;
        chat_search_repository::search_chats(&connection, input).map_err(storage_error)
    }

    pub fn search_conversation_history(
        &self,
        conversation_id: &str,
        query: &str,
        include_messages: bool,
        include_trace_items: bool,
        limit: usize,
    ) -> Result<Vec<conversation_history_repository::ConversationHistorySearchHit>, String> {
        let connection = self.state.connection()?;
        conversation_history_repository::search_records(
            &connection,
            conversation_id,
            query,
            include_messages,
            include_trace_items,
            limit,
        )
        .map_err(storage_error)
    }

    pub fn read_conversation_history_record(
        &self,
        conversation_id: &str,
        reference: &conversation_history_repository::ConversationHistoryRecordRef,
    ) -> Result<Option<conversation_history_repository::ConversationHistoryRecord>, String> {
        let connection = self.state.connection()?;
        conversation_history_repository::read_record(&connection, conversation_id, reference)
            .map_err(storage_error)
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
}
