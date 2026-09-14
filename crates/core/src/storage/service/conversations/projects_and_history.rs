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
        let tree_scopes = project_agent_tree_deletion_scopes(&connection, project_id)?;
        let conversation_ids = project_conversation_ids(&connection, project_id)?;
        if tree_scopes.is_empty() {
            let transaction = connection.transaction().map_err(storage_error)?;
            automation_repository::prepare_tombstoned_automations_for_project_delete(
                &transaction,
                project_id,
                &conversation_ids,
            )
            .map_err(storage_error)?;
            let deleted_at = now_ms();
            for conversation_id in &conversation_ids {
                notification_repository::resolve_notification_events_by_conversation_id_in_transaction(
                    &transaction,
                    conversation_id,
                    deleted_at,
                )
                .map_err(storage_error)?;
            }
            automation_repository::terminalize_automation_runs_before_project_delete(
                &transaction,
                project_id,
                &conversation_ids,
                now_ms(),
            )
            .map_err(storage_error)?;
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
        } else {
            with_agent_deletion_transaction(&mut connection, |transaction| {
                automation_repository::prepare_tombstoned_automations_for_project_delete(
                    transaction,
                    project_id,
                    &conversation_ids,
                )
                .map_err(storage_error)?;
                automation_repository::terminalize_automation_runs_before_project_delete(
                    transaction,
                    project_id,
                    &conversation_ids,
                    now_ms(),
                )
                .map_err(storage_error)?;
                ensure_deletion_scope_has_no_active_execution(transaction, &conversation_ids)?;
                automation_repository::invalidate_automations_before_trigger_disabled_project_delete(
                    transaction,
                    project_id,
                    &conversation_ids,
                    now_ms(),
                )
                .map_err(storage_error)?;
                usage_repository::roll_up_deleted_usage_for_project(
                    transaction,
                    project_id,
                    now_ms(),
                )
                .map_err(storage_error)?;
                pending_action_repository::delete_pending_actions_for_project(
                    transaction,
                    project_id,
                )
                .map_err(storage_error)?;
                agent_action_audit_repository::delete_action_audit_for_project(
                    transaction,
                    project_id,
                )
                .map_err(storage_error)?;
                composer_draft_repository::delete_project_composer_drafts(transaction, project_id)
                    .map_err(storage_error)?;
                if !conversation_ids.is_empty() {
                    let placeholders = std::iter::repeat_n("?", conversation_ids.len())
                        .collect::<Vec<_>>()
                        .join(", ");
                    transaction
                        .execute(
                            &format!(
                                "DELETE FROM conversation_history_fts
                                 WHERE conversation_id IN ({placeholders})"
                            ),
                            rusqlite::params_from_iter(conversation_ids.iter()),
                        )
                        .map_err(storage_error)?;
                }
                for scope in &tree_scopes {
                    delete_agent_tree_records(transaction, scope)?;
                }
                project_repository::delete_project(transaction, project_id)
                    .map_err(storage_error)?;
                Ok(())
            })?;
        }
        drop(connection);
        if let Err(error) = self.cleanup_attachment_files(attachments) {
            eprintln!("failed to remove deleted project attachment files: {error}");
        }
        Ok(())
    }

    pub fn load_conversations(&self) -> Result<Vec<ChatConversationRecord>, String> {
        let connection = self.state.connection()?;
        let mut conversations =
            chat_repository::list_active_conversations(&connection).map_err(storage_error)?;
        let preview_attachments =
            self.attach_message_attachments(&connection, &mut conversations)?;
        attach_message_guidance_timelines(&connection, &mut conversations)?;
        drop(connection);
        self.hydrate_message_attachment_previews(&mut conversations, preview_attachments);
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
            .filter_map(
                |conversation| match agent_graph_repository::get_agent_node_by_conversation(
                    &connection,
                    &conversation.id,
                ) {
                    Ok(Some(node)) if node.parent_agent_id.is_some() => None,
                    Ok(_) => Some(Ok(conversation)),
                    Err(error) => Some(Err(error.to_string())),
                },
            )
            .collect::<Result<Vec<_>, String>>()?
            .into_iter()
            .map(|conversation| {
                let mut conversation = conversation;
                chat_repository::retain_user_facing_root_messages(&connection, &mut conversation)
                    .map_err(storage_error)?;
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
        chat_repository::list_root_conversation_metas(&connection).map_err(storage_error)
    }

    pub fn load_conversation(
        &self,
        conversation_id: &str,
    ) -> Result<Option<ChatConversationRecord>, String> {
        let connection = self.state.connection()?;
        let mut conversation =
            chat_repository::get_active_conversation(&connection, conversation_id)
                .map_err(storage_error)?;
        let preview_attachments = if let Some(conversation) = &mut conversation {
            let preview_attachments =
                self.attach_message_attachments(&connection, std::slice::from_mut(conversation))?;
            attach_message_guidance_timelines(&connection, std::slice::from_mut(conversation))?;
            preview_attachments
        } else {
            Vec::new()
        };
        drop(connection);
        if let Some(conversation) = &mut conversation {
            self.hydrate_message_attachment_previews(
                std::slice::from_mut(conversation),
                preview_attachments,
            );
        }
        Ok(conversation)
    }

    /// Loads a complete observer Conversation and its input provenance from one SQLite read cut.
    ///
    /// This is intentionally a narrow bulk boundary: actor facts are decoded in one ordered
    /// query and returned as an in-memory map, rather than opening a new connection for every
    /// message. A missing or corrupt origin aborts the whole snapshot instead of silently
    /// presenting an Agent instruction as a human message.
    pub fn load_conversation_observer_snapshot(
        &self,
        conversation_id: &str,
    ) -> Result<Option<ConversationObserverSnapshot>, String> {
        let mut connection = self.state.connection()?;
        let transaction = connection.transaction().map_err(storage_error)?;
        let mut conversation =
            chat_repository::get_active_conversation(&transaction, conversation_id)
                .map_err(storage_error)?;
        let Some(mut conversation) = conversation.take() else {
            transaction.commit().map_err(storage_error)?;
            return Ok(None);
        };
        let preview_attachments =
            self.attach_message_attachments(&transaction, std::slice::from_mut(&mut conversation))?;
        attach_message_guidance_timelines(&transaction, std::slice::from_mut(&mut conversation))?;
        let mut input_origins =
            agent_graph_repository::conversation_message_origins(&transaction, conversation_id)
                .map_err(|error| error.to_string())?
                .into_iter()
                .collect::<BTreeMap<_, _>>();
        let active_user_ids = conversation
            .messages
            .iter()
            .filter(|message| message.role == "user")
            .map(|message| message.id.as_str())
            .collect::<HashSet<_>>();
        input_origins.retain(|message_id, _| active_user_ids.contains(message_id.as_str()));
        let expected_input_count = conversation
            .messages
            .iter()
            .filter(|message| message.role == "user")
            .count();
        if input_origins.len() != expected_input_count
            || conversation
                .messages
                .iter()
                .filter(|message| message.role == "user")
                .any(|message| !input_origins.contains_key(&message.id))
        {
            return Err(
                "Conversation observer snapshot has incomplete input provenance.".to_string(),
            );
        }
        transaction.commit().map_err(storage_error)?;
        drop(connection);
        self.hydrate_message_attachment_previews(
            std::slice::from_mut(&mut conversation),
            preview_attachments,
        );
        Ok(Some(ConversationObserverSnapshot {
            conversation,
            input_origins,
        }))
    }

    /// Loads the complete persisted Conversation together with the opaque revision used for Turn
    /// admission. Both facts come from one SQLite read transaction, so a later
    /// `save_conversation_and_begin_turn` can reject a stale full snapshot even when a competing
    /// Host has already completed and released its active trace.
    ///
    /// Unlike renderer/observer reads, this admission snapshot deliberately does not decorate
    /// `agent_run_json` from durable Trace, Guidance, or Command Session facts. Those decorations
    /// are presentation projections and may differ from the raw JSON copied into immutable Agent
    /// and fork-snapshot messages. Feeding them back into a write would make a valid continuation
    /// look like an attempted rewrite of immutable history.
    pub fn load_conversation_for_turn(
        &self,
        conversation_id: &str,
    ) -> Result<(Option<ChatConversationRecord>, Option<i64>), String> {
        let mut connection = self.state.connection()?;
        let transaction = connection.transaction().map_err(storage_error)?;
        let revision = transaction
            .query_row(
                "SELECT revision FROM conversations WHERE id = ?1",
                [conversation_id],
                |row| row.get::<_, i64>(0),
            )
            .optional()
            .map_err(storage_error)?;
        let mut conversation =
            chat_repository::get_active_persisted_conversation(&transaction, conversation_id)
                .map_err(storage_error)?;
        if conversation.is_some() != revision.is_some() {
            return Err(
                "Conversation Turn admission snapshot is internally inconsistent.".to_string(),
            );
        }
        let preview_attachments = if let Some(conversation) = &mut conversation {
            self.attach_message_attachments(&transaction, std::slice::from_mut(conversation))?
        } else {
            Vec::new()
        };
        transaction.commit().map_err(storage_error)?;
        drop(connection);
        if let Some(conversation) = &mut conversation {
            self.hydrate_message_attachment_previews(
                std::slice::from_mut(conversation),
                preview_attachments,
            );
        }
        Ok((conversation, revision))
    }

    pub fn conversation_revision(&self, conversation_id: &str) -> Result<Option<i64>, String> {
        let connection = self.state.connection()?;
        connection
            .query_row(
                "SELECT revision FROM conversations WHERE id = ?1",
                [conversation_id],
                |row| row.get::<_, i64>(0),
            )
            .optional()
            .map_err(storage_error)
    }

    pub fn load_conversation_view(
        &self,
        conversation_id: &str,
    ) -> Result<Option<ChatConversationViewRecord>, String> {
        let Some(mut conversation) = self.load_conversation(conversation_id)? else {
            return Ok(None);
        };
        let connection = self.state.connection()?;
        chat_repository::retain_user_facing_root_messages(&connection, &mut conversation)
            .map_err(storage_error)?;
        let continuation_origin =
            conversation_fork_repository::get_continuation_origin(&connection, conversation_id)
                .map_err(storage_error)?;
        Ok(Some(ChatConversationViewRecord {
            conversation,
            continuation_origin,
        }))
    }

    fn fork_conversation_request_with_domain_error(
        &self,
        input: ForkConversationRequest,
        provider_continuation_vault: Option<&ProviderContinuationVault>,
    ) -> Result<ChatConversationRecord, conversation_fork_repository::ConversationForkError> {
        self.fork_conversation_at_point_with_domain_error(
            input.request_id,
            input.source_conversation_id,
            input.fork_point,
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
        conversation_fork_repository::validate_fork_point_input(
            &request_id,
            &source_conversation_id,
            &point,
        )
        .map_err(conversation_fork_repository::ConversationForkError::Other)?;
        let mut connection = self.state.connection()?;
        if let Some(existing) =
            conversation_fork_repository::find_existing_fork(&connection, &request_id)
                .map_err(storage_error)?
        {
            if existing.source_conversation_id != source_conversation_id
                || existing.source_fork_point != point
            {
                return Err("同一个分叉请求 ID 不能用于不同的历史快照。"
                    .to_string()
                    .into());
            }
            conversation_fork_repository::validate_existing_fork_authority(&connection, &existing)?;
            let mut conversation = chat_repository::get_active_conversation(
                &connection,
                &existing.target_conversation_id,
            )
            .map_err(storage_error)?
            .ok_or_else(|| "分叉记录指向的新任务不存在。".to_string())?;
            let preview_attachments = self
                .attach_message_attachments(&connection, std::slice::from_mut(&mut conversation))?;
            attach_message_guidance_timelines(
                &connection,
                std::slice::from_mut(&mut conversation),
            )?;
            drop(connection);
            self.hydrate_message_attachment_previews(
                std::slice::from_mut(&mut conversation),
                preview_attachments,
            );
            return Ok(conversation);
        }

        let mut plan = conversation_fork_repository::build_fork_plan_at_point(
            &connection,
            &request_id,
            &source_conversation_id,
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
            for attachment in plan.attachments_mut() {
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
            drop(connection);
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
            drop(connection);
            cleanup_fork_files(&staged_files, &committed_files);
            return Err(error);
        }
        let mut conversation = chat_repository::get_conversation(&connection, &plan.target.id)
            .map_err(storage_error)?
            .ok_or_else(|| "新任务创建后无法重新读取。".to_string())?;
        let preview_attachments =
            self.attach_message_attachments(&connection, std::slice::from_mut(&mut conversation))?;
        attach_message_guidance_timelines(&connection, std::slice::from_mut(&mut conversation))?;
        drop(connection);
        self.hydrate_message_attachment_previews(
            std::slice::from_mut(&mut conversation),
            preview_attachments,
        );
        Ok(conversation)
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
        mut conversation: ChatConversationRecord,
    ) -> Result<ChatConversationViewRecord, conversation_fork_repository::ConversationForkError>
    {
        let connection = self.state.connection()?;
        chat_repository::retain_user_facing_root_messages(&connection, &mut conversation)
            .map_err(storage_error)?;
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

    pub fn project_agent_messages_for_model(
        &self,
        conversation_id: &str,
        messages: &mut [crate::AgentChatMessage],
    ) -> Result<(), String> {
        let connection = self.state.connection()?;
        for message in messages.iter_mut().filter(|message| message.role == "user") {
            let Some(message_id) = message.message_id.as_deref() else {
                continue;
            };
            if let Some(content) = crate::storage::agent_message_model_projection::project_message(
                &connection,
                conversation_id,
                message_id,
            )? {
                message.content = content;
            }
        }
        Ok(())
    }

    pub fn project_history_conversation_for_model(
        &self,
        conversation: &mut ChatConversationRecord,
    ) -> Result<(), String> {
        let connection = self.state.connection()?;
        for message in conversation
            .messages
            .iter_mut()
            .filter(|message| message.role == "user")
        {
            if let Some(content) = crate::storage::agent_message_model_projection::project_message(
                &connection,
                &conversation.id,
                &message.id,
            )? {
                message.content = content;
            }
        }
        Ok(())
    }

    pub fn search_conversation_history(
        &self,
        conversation_id: &str,
        query: &str,
        filter: &conversation_history_repository::ConversationHistorySearchFilter,
        limit: usize,
    ) -> Result<Vec<conversation_history_repository::ConversationHistorySearchHit>, String> {
        let connection = self.state.connection()?;
        let mut hits = conversation_history_repository::search_records(
            &connection,
            conversation_id,
            query,
            filter,
            limit,
        )
        .map_err(storage_error)?;
        for hit in &mut hits {
            crate::storage::agent_message_model_projection::project_history_preview(
                &connection,
                conversation_id,
                &hit.reference,
                &mut hit.preview,
                &mut hit.preview_truncated,
            )?;
        }
        Ok(hits)
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
        let mut records = conversation_history_repository::records_around(
            &connection,
            conversation_id,
            reference,
            before,
            after,
        )
        .map_err(storage_error)?;
        for record in records.iter_mut().flatten() {
            crate::storage::agent_message_model_projection::project_history_preview(
                &connection,
                conversation_id,
                &record.reference,
                &mut record.preview,
                &mut record.preview_truncated,
            )?;
        }
        Ok(records)
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
        let mut records = conversation_history_repository::records_in_range(
            &connection,
            conversation_id,
            start,
            end,
            limit,
        )
        .map_err(storage_error)?;
        for record in records.iter_mut().flatten() {
            crate::storage::agent_message_model_projection::project_history_preview(
                &connection,
                conversation_id,
                &record.reference,
                &mut record.preview,
                &mut record.preview_truncated,
            )?;
        }
        Ok(records)
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
        let mut record =
            conversation_history_repository::read_record(&connection, conversation_id, reference)
                .map_err(storage_error)?;
        if let Some(record) = record.as_mut() {
            crate::storage::agent_message_model_projection::project_history_record(
                &connection,
                conversation_id,
                record,
            )?;
        }
        Ok(record)
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

    pub fn delete_unreferenced_exact_conversation_tool_result(
        &self,
        input: conversation_history_archive_repository::ConversationHistoryArchiveInput,
    ) -> Result<bool, String> {
        let mut connection = self.state.connection()?;
        conversation_history_archive_repository::delete_unreferenced_exact_archive(
            &mut connection,
            &input,
        )
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
        if conversation_turn_rewrite_repository::superseded_message_ids(
            &connection,
            conversation_id,
        )
        .map_err(storage_error)?
        .contains(assistant_message_id)
        {
            return Ok(None);
        }
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
        let archive = conversation_history_archive_repository::find_archive_by_ref(
            &connection,
            conversation_id,
            archive_ref,
        )
        .map_err(storage_error)?;
        let Some(archive) = archive else {
            return Ok(None);
        };
        if conversation_turn_rewrite_repository::superseded_message_ids(
            &connection,
            conversation_id,
        )
        .map_err(storage_error)?
        .contains(&archive.assistant_message_id)
        {
            return Ok(None);
        }
        Ok(Some(archive))
    }

    pub fn find_conversation_history_archive_match_char_offset(
        &self,
        conversation_id: &str,
        archive_ref: &str,
        query: &str,
    ) -> Result<Option<u64>, String> {
        let connection = self.state.connection()?;
        let Some(archive) = conversation_history_archive_repository::find_archive_by_ref(
            &connection,
            conversation_id,
            archive_ref,
        )
        .map_err(storage_error)?
        else {
            return Ok(None);
        };
        if conversation_turn_rewrite_repository::superseded_message_ids(
            &connection,
            conversation_id,
        )
        .map_err(storage_error)?
        .contains(&archive.assistant_message_id)
        {
            return Ok(None);
        }
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
        let Some(archive) = conversation_history_archive_repository::find_archive_by_ref(
            &connection,
            conversation_id,
            archive_ref,
        )
        .map_err(storage_error)?
        else {
            return Ok(None);
        };
        if conversation_turn_rewrite_repository::superseded_message_ids(
            &connection,
            conversation_id,
        )
        .map_err(storage_error)?
        .contains(&archive.assistant_message_id)
        {
            return Ok(None);
        }
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
}
