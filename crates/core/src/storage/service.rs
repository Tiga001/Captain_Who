use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::path::{Component, Path, PathBuf};

use crate::storage::models::{
    AgentActionAuditRecord, AgentFileDraftChunkRecord, AgentFileDraftOperationRecord,
    AgentFileDraftRecord, AgentPendingActionRecord, AgentPromptPreferencesRecord,
    AgentUsageRecordInsert, AttachmentImageRecord, AttachmentRecord, ChatConversationMetaRecord,
    ChatConversationRecord, ChatMessageAttachmentRecord, ChatMessageRecord, ChatMessageStateRecord,
    ChatSearchInput, ChatSearchResult, ComposerDraftRecord, ForkConversationInput,
    ModelSettingsRecord, ProjectRecord, UiPreferencesRecord,
};
use crate::storage::{
    agent_action_audit_repository, agent_prompt_preferences_repository, attachment_repository,
    chat_repository, chat_search_repository, composer_draft_repository, config_repository,
    context_compaction_audit_repository, context_compaction_receipt_repository,
    context_compaction_repository, conversation_fork_repository, conversation_history_repository,
    conversation_trace_repository, file_draft_repository, model_request_observation_repository,
    now_ms, pending_action_repository, preferences_repository, project_repository,
    skill_enablement_repository, storage_error, usage_repository, StorageState,
};
use crate::{
    AgentAttachmentLibraryContext, AgentAttachmentReference, AgentChatInput, AgentInputAttachment,
    AgentInputAttachmentEncoding, AgentInputAttachmentKind, AgentProposedAction, AgentToolResult,
    AgentUsageClearInput, AgentUsageClearOutput, AgentUsageSummaryInput, AgentUsageSummaryOutput,
    ContextCompactionAuditBundle, ContextCompactionPrefix, ContextCompactionReceipt,
    ContextCompactionSummary, ContextCompactionSummaryDraft, ContextJournalCursor,
    ConversationTurnTrace, ConversationTurnTraceItem, ModelRequestObservation,
};
use base64::Engine;
use rusqlite::OptionalExtension;
use uuid::Uuid;

const MAX_SKILL_ENABLEMENT_ID_BYTES: usize = 16 * 1024;

fn is_valid_pending_successor(
    interrupted: &AgentPendingActionRecord,
    candidate: &AgentPendingActionRecord,
) -> bool {
    if !is_pending_successor_candidate(interrupted, candidate) {
        return false;
    }
    let Ok(action) = serde_json::from_str::<AgentProposedAction>(&candidate.action_json) else {
        return false;
    };
    let action_id = match &action {
        AgentProposedAction::ToolCall { call } => call.id.as_str(),
        AgentProposedAction::Diff { diff } => diff.id.as_str(),
        AgentProposedAction::FileWrite { file_write } => file_write.id.as_str(),
        AgentProposedAction::Command { command } => command.id.as_str(),
    };
    if candidate.tool_call_id.as_deref() != Some(action_id) {
        return false;
    }
    let Ok(input) = serde_json::from_str::<AgentChatInput>(&candidate.agent_input_json) else {
        return false;
    };
    let Some(checkpoint) = input.resume_checkpoint.as_ref() else {
        return false;
    };
    if checkpoint.run_id != candidate.run_id || checkpoint.pending_tool_call_id != action_id {
        return false;
    }
    let Some(parent_call_id) = interrupted.tool_call_id.as_deref() else {
        return false;
    };
    let parent_result_sequence =
        checkpoint
            .conversation_trace_items
            .iter()
            .find_map(|item| match item {
                ConversationTurnTraceItem::ToolResult {
                    sequence, call_id, ..
                } if call_id == parent_call_id => Some(*sequence),
                _ => None,
            });
    let child_call_sequence =
        checkpoint
            .conversation_trace_items
            .iter()
            .find_map(|item| match item {
                ConversationTurnTraceItem::ToolCall {
                    sequence, call_id, ..
                } if call_id == action_id => Some(*sequence),
                _ => None,
            });
    matches!(
        (parent_result_sequence, child_call_sequence),
        (Some(parent), Some(child)) if parent < child
    )
}

fn is_pending_successor_candidate(
    interrupted: &AgentPendingActionRecord,
    candidate: &AgentPendingActionRecord,
) -> bool {
    candidate.status == "pending"
        && candidate.action_id != interrupted.action_id
        && candidate.run_id == interrupted.run_id
        && candidate.conversation_id == interrupted.conversation_id
        && candidate.assistant_message_id == interrupted.assistant_message_id
}

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
        if model.context_window_tokens == Some(0) {
            return Err(format!("模型 {model_id} 的上下文窗口必须大于 0。"));
        }
        model.connection_override()?;
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

