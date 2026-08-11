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
        attach_message_guidance_timelines(&connection, &mut conversations)?;
        Ok(conversations)
    }

    /// Loads complete conversations for the renderer and decorates only forked tasks with their
    /// backend-owned continuation boundary. Internal Agent callers continue to use the plain
    /// conversation record and never consume presentation lineage.
    pub fn load_conversation_views(&self) -> Result<Vec<ChatConversationViewRecord>, String> {
        let conversations = self.load_conversations()?;
        let connection = self.state.connection()?;
        conversations
            .into_iter()
            .map(|conversation| {
                let continuation_origin = conversation_fork_repository::get_continuation_origin(
                    &connection,
                    &conversation.id,
                )
                .map_err(storage_error)?;
                Ok(ChatConversationViewRecord {
                    conversation,
                    continuation_origin,
                })
            })
            .collect()
    }

    pub fn load_conversation_metas(&self) -> Result<Vec<ChatConversationMetaRecord>, String> {
        let connection = self.state.connection()?;
        chat_repository::list_conversation_metas(&connection).map_err(storage_error)
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
            attach_message_guidance_timelines(&connection, std::slice::from_mut(conversation))?;
        }
        Ok(conversation)
    }

    pub fn load_conversation_view(
        &self,
        conversation_id: &str,
    ) -> Result<Option<ChatConversationViewRecord>, String> {
        let Some(conversation) = self.load_conversation(conversation_id)? else {
            return Ok(None);
        };
        let connection = self.state.connection()?;
        let continuation_origin =
            conversation_fork_repository::get_continuation_origin(&connection, conversation_id)
                .map_err(storage_error)?;
        Ok(Some(ChatConversationViewRecord {
            conversation,
            continuation_origin,
        }))
    }

    pub fn fork_conversation(
        &self,
        input: ForkConversationInput,
    ) -> Result<ChatConversationRecord, String> {
        self.fork_conversation_with_domain_error(input, None)
            .map_err(|error| error.to_string())
    }

    fn fork_conversation_with_domain_error(
        &self,
        input: ForkConversationInput,
        provider_continuation_vault: Option<&ProviderContinuationVault>,
    ) -> Result<ChatConversationRecord, conversation_fork_repository::ConversationForkError> {
        let point = ConversationForkPoint::AssistantReply {
            assistant_message_id: input.through_assistant_message_id,
        };
        self.fork_conversation_at_point_with_domain_error(
            input.request_id,
            input.source_conversation_id,
            point,
            provider_continuation_vault,
        )
    }

    fn fork_conversation_request_with_domain_error(
        &self,
        input: ForkConversationRequest,
        provider_continuation_vault: Option<&ProviderContinuationVault>,
    ) -> Result<ChatConversationRecord, conversation_fork_repository::ConversationForkError> {
        let point = input.resolve_point().map_err(|message| {
            conversation_fork_repository::ConversationForkError::Other(message)
        })?;
        self.fork_conversation_at_point_with_domain_error(
            input.request_id,
            input.source_conversation_id,
            point,
            provider_continuation_vault,
        )
    }

    fn fork_conversation_at_point_with_domain_error(
        &self,
        request_id: String,
        source_conversation_id: String,
        point: ConversationForkPoint,
        provider_continuation_vault: Option<&ProviderContinuationVault>,
    ) -> Result<ChatConversationRecord, conversation_fork_repository::ConversationForkError> {
        let mut connection = self.state.connection()?;
        if let Some(existing) =
            conversation_fork_repository::find_existing_fork(&connection, request_id.trim())
                .map_err(storage_error)?
        {
            let existing_point = existing.source_fork_point.unwrap_or_else(|| {
                ConversationForkPoint::AssistantReply {
                    assistant_message_id: existing.source_message_id.clone(),
                }
            });
            if existing.source_conversation_id != source_conversation_id || existing_point != point
            {
                return Err("同一个分叉请求 ID 不能用于不同的历史快照。"
                    .to_string()
                    .into());
            }
            let mut conversation =
                chat_repository::get_conversation(&connection, &existing.target_conversation_id)
                    .map_err(storage_error)?
                    .ok_or_else(|| "分叉记录指向的新任务不存在。".to_string())?;
            self.attach_message_attachments(&connection, std::slice::from_mut(&mut conversation))?;
            attach_message_guidance_timelines(
                &connection,
                std::slice::from_mut(&mut conversation),
            )?;
            return Ok(conversation);
        }

        let mut plan = conversation_fork_repository::build_fork_plan_at_point(
            &connection,
            request_id.trim(),
            source_conversation_id.trim(),
            &point,
            now_ms(),
        )?;
        ensure_project_reference_exists(&connection, plan.target.project_id.as_deref())?;
        let provider_continuations = if plan.provider_continuation_mappings.is_empty() {
            Vec::new()
        } else {
            let vault = provider_continuation_vault.ok_or_else(|| {
                conversation_fork_repository::ConversationForkError::Other(
                    "原任务包含 Provider continuation，但当前 Host 未提供安全克隆能力。"
                        .to_string(),
                )
            })?;
            vault
                .prepare_fork_clones(&plan.provider_continuation_mappings)
                .map_err(|error| {
                    conversation_fork_repository::ConversationForkError::Other(format!(
                        "Provider continuation 安全克隆失败：{}",
                        error.code()
                    ))
                })?
        };

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
            return Err(error.into());
        }

        if let Err(error) =
            conversation_fork_repository::commit_fork_plan_with_provider_continuations(
                &mut connection,
                &plan,
                &provider_continuations,
            )
        {
            cleanup_fork_files(&staged_files, &committed_files);
            return Err(error);
        }
        let mut conversation = chat_repository::get_conversation(&connection, &plan.target.id)
            .map_err(storage_error)?
            .ok_or_else(|| "新任务创建后无法重新读取。".to_string())?;
        self.attach_message_attachments(&connection, std::slice::from_mut(&mut conversation))?;
        attach_message_guidance_timelines(&connection, std::slice::from_mut(&mut conversation))?;
        Ok(conversation)
    }

    pub fn fork_conversation_view(
        &self,
        input: ForkConversationInput,
    ) -> Result<ChatConversationViewRecord, conversation_fork_repository::ConversationForkError>
    {
        let conversation = self.fork_conversation_with_domain_error(input, None)?;
        self.decorate_fork_conversation_view(conversation)
    }

    pub fn fork_conversation_view_with_provider_continuation_vault(
        &self,
        input: ForkConversationInput,
        provider_continuation_vault: &ProviderContinuationVault,
    ) -> Result<ChatConversationViewRecord, conversation_fork_repository::ConversationForkError>
    {
        let conversation =
            self.fork_conversation_with_domain_error(input, Some(provider_continuation_vault))?;
        self.decorate_fork_conversation_view(conversation)
    }

    pub fn fork_conversation_request_view(
        &self,
        input: ForkConversationRequest,
    ) -> Result<ChatConversationViewRecord, conversation_fork_repository::ConversationForkError>
    {
        let conversation = self.fork_conversation_request_with_domain_error(input, None)?;
        self.decorate_fork_conversation_view(conversation)
    }

    pub fn fork_conversation_request_view_with_provider_continuation_vault(
        &self,
        input: ForkConversationRequest,
        provider_continuation_vault: &ProviderContinuationVault,
    ) -> Result<ChatConversationViewRecord, conversation_fork_repository::ConversationForkError>
    {
        let conversation = self.fork_conversation_request_with_domain_error(
            input,
            Some(provider_continuation_vault),
        )?;
        self.decorate_fork_conversation_view(conversation)
    }

    fn decorate_fork_conversation_view(
        &self,
        conversation: ChatConversationRecord,
    ) -> Result<ChatConversationViewRecord, conversation_fork_repository::ConversationForkError>
    {
        let connection = self.state.connection()?;
        let continuation_origin =
            conversation_fork_repository::get_continuation_origin(&connection, &conversation.id)
                .map_err(storage_error)?;
        Ok(ChatConversationViewRecord {
            conversation,
            continuation_origin,
        })
    }

    pub fn search_chats(&self, input: &ChatSearchInput) -> Result<Vec<ChatSearchResult>, String> {
        let connection = self.state.connection()?;
        chat_search_repository::search_chats(&connection, input).map_err(storage_error)
    }

    pub fn search_conversation_history(
        &self,
        conversation_id: &str,
        query: &str,
        filter: &conversation_history_repository::ConversationHistorySearchFilter,
        limit: usize,
    ) -> Result<Vec<conversation_history_repository::ConversationHistorySearchHit>, String> {
        let connection = self.state.connection()?;
        conversation_history_repository::search_records(
            &connection,
            conversation_id,
            query,
            filter,
            limit,
        )
        .map_err(storage_error)
    }

    pub fn conversation_history_around(
        &self,
        conversation_id: &str,
        reference: &conversation_history_repository::ConversationHistoryRecordRef,
        before: usize,
        after: usize,
    ) -> Result<
        Option<Vec<conversation_history_repository::ConversationHistoryTimelineRecord>>,
        String,
    > {
        let connection = self.state.connection()?;
        conversation_history_repository::records_around(
            &connection,
            conversation_id,
            reference,
            before,
            after,
        )
        .map_err(storage_error)
    }

    pub fn conversation_history_range(
        &self,
        conversation_id: &str,
        start: &conversation_history_repository::ConversationHistoryRecordRef,
        end: &conversation_history_repository::ConversationHistoryRecordRef,
        limit: usize,
    ) -> Result<
        Option<Vec<conversation_history_repository::ConversationHistoryTimelineRecord>>,
        String,
    > {
        let connection = self.state.connection()?;
        conversation_history_repository::records_in_range(
            &connection,
            conversation_id,
            start,
            end,
            limit,
        )
        .map_err(storage_error)
    }

    pub fn conversation_history_tool_exchange(
        &self,
        conversation_id: &str,
        reference: Option<&conversation_history_repository::ConversationHistoryRecordRef>,
        call_id: Option<&str>,
        run_id: Option<&str>,
    ) -> Result<Option<Vec<conversation_history_repository::ConversationHistoryRecord>>, String>
    {
        let connection = self.state.connection()?;
        conversation_history_repository::get_tool_exchange(
            &connection,
            conversation_id,
            reference,
            call_id,
            run_id,
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

    pub fn archive_conversation_tool_result(
        &self,
        input: conversation_history_archive_repository::ConversationHistoryArchiveInput,
    ) -> Result<conversation_history_archive_repository::ConversationHistoryArchiveDescriptor, String>
    {
        let mut connection = self.state.connection()?;
        conversation_history_archive_repository::store_archive(&mut connection, &input)
            .map_err(storage_error)
    }

    pub fn archive_conversation_tool_result_file(
        &self,
        input: conversation_history_archive_repository::ConversationHistoryArchiveFileInput,
    ) -> Result<conversation_history_archive_repository::ConversationHistoryArchiveDescriptor, String>
    {
        let mut connection = self.state.connection()?;
        conversation_history_archive_repository::store_archive_file(&mut connection, &input)
            .map_err(storage_error)
    }

    pub fn find_conversation_history_archive_for_trace_item(
        &self,
        conversation_id: &str,
        assistant_message_id: &str,
        sequence: u64,
    ) -> Result<
        Option<conversation_history_archive_repository::ConversationHistoryArchiveDescriptor>,
        String,
    > {
        let connection = self.state.connection()?;
        conversation_history_archive_repository::find_archive_for_trace_item(
            &connection,
            conversation_id,
            assistant_message_id,
            sequence,
        )
        .map_err(storage_error)
    }

    pub fn find_conversation_history_archive_by_ref(
        &self,
        conversation_id: &str,
        archive_ref: &str,
    ) -> Result<
        Option<conversation_history_archive_repository::ConversationHistoryArchiveDescriptor>,
        String,
    > {
        let connection = self.state.connection()?;
        conversation_history_archive_repository::find_archive_by_ref(
            &connection,
            conversation_id,
            archive_ref,
        )
        .map_err(storage_error)
    }

    pub fn find_conversation_history_archive_match_char_offset(
        &self,
        conversation_id: &str,
        archive_ref: &str,
        query: &str,
    ) -> Result<Option<u64>, String> {
        let connection = self.state.connection()?;
        conversation_history_archive_repository::find_archive_match_char_offset(
            &connection,
            conversation_id,
            archive_ref,
            query,
        )
        .map_err(storage_error)
    }

    pub fn read_conversation_history_archive_page(
        &self,
        conversation_id: &str,
        archive_ref: &str,
        unit: conversation_history_archive_repository::ConversationHistoryArchivePageUnit,
        start: u64,
        maximum: u64,
    ) -> Result<
        Option<conversation_history_archive_repository::ConversationHistoryArchivePage>,
        String,
    > {
        let connection = self.state.connection()?;
        conversation_history_archive_repository::read_archive_page(
            &connection,
            conversation_id,
            archive_ref,
            unit,
            start,
            maximum,
        )
        .map_err(storage_error)
    }

    /// Follows the same opaque Archive route accepted by `conversation_history` while preserving
    /// the caller's conversation boundary. This narrow storage entry point is useful to Host
    /// integrations that must verify a ToolResult recovery route without exposing archive ids or
    /// the route codec itself.
    pub fn read_conversation_history_archive_page_from_open(
        &self,
        conversation_id: &str,
        open: &str,
        maximum_chars: u64,
    ) -> Result<
        Option<conversation_history_archive_repository::ConversationHistoryArchivePage>,
        String,
    > {
        let crate::storage::conversation_history_open::HistoryOpenRoute::Archive {
            archive_ref,
            start_char,
        } = crate::storage::conversation_history_open::decode_history_open(open)?
        else {
            return Err("history open does not reference an Exact Archive page".to_string());
        };
        self.read_conversation_history_archive_page(
            conversation_id,
            &archive_ref,
            conversation_history_archive_repository::ConversationHistoryArchivePageUnit::Char,
            start_char,
            maximum_chars,
        )
    }

    /// Creates the opaque Archive route accepted by `conversation_history` after verifying that
    /// the immutable Archive belongs to the caller's conversation.
    ///
    /// Host integrations use this narrow method to project a recovery capability without exposing
    /// the raw Archive reference or the route codec across the Core boundary.
    pub fn conversation_history_archive_open(
        &self,
        conversation_id: &str,
        archive_ref: &str,
    ) -> Result<Option<String>, String> {
        if self
            .find_conversation_history_archive_by_ref(conversation_id, archive_ref)?
            .is_none()
        {
            return Ok(None);
        }
        crate::storage::conversation_history_open::encode_archive_history_open(archive_ref, 0)
            .map(Some)
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
            provider_continuation_repository::delete_for_conversation(
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
        let transaction = connection.transaction().map_err(storage_error)?;
        usage_repository::roll_up_deleted_usage_for_messages(
            &transaction,
            conversation_id,
            message_ids,
            now_ms(),
        )
        .map_err(storage_error)?;
        let attachments = attachment_repository::list_message_attachments(
            &transaction,
            conversation_id,
            message_ids,
        )
        .map_err(storage_error)?;
        attachment_repository::delete_message_attachments(
            &transaction,
            conversation_id,
            message_ids,
        )
        .map_err(storage_error)?;
        pending_action_repository::delete_pending_actions_for_messages(
            &transaction,
            conversation_id,
            message_ids,
        )
        .map_err(storage_error)?;
        agent_action_audit_repository::delete_action_audit_for_messages(
            &transaction,
            conversation_id,
            message_ids,
        )
        .map_err(storage_error)?;
        chat_repository::delete_messages_in_transaction(&transaction, conversation_id, message_ids)
            .map_err(storage_error)?;
        transaction.commit().map_err(storage_error)?;
        if let Err(error) = self.cleanup_attachment_files(attachments) {
            eprintln!("failed to remove deleted message attachment files: {error}");
        }
        Ok(())
    }
}

fn attach_message_guidance_timelines(
    connection: &rusqlite::Connection,
    conversations: &mut [ChatConversationRecord],
) -> Result<(), String> {
    for conversation in conversations {
        let traces = conversation_trace_repository::list_traces_for_conversation(
            connection,
            &conversation.id,
        )
        .map_err(storage_error)?;
        let traces = traces
            .into_iter()
            .map(|trace| (trace.assistant_message_id.clone(), trace))
            .collect::<HashMap<_, _>>();

        for message in &mut conversation.messages {
            if message.role != "assistant" {
                continue;
            }
            let guidances =
                guidance_repository::list_guidances_for_assistant_message(connection, &message.id)
                    .map_err(storage_error)?;
            let trace = traces.get(&message.id);
            if trace.is_none() && guidances.is_empty() {
                continue;
            }
            message.agent_run_json = Some(project_guidance_timeline(
                connection,
                message.agent_run_json.as_deref(),
                trace,
                &guidances,
                message.created_at,
            )?);
        }
    }
    Ok(())
}

fn project_guidance_timeline(
    connection: &rusqlite::Connection,
    existing_run_json: Option<&str>,
    trace: Option<&ConversationTurnTrace>,
    guidances: &[AgentRunGuidanceRecord],
    fallback_started_at: i64,
) -> Result<String, String> {
    let mut run = existing_run_json
        .and_then(|raw| serde_json::from_str::<serde_json::Value>(raw).ok())
        .and_then(|value| value.as_object().cloned())
        .unwrap_or_default();
    let mcp_trace_anchors = mcp_trace_anchors(&run);
    let existing_timeline = run
        .remove("timeline")
        .and_then(|value| value.as_array().cloned())
        .unwrap_or_default();
    let presentation_only_items = existing_timeline
        .into_iter()
        .filter(|item| {
            !matches!(
                item.get("type").and_then(serde_json::Value::as_str),
                Some("message" | "tool_call" | "user_guidance" | "mcp_tool_call")
            )
        })
        .collect::<Vec<_>>();
    // Renderer-only items that have no durable trace identity remain presentation-only. Typed MCP
    // items are rebuilt below from their call-id anchors so they retain their original sequence.
    let mut timeline = presentation_only_items;
    let mut emitted_mcp_invocations = HashSet::new();

    if let Some(trace) = trace {
        run.insert("runId".to_string(), trace.run_id.clone().into());
        for item in &trace.items {
            match item {
                ConversationTurnTraceItem::AssistantNarration {
                    sequence, content, ..
                } => timeline.push(serde_json::json!({
                    "id": format!("trace-message-{sequence}"),
                    "type": "message",
                    "content": content,
                })),
                ConversationTurnTraceItem::UserGuidance {
                    sequence,
                    guidance_id,
                    client_message_id,
                    content,
                    attachments,
                    created_at,
                    ..
                } => timeline.push(serde_json::json!({
                    "id": format!("user-guidance-{client_message_id}"),
                    "type": "user_guidance",
                    "guidanceId": guidance_id,
                    "clientMessageId": client_message_id,
                    "content": content,
                    "attachments": attachments,
                    "status": "applied",
                    "createdAt": created_at,
                    "sequence": sequence,
                })),
                ConversationTurnTraceItem::ToolCall {
                    call_id, sequence, ..
                } => {
                    if let Some(invocation_id) = mcp_trace_anchors.get(call_id) {
                        if emitted_mcp_invocations.insert(invocation_id.clone()) {
                            timeline.push(serde_json::json!({
                                "id": format!("mcp-invocation-{invocation_id}"),
                                "type": "mcp_tool_call",
                                "invocationId": invocation_id,
                            }));
                        }
                    } else {
                        timeline.push(serde_json::json!({
                            "id": format!("tool-call-{call_id}"),
                            "type": "tool_call",
                            "callId": call_id,
                            "traceSequence": sequence,
                        }));
                    }
                }
                ConversationTurnTraceItem::ToolResult { .. }
                | ConversationTurnTraceItem::CommandSessionLifecycle { .. } => {}
            }
        }
    }

    for guidance in guidances {
        if !matches!(
            guidance.status,
            crate::AgentGuidanceStatus::Queued | crate::AgentGuidanceStatus::Abandoned
        ) {
            continue;
        }
        let mut attachments = Vec::with_capacity(guidance.attachment_ids.len());
        for attachment_id in &guidance.attachment_ids {
            let attachment = attachment_repository::get_attachment(connection, attachment_id)
                .map_err(storage_error)?
                .ok_or_else(|| {
                    format!(
                        "guidance `{}` references missing attachment `{attachment_id}`",
                        guidance.guidance_id
                    )
                })?;
            attachments.push(serde_json::json!({
                "id": attachment.id,
                "kind": attachment.kind,
                "name": attachment.original_name,
                "mimeType": attachment.mime_type,
                "sizeBytes": attachment.size_bytes,
            }));
        }
        run.insert("runId".to_string(), guidance.run_id.clone().into());
        if guidance.status == crate::AgentGuidanceStatus::Abandoned {
            timeline.push(serde_json::json!({
                "id": format!("user-guidance-{}", guidance.client_message_id),
                "type": "user_guidance",
                "guidanceId": guidance.guidance_id,
                "clientMessageId": guidance.client_message_id,
                "content": guidance.content,
                "attachments": attachments,
                "status": "rejected",
                "rejectionCode": "run_interrupted",
                "error": guidance.terminal_reason,
                "recoverable": true,
                "createdAt": guidance.created_at,
            }));
        } else {
            timeline.push(serde_json::json!({
                "id": format!("user-guidance-{}", guidance.client_message_id),
                "type": "user_guidance",
                "guidanceId": guidance.guidance_id,
                "clientMessageId": guidance.client_message_id,
                "content": guidance.content,
                "attachments": attachments,
                "status": "queued",
                "createdAt": guidance.created_at,
            }));
        }
    }

    run.insert("timeline".to_string(), timeline.into());
    run.entry("startedAt".to_string())
        .or_insert_with(|| fallback_started_at.into());
    for field in [
        "toolDefinitions",
        "toolCalls",
        "toolResults",
        "approvals",
        "diffs",
        "fileDrafts",
        "webSearchActivities",
        "readActivities",
    ] {
        run.entry(field.to_string())
            .or_insert_with(|| serde_json::json!([]));
    }
    run.entry("messageStreamCheckpoints".to_string())
        .or_insert_with(|| serde_json::json!({}));
    if !run.contains_key("status") {
        let status = trace
            .map(|trace| match trace.terminal_status {
                crate::ConversationTurnTraceTerminalStatus::InProgress => "running",
                crate::ConversationTurnTraceTerminalStatus::Completed => "completed",
                crate::ConversationTurnTraceTerminalStatus::Failed => "failed",
                crate::ConversationTurnTraceTerminalStatus::Cancelled => "cancelled",
            })
            .unwrap_or("running");
        run.insert("status".to_string(), status.into());
    }

    serde_json::to_string(&serde_json::Value::Object(run))
        .map_err(|error| format!("serialize guidance timeline: {error}"))
}

fn mcp_trace_anchors(run: &serde_json::Map<String, serde_json::Value>) -> HashMap<String, String> {
    let Some(invocations) = run
        .get("mcpInvocations")
        .and_then(serde_json::Value::as_array)
    else {
        return HashMap::new();
    };

    let mut invocation_ids_by_call_id = HashMap::<String, HashSet<String>>::new();
    let mut call_ids_by_invocation_id = HashMap::<String, HashSet<String>>::new();
    for invocation in invocations {
        let Some(call_id) = invocation
            .get("callId")
            .and_then(serde_json::Value::as_str)
            .filter(|value| !value.is_empty())
        else {
            continue;
        };
        let Some(invocation_id) = invocation
            .get("invocationId")
            .and_then(serde_json::Value::as_str)
            .filter(|value| !value.is_empty())
        else {
            continue;
        };
        invocation_ids_by_call_id
            .entry(call_id.to_string())
            .or_default()
            .insert(invocation_id.to_string());
        call_ids_by_invocation_id
            .entry(invocation_id.to_string())
            .or_default()
            .insert(call_id.to_string());
    }

    invocation_ids_by_call_id
        .into_iter()
        .filter_map(|(call_id, invocation_ids)| {
            if invocation_ids.len() != 1 {
                return None;
            }
            let invocation_id = invocation_ids.into_iter().next()?;
            (call_ids_by_invocation_id
                .get(&invocation_id)
                .is_some_and(|call_ids| call_ids.len() == 1))
            .then_some((call_id, invocation_id))
        })
        .collect()
}