fn cleanup_fork_files(staged: &[(PathBuf, PathBuf)], committed: &[PathBuf]) {
    for (staging_path, _) in staged {
        let _ = fs::remove_file(staging_path);
    }
    for target_path in committed {
        let _ = fs::remove_file(target_path);
    }
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
                if let Err(error) =
                    context_compaction_receipt_repository::mark_in_progress_receipts_interrupted(
                        &mut connection,
                        now_ms(),
                    )
                {
                    eprintln!("failed to mark interrupted context compactions: {error}");
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

    pub fn get_assistant_message_created_at(
        &self,
        conversation_id: &str,
        message_id: &str,
    ) -> Result<Option<i64>, String> {
        let connection = self.state.connection()?;
        chat_repository::get_assistant_message_created_at(&connection, conversation_id, message_id)
            .map_err(storage_error)
    }

    pub fn replace_conversation_turn_trace(
        &self,
        trace: &ConversationTurnTrace,
        created_at: i64,
        completed_at: i64,
    ) -> Result<(), String> {
        let mut connection = self.state.connection()?;
        conversation_trace_repository::replace_trace(
            &mut connection,
            trace,
            created_at,
            completed_at,
        )
        .map_err(storage_error)
    }

    pub fn append_in_progress_conversation_turn_trace(
        &self,
        trace: &ConversationTurnTrace,
        created_at: i64,
        updated_at: i64,
    ) -> Result<bool, String> {
        let mut connection = self.state.connection()?;
        conversation_trace_repository::append_in_progress_trace(
            &mut connection,
            trace,
            created_at,
            updated_at,
        )
        .map_err(storage_error)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn finalize_chat_message_with_conversation_trace(
        &self,
        conversation_id: &str,
        message_id: &str,
        content: &str,
        message_status: Option<&str>,
        run_status: &str,
        trace: &ConversationTurnTrace,
        trace_created_at: i64,
        completed_at: i64,
    ) -> Result<(), String> {
        self.finalize_chat_message_with_conversation_trace_and_usage(
            conversation_id,
            message_id,
            content,
            message_status,
            run_status,
            trace,
            trace_created_at,
            completed_at,
            None,
        )
    }

    /// Atomically commits every durable fact that makes an agent run terminal.
    ///
    /// A terminal assistant message, its trace, and its usage row form one visibility boundary.
    /// Keeping the optional usage write in this transaction prevents a retryable pending action
    /// from being exposed after its assistant message has already become terminal.
    #[allow(clippy::too_many_arguments)]
    pub fn finalize_chat_message_with_conversation_trace_and_usage(
        &self,
        conversation_id: &str,
        message_id: &str,
        content: &str,
        message_status: Option<&str>,
        run_status: &str,
        trace: &ConversationTurnTrace,
        trace_created_at: i64,
        completed_at: i64,
        usage: Option<&AgentUsageRecordInsert>,
    ) -> Result<(), String> {
        let mut connection = self.state.connection()?;
        let transaction = connection.transaction().map_err(storage_error)?;
        chat_repository::update_message_status_and_content(
            &transaction,
            conversation_id,
            message_id,
            content,
            message_status,
            completed_at,
        )
        .map_err(storage_error)?;
        chat_repository::update_message_run_terminal_state(
            &transaction,
            conversation_id,
            message_id,
            message_status,
            run_status,
            completed_at,
        )
        .map_err(storage_error)?;
        conversation_trace_repository::commit_trace_in_connection(
            &transaction,
            trace,
            trace_created_at,
            completed_at,
        )
        .map_err(storage_error)?;
        if let Some(usage) = usage {
            usage_repository::upsert_usage_record(&transaction, usage).map_err(storage_error)?;
        }
        transaction.commit().map_err(storage_error)
    }

    pub fn get_conversation_turn_trace(
        &self,
        assistant_message_id: &str,
    ) -> Result<Option<ConversationTurnTrace>, String> {
        let connection = self.state.connection()?;
        conversation_trace_repository::get_trace_for_message(&connection, assistant_message_id)
            .map_err(storage_error)
    }

    pub fn list_conversation_turn_traces(
        &self,
        conversation_id: &str,
    ) -> Result<Vec<ConversationTurnTrace>, String> {
        let connection = self.state.connection()?;
        conversation_trace_repository::list_traces_for_conversation(&connection, conversation_id)
            .map_err(storage_error)
    }

    pub fn get_active_context_compaction_summary(
        &self,
        conversation_id: &str,
    ) -> Result<Option<ContextCompactionSummary>, String> {
        let connection = self.state.connection()?;
        context_compaction_repository::get_active_summary(&connection, conversation_id)
            .map_err(|error| error.to_string())
    }

    pub fn save_model_request_observation(
        &self,
        observation: &ModelRequestObservation,
    ) -> Result<(), String> {
        let connection = self.state.connection()?;
        model_request_observation_repository::insert_observation(&connection, observation)
            .map_err(|error| error.to_string())
    }

    pub fn record_context_compaction_receipt(
        &self,
        receipt: &ContextCompactionReceipt,
        observation: Option<&ModelRequestObservation>,
    ) -> Result<(), String> {
        let mut connection = self.state.connection()?;
        context_compaction_receipt_repository::record_receipt(&mut connection, receipt, observation)
            .map_err(|error| error.to_string())
    }

    pub fn get_context_compaction_audit(
        &self,
        conversation_id: &str,
        operation_id: Option<&str>,
        limit: usize,
    ) -> Result<ContextCompactionAuditBundle, String> {
        let connection = self.state.connection()?;
        context_compaction_audit_repository::get_audit_bundle(
            &connection,
            conversation_id,
            operation_id,
            limit,
        )
        .map_err(|error| error.to_string())
    }

    pub fn prepare_context_compaction_prefix(
        &self,
        conversation_id: &str,
        covered_through: &ContextJournalCursor,
    ) -> Result<ContextCompactionPrefix, String> {
        let connection = self.state.connection()?;
        context_compaction_repository::prepare_prefix(&connection, conversation_id, covered_through)
            .map_err(|error| error.to_string())
    }

    /// Returns `None` when the planner's active-summary identity is stale. The caller should
    /// refresh its durable baseline and plan again instead of treating this as a failed run.
    pub fn prepare_context_compaction_prefix_if_current(
        &self,
        conversation_id: &str,
        covered_through: &ContextJournalCursor,
        expected_active_summary_id: Option<&str>,
    ) -> Result<Option<ContextCompactionPrefix>, String> {
        let connection = self.state.connection()?;
        let active =
            match context_compaction_repository::get_active_summary(&connection, conversation_id) {
                Ok(active) => active,
                Err(error) if error.is_stale() => return Ok(None),
                Err(error) => return Err(error.to_string()),
            };
        if active.as_ref().map(|summary| summary.id.as_str()) != expected_active_summary_id {
            return Ok(None);
        }
        match context_compaction_repository::prepare_prefix(
            &connection,
            conversation_id,
            covered_through,
        ) {
            Ok(prefix) => Ok(Some(prefix)),
            Err(error) if error.is_stale() => Ok(None),
            Err(error) => Err(error.to_string()),
        }
    }

    pub fn commit_context_compaction_prefix(
        &self,
        expected_prefix: &ContextCompactionPrefix,
        draft: ContextCompactionSummaryDraft,
        introduced_by_assistant_message_id: &str,
    ) -> Result<ContextCompactionSummary, String> {
        let mut connection = self.state.connection()?;
        context_compaction_repository::commit_prefix_replacement(
            &mut connection,
            expected_prefix,
            draft,
            introduced_by_assistant_message_id,
        )
        .map_err(|error| error.to_string())
    }

    /// Commits only if the prepared durable prefix is still current. `None` is a normal stale
    /// outcome and must cause a baseline refresh plus replanning.
    pub fn commit_context_compaction_prefix_if_current(
        &self,
        expected_prefix: &ContextCompactionPrefix,
        draft: ContextCompactionSummaryDraft,
        introduced_by_assistant_message_id: &str,
    ) -> Result<Option<ContextCompactionSummary>, String> {
        let mut connection = self.state.connection()?;
        match context_compaction_repository::commit_prefix_replacement(
            &mut connection,
            expected_prefix,
            draft,
            introduced_by_assistant_message_id,
        ) {
            Ok(summary) => Ok(Some(summary)),
            Err(error) if error.is_stale() => Ok(None),
            Err(error) => Err(error.to_string()),
        }
    }

    /// Atomically commits a successful compaction and its diagnostic evidence. `None` means the
    /// prepared prefix became stale; the transaction leaves no observation, summary, head or
    /// terminal receipt behind in that case.
    pub fn commit_context_compaction_prefix_with_receipt_if_current(
        &self,
        expected_prefix: &ContextCompactionPrefix,
        draft: ContextCompactionSummaryDraft,
        receipt: &ContextCompactionReceipt,
        observation: &ModelRequestObservation,
    ) -> Result<Option<ContextCompactionSummary>, String> {
        let mut connection = self.state.connection()?;
        match context_compaction_repository::commit_prefix_replacement_with_receipt(
            &mut connection,
            expected_prefix,
            draft,
            receipt,
            observation,
        ) {
            Ok(summary) => Ok(Some(summary)),
            Err(error) if error.is_stale() => Ok(None),
            Err(error) => Err(error.to_string()),
        }
    }

    pub fn rollback_context_compaction_summary(
        &self,
        conversation_id: &str,
        expected_summary_id: &str,
        updated_at: i64,
    ) -> Result<Option<ContextCompactionSummary>, String> {
        let mut connection = self.state.connection()?;
        context_compaction_repository::rollback_active_summary(
            &mut connection,
            conversation_id,
            expected_summary_id,
            updated_at,
        )
        .map_err(|error| error.to_string())
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
        composer_draft_repository::list_composer_drafts(&connection)
            .map(|drafts| {
                drafts
                    .into_iter()
                    .map(ComposerDraftRecord::normalize_permission_mode)
                    .collect()
            })
            .map_err(storage_error)
    }

    pub fn save_composer_draft(
        &self,
        draft: ComposerDraftRecord,
    ) -> Result<ComposerDraftRecord, String> {
        let draft = draft.normalize_permission_mode();
        let connection = self.state.connection()?;
        ensure_project_reference_exists(&connection, draft.project_id.as_deref())?;
        composer_draft_repository::save_composer_draft(&connection, draft.clone())
            .map_err(storage_error)?;
        Ok(draft)
    }

    /// Loads effective enablement for a batch of complete, opaque Skill ids.
    ///
    /// An absent override is intentionally enabled by default. The returned
    /// map contains one entry for every distinct requested id.
    pub fn load_skill_enablement(
        &self,
        skill_ids: &[String],
    ) -> Result<BTreeMap<String, bool>, String> {
        for skill_id in skill_ids {
            validate_skill_enablement_id(skill_id)?;
        }
        let mut enablement = skill_ids
            .iter()
            .cloned()
            .map(|skill_id| (skill_id, true))
            .collect::<BTreeMap<_, _>>();
        if enablement.is_empty() {
            return Ok(enablement);
        }

        let mut connection = self.state.connection()?;
        let overrides = skill_enablement_repository::load_skill_enablement_overrides(
            &mut connection,
            skill_ids,
        )
        .map_err(storage_error)?;
        enablement.extend(overrides);
        Ok(enablement)
    }

    /// Loads effective enablement together with its monotonic mutation
    /// generation for compare-and-swap state tokens.
    pub fn load_skill_enablement_states(
        &self,
        skill_ids: &[String],
    ) -> Result<BTreeMap<String, skill_enablement_repository::SkillEnablementState>, String> {
        for skill_id in skill_ids {
            validate_skill_enablement_id(skill_id)?;
        }
        let mut connection = self.state.connection()?;
        skill_enablement_repository::load_skill_enablement_states(&mut connection, skill_ids)
            .map_err(storage_error)
    }

    /// Stores an explicit enablement override and reports whether state changed.
    pub fn set_skill_enablement_override(
        &self,
        skill_id: &str,
        enabled: bool,
    ) -> Result<bool, String> {
        validate_skill_enablement_id(skill_id)?;
        let connection = self.state.connection()?;
        skill_enablement_repository::set_skill_enablement_override(&connection, skill_id, enabled)
            .map_err(storage_error)
    }

    /// Atomically mutates effective enablement if it still matches the state
    /// observed by the caller. An absent row participates as the product
    /// default (`enabled = true`).
    pub fn compare_and_set_skill_enablement(
        &self,
        skill_id: &str,
        expected_enabled: bool,
        expected_generation: u64,
        target: bool,
    ) -> Result<skill_enablement_repository::SkillEnablementCompareAndSetOutcome, String> {
        validate_skill_enablement_id(skill_id)?;
        let mut connection = self.state.connection()?;
        skill_enablement_repository::compare_and_set_skill_enablement(
            &mut connection,
            skill_id,
            expected_enabled,
            expected_generation,
            target,
        )
        .map_err(storage_error)
    }

    /// Removes an explicit override, restoring the default enabled state.
    pub fn delete_skill_enablement_override(&self, skill_id: &str) -> Result<bool, String> {
        validate_skill_enablement_id(skill_id)?;
        let connection = self.state.connection()?;
        skill_enablement_repository::delete_skill_enablement_override(&connection, skill_id)
            .map_err(storage_error)
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

    pub fn store_pending_agent_action(
        &self,
        record: AgentPendingActionRecord,
    ) -> Result<pending_action_repository::PendingActionStoreOutcome, String> {
        let connection = self.state.connection()?;
        let outcome = pending_action_repository::store_pending_action(&connection, &record)
            .map_err(storage_error)?;
        match outcome {
            pending_action_repository::PendingActionStoreOutcome::Conflict {
                ref existing_run_id,
                ref existing_status,
            } => Err(format!(
                "待审批操作 actionId={} 已属于 runId={}（status={}）；拒绝覆盖冻结快照。",
                record.action_id, existing_run_id, existing_status
            )),
            _ => Ok(outcome),
        }
    }

    pub fn list_pending_agent_actions(&self) -> Result<Vec<AgentPendingActionRecord>, String> {
        let connection = self.state.connection()?;
        pending_action_repository::list_pending_actions(&connection).map_err(storage_error)
    }

    pub fn reconcile_interrupted_pending_agent_actions(
        &self,
        updated_at: i64,
    ) -> Result<Vec<AgentPendingActionRecord>, String> {
        const INTERRUPTION_REASON: &str =
            "The application exited after approval; command outcome is unknown and was not replayed.";
        let mut connection = self.state.connection()?;
        let transaction = connection.transaction().map_err(storage_error)?;
        let interrupted = pending_action_repository::list_interrupted_actions(&transaction)
            .map_err(storage_error)?;
        let pending_successors =
            pending_action_repository::list_pending_actions(&transaction).map_err(storage_error)?;
        let mut retired_successors = HashSet::new();
        let mut claimed_successors = HashSet::new();
        for record in &interrupted {
            let durable_run_status = match (
                record.conversation_id.as_deref(),
                record.assistant_message_id.as_deref(),
            ) {
                (Some(conversation_id), Some(message_id)) => {
                    let raw = transaction
                        .query_row(
                            "SELECT agent_run_json FROM messages
                         WHERE conversation_id = ?1 AND id = ?2",
                            rusqlite::params![conversation_id, message_id],
                            |row| row.get::<_, Option<String>>(0),
                        )
                        .optional()
                        .map_err(storage_error)?
                        .flatten();
                    raw.map(|raw| {
                        serde_json::from_str::<serde_json::Value>(&raw)
                            .map_err(|error| {
                                format!(
                                    "无法解析中断操作 {} 的 agent run 状态：{error}",
                                    record.action_id
                                )
                            })
                            .map(|run| {
                                run.get("status")
                                    .and_then(serde_json::Value::as_str)
                                    .map(ToString::to_string)
                            })
                    })
                    .transpose()?
                    .flatten()
                }
                _ => None,
            };
            let reconciled_status = match record.target_status.as_deref() {
                Some(status @ ("completed" | "failed" | "rejected" | "cancelled")) => status,
                None => {
                    let affected = pending_action_repository::set_pending_action_target_status(
                        &transaction,
                        &record.action_id,
                        &record.status,
                        "failed",
                        updated_at,
                    )
                    .map_err(storage_error)?;
                    if affected != 1 {
                        return Err(format!(
                            "启动对账无法为旧待审批操作 {} 写入 failed 目标终态。",
                            record.action_id
                        ));
                    }
                    "failed"
                }
                Some(status) => {
                    return Err(format!(
                        "启动对账发现待审批操作 {} 的目标终态无效：{status}",
                        record.action_id
                    ));
                }
            };
            let affected = pending_action_repository::transition_pending_action(
                &transaction,
                &record.action_id,
                &record.status,
                reconciled_status,
                "{}",
                updated_at,
            )
            .map_err(storage_error)?;
            if affected != 1 {
                return Err(format!(
                    "启动对账无法以 CAS 迁移待审批操作 {}（expectedStatus={}）。",
                    record.action_id, record.status
                ));
            }
            let successor_candidates = pending_successors
                .iter()
                .filter(|candidate| {
                    !retired_successors.contains(&candidate.action_id)
                        && is_pending_successor_candidate(record, candidate)
                })
                .collect::<Vec<_>>();
            let valid_successors = successor_candidates
                .iter()
                .copied()
                .filter(|candidate| is_valid_pending_successor(record, candidate))
                .collect::<Vec<_>>();
            if valid_successors.len() > 1 {
                return Err(format!(
                    "启动对账发现 action {} 存在多个合法待审批后继。",
                    record.action_id
                ));
            }
            for candidate in successor_candidates
                .iter()
                .copied()
                .filter(|candidate| !is_valid_pending_successor(record, candidate))
            {
                let target_affected = pending_action_repository::set_pending_action_target_status(
                    &transaction,
                    &candidate.action_id,
                    "pending",
                    "cancelled",
                    updated_at,
                )
                .map_err(storage_error)?;
                if target_affected != 1 {
                    return Err(format!(
                        "启动对账无法取消无效待审批后继 {}。",
                        candidate.action_id
                    ));
                }
                let transition_affected = pending_action_repository::transition_pending_action(
                    &transaction,
                    &candidate.action_id,
                    "pending",
                    "cancelled",
                    "{}",
                    updated_at,
                )
                .map_err(storage_error)?;
                if transition_affected != 1 {
                    return Err(format!(
                        "启动对账无法终结无效待审批后继 {}。",
                        candidate.action_id
                    ));
                }
                retired_successors.insert(candidate.action_id.clone());
            }
            let valid_successor = valid_successors.first().copied();
            if let Some(successor) = valid_successor {
                if !claimed_successors.insert(successor.action_id.clone()) {
                    return Err(format!(
                        "启动对账发现待审批后继 {} 被多个父操作声明。",
                        successor.action_id
                    ));
                }
                if matches!(
                    durable_run_status.as_deref(),
                    Some("completed" | "failed" | "cancelled")
                ) {
                    return Err(format!(
                        "启动对账发现 run {} 同时存在终态 assistant 与合法待审批后继。",
                        record.run_id
                    ));
                }
                if let (Some(conversation_id), Some(message_id)) = (
                    record.conversation_id.as_deref(),
                    record.assistant_message_id.as_deref(),
                ) {
                    chat_repository::update_message_run_waiting_state(
                        &transaction,
                        conversation_id,
                        message_id,
                        &record.run_id,
                        updated_at,
                    )
                    .map_err(storage_error)?;
                }
                transaction
                    .execute(
                        "UPDATE agent_usage_records
                         SET status = 'waiting_for_approval', error = NULL, completed_at = NULL
                         WHERE run_id = ?1
                           AND COALESCE(status, '') NOT IN ('completed', 'failed', 'cancelled')",
                        [&record.run_id],
                    )
                    .map_err(storage_error)?;
                continue;
            }
            if matches!(
                durable_run_status.as_deref(),
                Some("completed" | "failed" | "cancelled")
            ) {
                transaction
                    .execute(
                        "UPDATE agent_usage_records
                         SET status = ?2, completed_at = COALESCE(completed_at, ?3)
                         WHERE run_id = ?1
                           AND COALESCE(status, '') NOT IN ('completed', 'failed', 'cancelled')",
                        rusqlite::params![record.run_id, durable_run_status, updated_at],
                    )
                    .map_err(storage_error)?;
                continue;
            }
            if let (Some(conversation_id), Some(message_id)) = (
                record.conversation_id.as_deref(),
                record.assistant_message_id.as_deref(),
            ) {
                chat_repository::update_message_run_terminal_state(
                    &transaction,
                    conversation_id,
                    message_id,
                    Some("error"),
                    "failed",
                    updated_at,
                )
                .map_err(storage_error)?;
            }
            transaction
                .execute(
                    "UPDATE agent_usage_records
                     SET status = 'failed', error = ?2, completed_at = ?3
                     WHERE run_id = ?1
                       AND COALESCE(status, '') NOT IN ('completed', 'failed', 'cancelled')",
                    rusqlite::params![record.run_id, INTERRUPTION_REASON, updated_at],
                )
                .map_err(storage_error)?;
        }
        transaction.commit().map_err(storage_error)?;
        Ok(interrupted)
    }

    pub fn transition_pending_agent_action(
        &self,
        action_id: &str,
        expected_status: &str,
        status: &str,
        agent_input_json: &str,
        updated_at: i64,
    ) -> Result<(), String> {
        let connection = self.state.connection()?;
        let affected = pending_action_repository::transition_pending_action(
            &connection,
            action_id,
            expected_status,
            status,
            agent_input_json,
            updated_at,
        )
        .map_err(storage_error)?;
        if affected != 1 {
            return Err(format!(
                "待审批操作状态迁移必须且只能更新一条记录，actionId={action_id}，实际更新 {affected} 条。"
            ));
        }
        Ok(())
    }

    pub fn set_pending_agent_action_target_status(
        &self,
        action_id: &str,
        expected_status: &str,
        target_status: &str,
        updated_at: i64,
    ) -> Result<(), String> {
        let connection = self.state.connection()?;
        let affected = pending_action_repository::set_pending_action_target_status(
            &connection,
            action_id,
            expected_status,
            target_status,
            updated_at,
        )
        .map_err(storage_error)?;
        if affected != 1 {
            return Err(format!(
                "待审批操作目标终态写入必须且只能更新一条记录，actionId={action_id}，expectedStatus={expected_status}，实际更新 {affected} 条。"
            ));
        }
        Ok(())
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

fn validate_skill_enablement_id(skill_id: &str) -> Result<(), String> {
    if skill_id.is_empty() {
        return Err("Skill ID 不能为空。".to_string());
    }
    if skill_id.len() > MAX_SKILL_ENABLEMENT_ID_BYTES {
        return Err(format!(
            "Skill ID 不能超过 {MAX_SKILL_ENABLEMENT_ID_BYTES} 字节。"
        ));
    }
    Ok(())
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
                api_url_override: None,
                api_token_override: None,
                supports_image: false,
                context_window_tokens: Some(128_000),
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
        assert_eq!(stored.models[0].context_window_tokens, Some(128_000));
    }

    #[test]
    fn rejects_zero_context_window_without_overwriting_saved_settings() {
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
                api_url_override: None,
                api_token_override: None,
                supports_image: false,
                context_window_tokens: Some(128_000),
                input_price: "0.01".to_string(),
                output_price: "0.02".to_string(),
                enabled: true,
            }],
        };
        service.save_model_settings(valid.clone()).unwrap();

        let mut invalid = valid;
        invalid.models[0].context_window_tokens = Some(0);
        assert!(service.save_model_settings(invalid).is_err());

        let stored = service.load_model_settings().unwrap().unwrap();
        assert_eq!(stored.models[0].context_window_tokens, Some(128_000));
    }

    #[test]
    fn requires_model_connection_overrides_to_be_saved_as_a_complete_pair() {
        let fixture = StorageFixture::new();
        let service = fixture.service();
        let valid = ModelSettingsRecord {
            api_url: "https://global.example/v1".to_string(),
            api_token: "global-token".to_string(),
            search_mode: "disabled".to_string(),
            tavily_api_key: String::new(),
            models: vec![ModelConfigRecord {
                id: "model-a".to_string(),
                display_name: "Model A".to_string(),
                api_url_override: Some("https://model.example/v1".to_string()),
                api_token_override: Some("model-token".to_string()),
                supports_image: false,
                context_window_tokens: Some(128_000),
                input_price: "0.01".to_string(),
                output_price: "0.02".to_string(),
                enabled: true,
            }],
        };
        service.save_model_settings(valid.clone()).unwrap();

        let mut invalid = valid;
        invalid.models[0].api_token_override = None;
        assert!(service.save_model_settings(invalid).is_err());

        let stored = service.load_model_settings().unwrap().unwrap();
        assert_eq!(
            stored.models[0].api_url_override.as_deref(),
            Some("https://model.example/v1")
        );
        assert_eq!(
            stored.models[0].api_token_override.as_deref(),
            Some("model-token")
        );
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

        let conversation = service
            .load_conversation("conversation-1")
            .unwrap()
            .unwrap();
        let attachment = &conversation.messages[0].attachments[0];

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
    fn forked_conversation_owns_independent_attachment_files_and_is_idempotent() {
        let fixture = StorageFixture::new();
        let service = fixture.service();
        let mut source = conversation("conversation-source", Some("project-1"), "message-user");
        source.messages.push(ChatMessageRecord {
            id: "message-assistant".to_string(),
            role: "assistant".to_string(),
            content: "done".to_string(),
            created_at: 2,
            status: Some("sent".to_string()),
            attachments: Vec::new(),
            agent_run_json: None,
            ui_state_json: None,
        });
        source.updated_at = 2;
        service.save_conversation(source).unwrap();
        service
            .save_input_attachments(
                "conversation-source",
                "message-user",
                Some("project-1"),
                &[input_attachment(
                    "attachment-source",
                    AgentInputAttachmentKind::File,
                    "notes.txt",
                    Some("text/plain"),
                    b"independent fork attachment",
                )],
                1,
            )
            .unwrap();

        let input = ForkConversationInput {
            request_id: "fork-request-1".to_string(),
            source_conversation_id: "conversation-source".to_string(),
            through_assistant_message_id: "message-assistant".to_string(),
        };
        let forked = service.fork_conversation(input.clone()).unwrap();
        assert_eq!(forked.project_id.as_deref(), Some("project-1"));
        assert_eq!(forked.messages.len(), 2);
        let forked_attachment = forked.messages[0].attachments.as_slice();
        assert_eq!(forked_attachment.len(), 1);
        assert_ne!(forked_attachment[0].id, "attachment-source");

        let retry = service.fork_conversation(input).unwrap();
        assert_eq!(retry.id, forked.id);
        assert_eq!(service.load_conversations().unwrap().len(), 2);

        service.delete_conversation("conversation-source").unwrap();
        let retained = service.load_conversation(&forked.id).unwrap().unwrap();
        let retained_attachment_id = retained.messages[0].attachments[0].id.clone();
        let retained_payload = service
            .load_input_attachments(&[retained_attachment_id])
            .unwrap();
        assert_eq!(retained_payload.len(), 1);
        assert_eq!(
            base64::engine::general_purpose::STANDARD
                .decode(&retained_payload[0].data)
                .unwrap(),
            b"independent fork attachment"
        );

        let retained_assistant_id = retained
            .messages
            .iter()
            .find(|message| message.role == "assistant")
            .unwrap()
            .id
            .clone();
        let recursive = service
            .fork_conversation(ForkConversationInput {
                request_id: "fork-request-2".to_string(),
                source_conversation_id: retained.id.clone(),
                through_assistant_message_id: retained_assistant_id,
            })
            .unwrap();
        service.delete_conversation(&retained.id).unwrap();

        let recursive = service.load_conversation(&recursive.id).unwrap().unwrap();
        let recursive_attachment_id = recursive.messages[0].attachments[0].id.clone();
        let recursive_payload = service
            .load_input_attachments(&[recursive_attachment_id])
            .unwrap();
        assert_eq!(
            base64::engine::general_purpose::STANDARD
                .decode(&recursive_payload[0].data)
                .unwrap(),
            b"independent fork attachment"
        );
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
    fn composer_drafts_only_preserve_full_for_current_permission_semantics() {
        let fixture = StorageFixture::new();
        let service = fixture.service();

        let mut legacy = composer_draft("legacy", None, "legacy full");
        legacy.permission_mode = "full".to_string();
        legacy.permission_mode_version = 0;
        let legacy = service.save_composer_draft(legacy).unwrap();
        assert_eq!(legacy.permission_mode, "default");

        let mut current = composer_draft("current", None, "current full");
        current.permission_mode = "full".to_string();
        current.permission_mode_version =
            crate::storage::models::CURRENT_COMPOSER_PERMISSION_MODE_VERSION;
        let current = service.save_composer_draft(current).unwrap();
        assert_eq!(current.permission_mode, "full");

        let stored = service.load_composer_drafts().unwrap();
        assert_eq!(
            stored
                .iter()
                .find(|draft| draft.scope_id == "legacy")
                .unwrap()
                .permission_mode,
            "default"
        );
        assert_eq!(
            stored
                .iter()
                .find(|draft| draft.scope_id == "current")
                .unwrap()
                .permission_mode,
            "full"
        );
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
            .store_pending_agent_action(pending_action("action-1", "conversation-1"))
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

    #[test]
    fn startup_reconciliation_marks_interrupted_action_message_and_usage_failed() {
        let fixture = StorageFixture::new();
        let service = fixture.service();
        let mut interrupted_conversation = conversation(
            "conversation-interrupted",
            Some("project-1"),
            "assistant-interrupted",
        );
        let message = &mut interrupted_conversation.messages[0];
        message.role = "assistant".to_string();
        message.status = Some("pending".to_string());
        message.agent_run_json = Some(
            serde_json::json!({
                "status": "waiting_for_approval",
                "state": {
                    "status": "waiting_for_approval",
                    "activeRunId": "run-1",
                    "updatedAt": 1
                }
            })
            .to_string(),
        );
        service.save_conversation(interrupted_conversation).unwrap();
        let mut usage = agent_usage_record("conversation-interrupted", "assistant-interrupted");
        usage.status = Some("running".to_string());
        usage.completed_at = None;
        service.upsert_agent_usage(usage).unwrap();
        let mut pending = pending_action("action-interrupted", "conversation-interrupted");
        pending.assistant_message_id = Some("assistant-interrupted".to_string());
        pending.status = "approved".to_string();
        pending.agent_input_json = "sensitive continuation".to_string();
        service.store_pending_agent_action(pending).unwrap();

        let reconciled = service
            .reconcile_interrupted_pending_agent_actions(42)
            .unwrap();
        assert_eq!(reconciled.len(), 1);
        assert_eq!(reconciled[0].status, "approved");
        assert!(service.list_pending_agent_actions().unwrap().is_empty());

        let conversation = service
            .load_conversation("conversation-interrupted")
            .unwrap()
            .unwrap();
        let message = &conversation.messages[0];
        assert_eq!(message.status.as_deref(), Some("error"));
        let run: serde_json::Value =
            serde_json::from_str(message.agent_run_json.as_deref().unwrap()).unwrap();
        assert_eq!(run["status"], "failed");
        assert_eq!(run["state"]["status"], "failed");
        assert!(run["state"]["activeRunId"].is_null());

        let connection = service.state.connection().unwrap();
        let pending_row: (String, String) = connection
            .query_row(
                "SELECT status, agent_input_json FROM agent_pending_actions WHERE action_id = ?1",
                ["action-interrupted"],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(pending_row, ("failed".to_string(), "{}".to_string()));
        let usage_row: (String, Option<String>, Option<i64>) = connection
            .query_row(
                "SELECT status, error, completed_at FROM agent_usage_records WHERE run_id = ?1",
                ["run-1"],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(usage_row.0, "failed");
        assert!(usage_row.1.unwrap().contains("outcome is unknown"));
        assert_eq!(usage_row.2, Some(42));
    }

    #[test]
    fn startup_reconciliation_preserves_a_durable_terminal_assistant_commit() {
        let fixture = StorageFixture::new();
        let service = fixture.service();
        let mut conversation = conversation(
            "conversation-terminal",
            Some("project-1"),
            "assistant-terminal",
        );
        let message = &mut conversation.messages[0];
        message.role = "assistant".to_string();
        message.status = Some("sent".to_string());
        message.agent_run_json = Some(
            serde_json::json!({
                "status": "completed",
                "completedAt": 40,
                "state": { "status": "completed", "activeRunId": null, "updatedAt": 40 }
            })
            .to_string(),
        );
        service.save_conversation(conversation).unwrap();
        let mut usage = agent_usage_record("conversation-terminal", "assistant-terminal");
        usage.status = Some("running".to_string());
        usage.completed_at = None;
        service.upsert_agent_usage(usage).unwrap();
        let mut pending = pending_action("action-terminal", "conversation-terminal");
        pending.assistant_message_id = Some("assistant-terminal".to_string());
        pending.status = "approved".to_string();
        pending.target_status = Some("completed".to_string());
        service.store_pending_agent_action(pending).unwrap();

        service
            .reconcile_interrupted_pending_agent_actions(42)
            .unwrap();

        let connection = service.state.connection().unwrap();
        let pending_status: String = connection
            .query_row(
                "SELECT status FROM agent_pending_actions WHERE action_id = 'action-terminal'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(pending_status, "completed");
        let usage_status: (String, Option<i64>) = connection
            .query_row(
                "SELECT status, completed_at FROM agent_usage_records WHERE run_id = 'run-1'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(usage_status, ("completed".to_string(), Some(42)));
        drop(connection);
        let conversation = service
            .load_conversation("conversation-terminal")
            .unwrap()
            .unwrap();
        assert_eq!(conversation.messages[0].status.as_deref(), Some("sent"));
    }

    #[test]
    fn startup_reconciliation_preserves_a_valid_nested_pending_checkpoint() {
        let fixture = StorageFixture::new();
        let service = fixture.service();
        let mut conversation =
            conversation("conversation-nested", Some("project-1"), "assistant-nested");
        let message = &mut conversation.messages[0];
        message.role = "assistant".to_string();
        message.status = Some("pending".to_string());
        message.agent_run_json = Some(
            serde_json::json!({
                "runId": "run-1",
                "status": "running",
                "state": { "status": "running", "activeRunId": "run-1", "updatedAt": 1 }
            })
            .to_string(),
        );
        service.save_conversation(conversation).unwrap();
        let mut usage = agent_usage_record("conversation-nested", "assistant-nested");
        usage.status = Some("running".to_string());
        usage.completed_at = None;
        service.upsert_agent_usage(usage).unwrap();

        let mut parent = pending_action("parent-action", "conversation-nested");
        parent.assistant_message_id = Some("assistant-nested".to_string());
        parent.status = "executing".to_string();
        parent.target_status = Some("completed".to_string());
        service.store_pending_agent_action(parent).unwrap();

        let mut child = pending_action("child-storage-id", "conversation-nested");
        child.assistant_message_id = Some("assistant-nested".to_string());
        child.action_type = "tool_call".to_string();
        child.tool_name = "approval_tool".to_string();
        child.tool_call_id = Some("child-call".to_string());
        child.action_json = serde_json::json!({
            "type": "tool_call",
            "call": {
                "id": "child-call",
                "tool": "approval_tool",
                "args": {},
                "approvalStatus": "required"
            }
        })
        .to_string();
        child.agent_input_json = serde_json::json!({
            "apiUrl": "https://example.test/v1/chat/completions",
            "apiToken": "",
            "model": "test-model",
            "messages": [],
            "resumeCheckpoint": {
                "version": 2,
                "runId": "run-1",
                "contextItems": [],
                "nextModelRequestIndex": 1,
                "queuedToolCalls": [],
                "suppressedNarration": false,
                "extensionSnapshots": [],
                "pendingToolCallId": "child-call",
                "conversationTraceItems": [
                    {
                        "type": "tool_result",
                        "sequence": 1,
                        "callId": "parent-action",
                        "tool": "run_command",
                        "status": "succeeded",
                        "success": true,
                        "observation": {},
                        "approvalStatus": "approved",
                        "truncated": false
                    },
                    {
                        "type": "tool_call",
                        "sequence": 2,
                        "callId": "child-call",
                        "tool": "approval_tool",
                        "operation": {},
                        "approvalStatus": "required",
                        "truncated": false
                    }
                ],
                "nextConversationTraceSequence": 3,
                "conversationTraceTruncated": false,
                "modelVisibleTraceItemCount": 0
            }
        })
        .to_string();
        service.store_pending_agent_action(child).unwrap();

        service
            .reconcile_interrupted_pending_agent_actions(42)
            .unwrap();

        let pending = service.list_pending_agent_actions().unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].action_id, "child-storage-id");
        let connection = service.state.connection().unwrap();
        let parent_status: String = connection
            .query_row(
                "SELECT status FROM agent_pending_actions WHERE action_id = 'parent-action'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(parent_status, "completed");
        let usage_state: (String, Option<String>, Option<i64>) = connection
            .query_row(
                "SELECT status, error, completed_at FROM agent_usage_records WHERE run_id = 'run-1'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(
            usage_state,
            ("waiting_for_approval".to_string(), None, None)
        );
        drop(connection);
        let conversation = service
            .load_conversation("conversation-nested")
            .unwrap()
            .unwrap();
        let message = &conversation.messages[0];
        assert_eq!(message.status.as_deref(), Some("pending"));
        let run: serde_json::Value =
            serde_json::from_str(message.agent_run_json.as_deref().unwrap()).unwrap();
        assert_eq!(run["status"], "waiting_for_approval");
        assert_eq!(run["state"]["status"], "waiting_for_approval");
        assert_eq!(run["state"]["activeRunId"], "run-1");
    }

    #[test]
    fn startup_reconciliation_retires_an_invalid_nested_pending_action() {
        let fixture = StorageFixture::new();
        let service = fixture.service();
        let mut conversation = conversation(
            "conversation-invalid-nested",
            Some("project-1"),
            "assistant-invalid-nested",
        );
        let message = &mut conversation.messages[0];
        message.role = "assistant".to_string();
        message.status = Some("pending".to_string());
        message.agent_run_json = Some(
            serde_json::json!({
                "runId": "run-1",
                "status": "waiting_for_approval",
                "state": {
                    "status": "waiting_for_approval",
                    "activeRunId": "run-1",
                    "updatedAt": 1
                }
            })
            .to_string(),
        );
        service.save_conversation(conversation).unwrap();
        let mut usage =
            agent_usage_record("conversation-invalid-nested", "assistant-invalid-nested");
        usage.status = Some("waiting_for_approval".to_string());
        usage.completed_at = None;
        service.upsert_agent_usage(usage).unwrap();

        let mut parent = pending_action("invalid-parent", "conversation-invalid-nested");
        parent.assistant_message_id = Some("assistant-invalid-nested".to_string());
        parent.status = "executing".to_string();
        service.store_pending_agent_action(parent).unwrap();

        let mut child = pending_action("invalid-child", "conversation-invalid-nested");
        child.assistant_message_id = Some("assistant-invalid-nested".to_string());
        child.action_type = "tool_call".to_string();
        child.tool_name = "approval_tool".to_string();
        child.tool_call_id = Some("invalid-child-call".to_string());
        child.action_json = serde_json::json!({
            "type": "tool_call",
            "call": {
                "id": "invalid-child-call",
                "tool": "approval_tool",
                "args": {},
                "approvalStatus": "required"
            }
        })
        .to_string();
        // A pending child without a run checkpoint is not a durable handoff and must never remain
        // approvable after its interrupted parent has been failed.
        child.agent_input_json = serde_json::json!({
            "apiUrl": "https://example.test/v1/chat/completions",
            "apiToken": "",
            "model": "test-model",
            "messages": []
        })
        .to_string();
        service.store_pending_agent_action(child).unwrap();

        service
            .reconcile_interrupted_pending_agent_actions(42)
            .unwrap();

        assert!(service.list_pending_agent_actions().unwrap().is_empty());
        let connection = service.state.connection().unwrap();
        let statuses = connection
            .prepare(
                "SELECT action_id, status, target_status
                 FROM agent_pending_actions
                 WHERE action_id IN ('invalid-parent', 'invalid-child')
                 ORDER BY action_id",
            )
            .unwrap()
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<String>>(2)?,
                ))
            })
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap();
        assert_eq!(
            statuses,
            vec![
                (
                    "invalid-child".to_string(),
                    "cancelled".to_string(),
                    Some("cancelled".to_string())
                ),
                (
                    "invalid-parent".to_string(),
                    "failed".to_string(),
                    Some("failed".to_string())
                )
            ]
        );
        drop(connection);
        let conversation = service
            .load_conversation("conversation-invalid-nested")
            .unwrap()
            .unwrap();
        assert_eq!(conversation.messages[0].status.as_deref(), Some("error"));
    }

    #[test]
    fn startup_reconciliation_keeps_failed_action_outcome_when_assistant_commit_completed() {
        let fixture = StorageFixture::new();
        let service = fixture.service();
        let mut conversation = conversation(
            "conversation-failed-action",
            Some("project-1"),
            "assistant-failed-action",
        );
        let message = &mut conversation.messages[0];
        message.role = "assistant".to_string();
        message.status = Some("sent".to_string());
        message.agent_run_json = Some(
            serde_json::json!({
                "status": "completed",
                "completedAt": 40,
                "state": { "status": "completed", "activeRunId": null, "updatedAt": 40 }
            })
            .to_string(),
        );
        service.save_conversation(conversation).unwrap();
        let mut usage = agent_usage_record("conversation-failed-action", "assistant-failed-action");
        usage.status = Some("completed".to_string());
        usage.completed_at = Some(40);
        service.upsert_agent_usage(usage).unwrap();
        let mut pending = pending_action("action-failed", "conversation-failed-action");
        pending.assistant_message_id = Some("assistant-failed-action".to_string());
        pending.status = "approved".to_string();
        pending.target_status = Some("failed".to_string());
        service.store_pending_agent_action(pending).unwrap();

        service
            .reconcile_interrupted_pending_agent_actions(42)
            .unwrap();

        let connection = service.state.connection().unwrap();
        let pending_status: String = connection
            .query_row(
                "SELECT status FROM agent_pending_actions WHERE action_id = 'action-failed'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(pending_status, "failed");
        let usage_status: String = connection
            .query_row(
                "SELECT status FROM agent_usage_records WHERE run_id = 'run-1'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(usage_status, "completed");
        drop(connection);
        let conversation = service
            .load_conversation("conversation-failed-action")
            .unwrap()
            .unwrap();
        assert_eq!(conversation.messages[0].status.as_deref(), Some("sent"));
    }

    #[test]
    fn skill_enablement_defaults_to_true_and_persists_explicit_overrides() {
        let fixture = StorageFixture::new();
        let first = "bundled:application:repository-evidence-auditor".to_string();
        let second = "installed:user:01234567-89ab-4def-8123-456789abcdef".to_string();

        {
            let service = fixture.service();
            let initial = service
                .load_skill_enablement(&[first.clone(), second.clone(), first.clone()])
                .unwrap();
            assert_eq!(initial.len(), 2);
            assert_eq!(initial.get(&first), Some(&true));
            assert_eq!(initial.get(&second), Some(&true));

            assert!(service
                .set_skill_enablement_override(&first, false)
                .unwrap());
            assert!(!service
                .set_skill_enablement_override(&first, false)
                .unwrap());
            assert!(service
                .set_skill_enablement_override(&second, true)
                .unwrap());
        }

        let reopened = fixture.service();
        let persisted = reopened
            .load_skill_enablement(&[second.clone(), first.clone()])
            .unwrap();
        assert_eq!(persisted.get(&first), Some(&false));
        assert_eq!(persisted.get(&second), Some(&true));

        assert!(reopened.delete_skill_enablement_override(&first).unwrap());
        assert!(!reopened.delete_skill_enablement_override(&first).unwrap());
        assert_eq!(
            reopened
                .load_skill_enablement(std::slice::from_ref(&first))
                .unwrap()
                .get(&first),
            Some(&true)
        );
    }

    #[test]
    fn skill_enablement_rejects_empty_and_oversized_ids_before_storage() {
        let fixture = StorageFixture::new();
        let service = fixture.service();
        let oversized = "x".repeat(MAX_SKILL_ENABLEMENT_ID_BYTES + 1);

        assert!(service.set_skill_enablement_override("", false).is_err());
        assert!(service.delete_skill_enablement_override("").is_err());
        assert!(service
            .load_skill_enablement(&["valid:id".to_string(), String::new()])
            .is_err());
        assert!(service
            .set_skill_enablement_override(&oversized, false)
            .is_err());
        assert!(service
            .delete_skill_enablement_override(&oversized)
            .is_err());

        let maximum = "x".repeat(MAX_SKILL_ENABLEMENT_ID_BYTES);
        assert!(service
            .set_skill_enablement_override(&maximum, false)
            .unwrap());
        assert_eq!(
            service
                .load_skill_enablement(std::slice::from_ref(&maximum))
                .unwrap()
                .get(&maximum),
            Some(&false)
        );
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
            permission_mode_version: 0,
            model_id: Some("model-1".to_string()),
            project_id: project_id.map(ToString::to_string),
            attachments_json: "[]".to_string(),
            skills_json: "[]".to_string(),
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
            target_status: None,
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
