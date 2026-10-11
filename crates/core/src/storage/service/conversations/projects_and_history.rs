impl StorageService {
    fn with_conversation_read_snapshot<T>(
        &self,
        read: impl FnOnce(&rusqlite::Connection) -> Result<T, String>,
    ) -> Result<T, String> {
        let mut connection = self.state.read_connection()?;
        let transaction = connection.transaction().map_err(storage_error)?;
        let result = read(&transaction)?;
        transaction.commit().map_err(storage_error)?;
        Ok(result)
    }

    pub fn load_projects(&self) -> Result<Vec<ProjectRecord>, String> {
        self.with_conversation_read_snapshot(|connection| {
            project_repository::list_projects(connection).map_err(storage_error)
        })
    }

    pub fn save_project(&self, project: ProjectRecord) -> Result<ProjectRecord, String> {
        self.state.write_sync(
            crate::storage::write_size::estimated_json_bytes(&project)?,
            crate::storage::StorageWriteClass::Normal,
            |connection| {
                project_repository::save_project(connection, project.clone())
                    .map_err(storage_error)?;
                Ok(project)
            },
        )?
    }

    pub fn delete_project(&self, project_id: &str) -> Result<(), String> {
        let attachments = self.state.write_sync(
            project_id.len().saturating_add(256),
            crate::storage::StorageWriteClass::Normal,
            |connection| {
        let attachments =
            attachment_repository::list_project_deletion_attachments(connection, project_id)
                .map_err(storage_error)?;
        let tree_scopes = project_agent_tree_deletion_scopes(connection, project_id)?;
        let conversation_ids = project_conversation_ids(connection, project_id)?;
        if tree_scopes.is_empty() {
            let transaction = connection.transaction().map_err(storage_error)?;
            invalidate_workflows_before_trigger_disabled_deletion(&transaction, &conversation_ids)?;
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
            with_agent_deletion_transaction(connection, |transaction| {
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
                invalidate_workflows_before_trigger_disabled_deletion(
                    transaction,
                    &conversation_ids,
                )?;
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
        Ok::<_, String>(attachments)
            },
        )??;
        if let Err(error) = self.cleanup_attachment_files(attachments) {
            eprintln!("failed to remove deleted project attachment files: {error}");
        }
        Ok(())
    }

    pub fn load_conversations(&self) -> Result<Vec<ChatConversationRecord>, String> {
        self.load_conversations_with_projection_evidence()
            .map(|(conversations, _)| conversations)
    }

    fn load_conversations_with_projection_evidence(
        &self,
    ) -> Result<(Vec<ChatConversationRecord>, ConversationProjectionEvidence), String> {
        let mut pooled_connection = self.state.read_connection()?;
        let connection = pooled_connection.transaction().map_err(storage_error)?;
        let mut conversations =
            chat_repository::list_active_conversations(&connection).map_err(storage_error)?;
        let preview_attachments =
            self.attach_message_attachments(&connection, &mut conversations)?;
        let evidence = attach_message_guidance_timelines(&connection, &mut conversations)?;
        connection.commit().map_err(storage_error)?;
        drop(pooled_connection);
        self.hydrate_message_attachment_previews(&mut conversations, preview_attachments);
        Ok((conversations, evidence))
    }

    /// Loads complete conversations for the renderer and decorates only forked tasks with their
    /// backend-owned continuation boundary. Internal Agent callers continue to use the plain
    /// conversation record and never consume presentation lineage.
    pub fn load_conversation_views(&self) -> Result<Vec<ChatConversationViewRecord>, String> {
        let (mut conversations, previews, origins) =
            self.with_conversation_read_snapshot(|connection| {
                let conversations = chat_repository::list_active_conversations(connection)
                    .map_err(storage_error)?;
                let mut conversations = conversations
                    .into_iter()
                    .filter_map(|conversation| {
                        match agent_graph_repository::get_agent_node_by_conversation(
                            connection,
                            &conversation.id,
                        ) {
                            Ok(Some(node)) if node.parent_agent_id.is_some() => None,
                            Ok(_) => Some(Ok(conversation)),
                            Err(error) => Some(Err(error.to_string())),
                        }
                    })
                    .collect::<Result<Vec<_>, String>>()?;
                let previews = self.attach_message_attachments(connection, &mut conversations)?;
                let evidence = attach_message_guidance_timelines(connection, &mut conversations)?;
                let mut origins = Vec::with_capacity(conversations.len());
                for conversation in &mut conversations {
                    chat_repository::retain_user_facing_root_messages(connection, conversation)
                        .map_err(storage_error)?;
                    evidence.retain_workflow_top_level_messages(conversation);
                    origins.push(
                        conversation_fork_repository::get_continuation_origin(
                            connection,
                            &conversation.id,
                        )
                        .map_err(storage_error)?,
                    );
                }
                Ok((conversations, previews, origins))
            })?;
        self.hydrate_message_attachment_previews(&mut conversations, previews);
        Ok(conversations
            .into_iter()
            .zip(origins)
            .map(
                |(conversation, continuation_origin)| ChatConversationViewRecord {
                    conversation,
                    continuation_origin,
                },
            )
            .collect())
    }

    pub fn load_conversation_meta(
        &self,
        conversation_id: &str,
    ) -> Result<Option<ChatConversationMetaRecord>, String> {
        chat_repository::get_conversation_meta(&*self.state.read_connection()?, conversation_id)
            .map_err(storage_error)
    }

    pub fn load_conversation_metas(&self) -> Result<Vec<ChatConversationMetaRecord>, String> {
        self.with_conversation_read_snapshot(|connection| {
            chat_repository::list_root_conversation_metas(connection).map_err(storage_error)
        })
    }

    pub fn load_running_conversation_summaries(
        &self,
    ) -> Result<Vec<crate::storage::models::RunningConversationSummary>, String> {
        self.with_conversation_read_snapshot(|connection| {
            chat_repository::list_running_conversation_summaries(connection).map_err(storage_error)
        })
    }

    pub fn load_conversation(
        &self,
        conversation_id: &str,
    ) -> Result<Option<ChatConversationRecord>, String> {
        self.load_conversation_with_projection_evidence(conversation_id)
            .map(|(conversation, _)| conversation)
    }

    fn load_conversation_with_projection_evidence(
        &self,
        conversation_id: &str,
    ) -> Result<
        (
            Option<ChatConversationRecord>,
            ConversationProjectionEvidence,
        ),
        String,
    > {
        let mut pooled_connection = self.state.read_connection()?;
        let connection = pooled_connection.transaction().map_err(storage_error)?;
        let reading = crate::performance::Span::new("storage.read_decode", "conversation_messages");
        let mut conversation =
            chat_repository::get_active_conversation(&connection, conversation_id)
                .map_err(storage_error)?;
        drop(reading);
        let (preview_attachments, evidence) = if let Some(conversation) = &mut conversation {
            let preview_attachments =
                self.attach_message_attachments(&connection, std::slice::from_mut(conversation))?;
            let evidence =
                attach_message_guidance_timelines(&connection, std::slice::from_mut(conversation))?;
            (preview_attachments, evidence)
        } else {
            (Vec::new(), ConversationProjectionEvidence::default())
        };
        connection.commit().map_err(storage_error)?;
        drop(pooled_connection);
        if let Some(conversation) = &mut conversation {
            self.hydrate_message_attachment_previews(
                std::slice::from_mut(conversation),
                preview_attachments,
            );
        }
        Ok((conversation, evidence))
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
        self.load_conversation_observer_snapshot_page(conversation_id, None, None)
    }

    pub fn load_conversation_observer_snapshot_page(
        &self,
        conversation_id: &str,
        message_limit: Option<u32>,
        before_message_id: Option<&str>,
    ) -> Result<Option<ConversationObserverSnapshot>, String> {
        if message_limit.is_some_and(|limit| !(1..=100).contains(&limit))
            || before_message_id.is_some_and(|id| message_limit.is_none() || id.trim().is_empty())
        {
            return Err("Invalid observer history page request.".into());
        }
        let mut connection = self.state.read_connection()?;
        let transaction = connection.transaction().map_err(storage_error)?;
        let (mut conversation, has_more) = match message_limit {
            Some(limit) => chat_repository::get_active_conversation_page(
                &transaction,
                conversation_id,
                limit,
                before_message_id,
            ),
            None => chat_repository::get_active_conversation(&transaction, conversation_id)
                .map(|conversation| (conversation, false)),
        }
        .map_err(storage_error)?;
        let Some(mut conversation) = conversation.take() else {
            transaction.commit().map_err(storage_error)?;
            return Ok(None);
        };
        let selected_messages_only = message_limit.is_some();
        let ids = selected_messages_only.then(|| {
            serde_json::to_string(
                &conversation
                    .messages
                    .iter()
                    .map(|message| &message.id)
                    .collect::<Vec<_>>(),
            )
            .expect("message IDs are serializable")
        });
        let history = message_limit.map(|_| ConversationObserverHistoryPage {
            has_more,
            before_message_id: has_more.then(|| {
                conversation
                    .messages
                    .first()
                    .expect("nonempty page has older rows")
                    .id
                    .clone()
            }),
        });
        let preview_attachments = self.attach_message_attachments_in_scope(
            &transaction,
            std::slice::from_mut(&mut conversation),
            selected_messages_only,
        )?;
        attach_message_guidance_timelines_in_scope(
            &transaction,
            std::slice::from_mut(&mut conversation),
            selected_messages_only,
        )?;
        let mut input_origins = agent_graph_repository::conversation_message_origins_in_scope(
            &transaction,
            conversation_id,
            ids.as_deref(),
        )
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
            history,
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
        let mut connection = self.state.read_connection()?;
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
        self.with_conversation_read_snapshot(|connection| {
            connection
                .query_row(
                    "SELECT revision FROM conversations WHERE id = ?1",
                    [conversation_id],
                    |row| row.get::<_, i64>(0),
                )
                .optional()
                .map_err(storage_error)
        })
    }

    pub fn load_conversation_view(
        &self,
        conversation_id: &str,
    ) -> Result<Option<ChatConversationViewRecord>, String> {
        let Some((mut conversation, previews, continuation_origin)) = self
            .with_conversation_read_snapshot(|connection| {
                let Some(mut conversation) =
                    chat_repository::get_active_conversation(connection, conversation_id)
                        .map_err(storage_error)?
                else {
                    return Ok(None);
                };
                let previews = self.attach_message_attachments(
                    connection,
                    std::slice::from_mut(&mut conversation),
                )?;
                let evidence = attach_message_guidance_timelines(
                    connection,
                    std::slice::from_mut(&mut conversation),
                )?;
                chat_repository::retain_user_facing_root_messages(connection, &mut conversation)
                    .map_err(storage_error)?;
                evidence.retain_workflow_top_level_messages(&mut conversation);
                let continuation_origin = conversation_fork_repository::get_continuation_origin(
                    connection,
                    conversation_id,
                )
                .map_err(storage_error)?;
                Ok(Some((conversation, previews, continuation_origin)))
            })?
        else {
            return Ok(None);
        };
        self.hydrate_message_attachment_previews(std::slice::from_mut(&mut conversation), previews);
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
        let mut pooled_connection = self.state.read_connection()?;
        let connection = pooled_connection.transaction().map_err(storage_error)?;
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
            connection.commit().map_err(storage_error)?;
            drop(pooled_connection);
            self.hydrate_message_attachment_previews(
                std::slice::from_mut(&mut conversation),
                preview_attachments,
            );
            return Ok(conversation);
        }

        connection.commit().map_err(storage_error)?;
        drop(pooled_connection);
        self.state.write_sync(
            request_id
                .len()
                .saturating_add(source_conversation_id.len())
                .saturating_add(512),
            crate::storage::StorageWriteClass::Normal,
            |connection| -> Result<(), conversation_fork_repository::ConversationForkError> {
                let transaction = connection
                    .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
                    .map_err(storage_error)?;
                conversation_fork_repository::recover_fork_source_continuations(
                    &transaction,
                    &source_conversation_id,
                    &point,
                )?;
                transaction.commit().map_err(storage_error)?;
                Ok(())
            },
        )??;
        let mut pooled_connection = self.state.read_connection()?;
        let transaction = pooled_connection.transaction().map_err(storage_error)?;
        let mut plan = conversation_fork_repository::build_fork_plan_at_point_readonly(
            &transaction,
            &request_id,
            &source_conversation_id,
            &point,
            now_ms(),
        )?;
        ensure_project_reference_exists(&transaction, plan.target.project_id.as_deref())?;
        let source_versions = fork_staging::ForkSourceVersions::capture(self, &transaction, &plan)?;
        transaction.commit().map_err(storage_error)?;
        // Neither the database connection nor a read snapshot survives cryptography/file I/O.
        drop(pooled_connection);
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

        let mut files = fork_staging::ForkFiles::prepare(&self.attachment_root, &mut plan)?;
        let estimated_bytes = plan.estimated_bytes()?.saturating_add(
            provider_continuations
                .iter()
                .map(|clone| clone.record.ciphertext.len())
                .sum::<usize>(),
        );
        let target_id = self.state.write_sync(
            estimated_bytes,
            crate::storage::StorageWriteClass::Normal,
            |connection| -> Result<String, conversation_fork_repository::ConversationForkError> {
                let target_id = match conversation_fork_repository::commit_prepared_fork_plan(
                    connection,
                    &plan,
                    &provider_continuations,
                    |transaction| source_versions.validate(self, transaction, &plan),
                ) {
                    Ok(target_id) => target_id,
                    Err(error) => {
                        // A failed COMMIT/rollback or an unreadable database cannot prove that no
                        // durable reference exists. Preserve published bytes in that case; report
                        // the original failure rather than inventing a successful receipt.
                        let unreferenced = connection.is_autocommit()
                            && connection
                                .query_row(
                                    "SELECT NOT EXISTS(SELECT 1 FROM conversations WHERE id = ?1)",
                                    [&plan.target.id],
                                    |row| row.get::<_, bool>(0),
                                )
                                .unwrap_or(false);
                        if !unreferenced {
                            files.mark_committed();
                        }
                        return Err(error);
                    }
                };
                if target_id == plan.target.id {
                    // This must precede fallible read-back: committed rows already own these files.
                    files.mark_committed();
                }
                Ok(target_id)
            },
        )??;
        let mut pooled_connection = self.state.read_connection()?;
        let connection = pooled_connection.transaction().map_err(storage_error)?;
        let mut conversation = chat_repository::get_conversation(&connection, &target_id)
            .map_err(storage_error)?
            .ok_or_else(|| "新任务创建后无法重新读取。".to_string())?;
        let preview_attachments =
            self.attach_message_attachments(&connection, std::slice::from_mut(&mut conversation))?;
        attach_message_guidance_timelines(&connection, std::slice::from_mut(&mut conversation))?;
        connection.commit().map_err(storage_error)?;
        drop(pooled_connection);
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
        let mut pooled_connection = self.state.read_connection()?;
        let connection = pooled_connection.transaction().map_err(storage_error)?;
        chat_repository::retain_user_facing_root_messages(&connection, &mut conversation)
            .map_err(storage_error)?;
        let evidence =
            attach_message_guidance_timelines(&connection, std::slice::from_mut(&mut conversation))
                .map_err(conversation_fork_repository::ConversationForkError::Other)?;
        evidence.retain_workflow_top_level_messages(&mut conversation);
        let continuation_origin =
            conversation_fork_repository::get_continuation_origin(&connection, &conversation.id)
                .map_err(storage_error)?;
        connection.commit().map_err(storage_error)?;
        Ok(ChatConversationViewRecord {
            conversation,
            continuation_origin,
        })
    }

    pub fn search_chats(&self, input: &ChatSearchInput) -> Result<Vec<ChatSearchResult>, String> {
        self.with_conversation_read_snapshot(|connection| {
            chat_search_repository::search_chats(connection, input).map_err(storage_error)
        })
    }

    pub fn project_agent_messages_for_model(
        &self,
        conversation_id: &str,
        messages: &mut Vec<crate::AgentChatMessage>,
    ) -> Result<(), String> {
        self.with_conversation_read_snapshot(|connection| {
            let workflow_inputs =
                crate::storage::workflow_execution_repository::delivery_origins_for_conversation(
                    connection,
                    conversation_id,
                )?;
            let workflow_message_ids: std::collections::HashSet<_> = workflow_inputs
                .iter()
                .map(|(message_id, _)| message_id.as_str())
                .collect();
            // These chat bubbles are a UI projection. Their non-human model input is the
            // durable WorkflowDelivery trace, or the workflow inbox before first sampling.
            messages.retain(|message| {
                !message
                    .message_id
                    .as_deref()
                    .is_some_and(|id| workflow_message_ids.contains(id))
            });
            for message in messages.iter_mut().filter(|message| message.role == "user") {
                let Some(message_id) = message.message_id.as_deref() else {
                    continue;
                };
                if let Some(content) =
                    crate::storage::agent_message_model_projection::project_message(
                        connection,
                        conversation_id,
                        message_id,
                    )?
                {
                    message.content = content;
                }
            }
            Ok(())
        })
    }

    pub fn project_history_conversation_for_model(
        &self,
        conversation: &mut ChatConversationRecord,
    ) -> Result<(), String> {
        self.with_conversation_read_snapshot(|connection| {
            let workflow_inputs =
                crate::storage::workflow_execution_repository::delivery_origins_for_conversation(
                    connection,
                    &conversation.id,
                )?;
            let workflow_message_ids: std::collections::HashSet<_> = workflow_inputs
                .iter()
                .map(|(message_id, _)| message_id.as_str())
                .collect();
            conversation
                .messages
                .retain(|message| !workflow_message_ids.contains(message.id.as_str()));
            for message in conversation
                .messages
                .iter_mut()
                .filter(|message| message.role == "user")
            {
                if let Some(content) =
                    crate::storage::agent_message_model_projection::project_message(
                        connection,
                        &conversation.id,
                        &message.id,
                    )?
                {
                    message.content = content;
                }
            }
            Ok(())
        })
    }

    pub(crate) fn read_compacted_conversation_history(
        &self,
        conversation_id: &str,
        summary_id: &str,
    ) -> Result<conversation_history_repository::CompactedHistorySnapshot, String> {
        self.with_conversation_read_snapshot(|connection| {
            conversation_history_repository::read_compacted_snapshot(
                connection,
                conversation_id,
                summary_id,
            )
        })
    }

    pub fn search_conversation_history(
        &self,
        conversation_id: &str,
        query: &str,
        filter: &conversation_history_repository::ConversationHistorySearchFilter,
        limit: usize,
    ) -> Result<Vec<conversation_history_repository::ConversationHistorySearchHit>, String> {
        self.with_conversation_read_snapshot(|connection| {
            let mut hits = conversation_history_repository::search_records(
                connection,
                conversation_id,
                query,
                filter,
                limit,
            )
            .map_err(storage_error)?;
            for hit in &mut hits {
                crate::storage::agent_message_model_projection::project_history_preview(
                    connection,
                    conversation_id,
                    &hit.reference,
                    &mut hit.preview,
                    &mut hit.preview_truncated,
                )?;
            }
            Ok(hits)
        })
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
        self.with_conversation_read_snapshot(|connection| {
            let mut records = conversation_history_repository::records_around(
                connection,
                conversation_id,
                reference,
                before,
                after,
            )
            .map_err(storage_error)?;
            for record in records.iter_mut().flatten() {
                crate::storage::agent_message_model_projection::project_history_preview(
                    connection,
                    conversation_id,
                    &record.reference,
                    &mut record.preview,
                    &mut record.preview_truncated,
                )?;
            }
            Ok(records)
        })
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
        self.with_conversation_read_snapshot(|connection| {
            let mut records = conversation_history_repository::records_in_range(
                connection,
                conversation_id,
                start,
                end,
                limit,
            )
            .map_err(storage_error)?;
            for record in records.iter_mut().flatten() {
                crate::storage::agent_message_model_projection::project_history_preview(
                    connection,
                    conversation_id,
                    &record.reference,
                    &mut record.preview,
                    &mut record.preview_truncated,
                )?;
            }
            Ok(records)
        })
    }

    pub fn conversation_history_tool_exchange(
        &self,
        conversation_id: &str,
        reference: Option<&conversation_history_repository::ConversationHistoryRecordRef>,
        call_id: Option<&str>,
        run_id: Option<&str>,
    ) -> Result<Option<Vec<conversation_history_repository::ConversationHistoryRecord>>, String>
    {
        self.with_conversation_read_snapshot(|connection| {
            conversation_history_repository::get_tool_exchange(
                connection,
                conversation_id,
                reference,
                call_id,
                run_id,
            )
            .map_err(storage_error)
        })
    }

    pub fn read_conversation_history_record(
        &self,
        conversation_id: &str,
        reference: &conversation_history_repository::ConversationHistoryRecordRef,
    ) -> Result<Option<conversation_history_repository::ConversationHistoryRecord>, String> {
        self.with_conversation_read_snapshot(|connection| {
            let mut record = conversation_history_repository::read_record(
                connection,
                conversation_id,
                reference,
            )
            .map_err(storage_error)?;
            if let Some(record) = record.as_mut() {
                crate::storage::agent_message_model_projection::project_history_record(
                    connection,
                    conversation_id,
                    record,
                )?;
            }
            Ok(record)
        })
    }

    pub fn archive_conversation_tool_result(
        &self,
        input: conversation_history_archive_repository::ConversationHistoryArchiveInput,
    ) -> Result<conversation_history_archive_repository::ConversationHistoryArchiveDescriptor, String>
    {
        let prepared = conversation_history_archive_repository::prepare_archive(
            &input,
            crate::storage::execution::WRITE_BYTES,
        )
        .map_err(storage_error)?;
        self.state.write_sync(
            prepared.estimated_bytes(),
            crate::storage::StorageWriteClass::Normal,
            |connection| {
                conversation_history_archive_repository::store_prepared_archive(
                    connection, &prepared,
                )
                .map_err(storage_error)
            },
        )?
    }

    pub fn delete_unreferenced_exact_conversation_tool_result(
        &self,
        input: conversation_history_archive_repository::ConversationHistoryArchiveInput,
    ) -> Result<bool, String> {
        let bytes = [
            &input.conversation_id,
            &input.assistant_message_id,
            &input.call_id,
            &input.tool,
            &input.content_type,
            &input.content,
        ]
        .into_iter()
        .fold(256usize, |total, value| total.saturating_add(value.len()));
        self.state.write_sync(
            bytes,
            crate::storage::StorageWriteClass::Normal,
            |connection| {
                conversation_history_archive_repository::delete_unreferenced_exact_archive(
                    connection, &input,
                )
                .map_err(storage_error)
            },
        )?
    }

    pub fn archive_conversation_tool_result_file(
        &self,
        input: conversation_history_archive_repository::ConversationHistoryArchiveFileInput,
    ) -> Result<conversation_history_archive_repository::ConversationHistoryArchiveDescriptor, String>
    {
        let prepared = conversation_history_archive_repository::prepare_archive_file(
            &input,
            crate::storage::execution::WRITE_BYTES,
        )
        .map_err(storage_error)?;
        self.state.write_sync(
            prepared.estimated_bytes(),
            crate::storage::StorageWriteClass::Normal,
            |connection| {
                conversation_history_archive_repository::store_prepared_archive(
                    connection, &prepared,
                )
                .map_err(storage_error)
            },
        )?
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
        self.with_conversation_read_snapshot(|connection| {
            if conversation_turn_rewrite_repository::superseded_message_ids(
                connection,
                conversation_id,
            )
            .map_err(storage_error)?
            .contains(assistant_message_id)
            {
                return Ok(None);
            }
            conversation_history_archive_repository::find_archive_for_trace_item(
                connection,
                conversation_id,
                assistant_message_id,
                sequence,
            )
            .map_err(storage_error)
        })
    }

    pub fn find_conversation_history_archive_by_ref(
        &self,
        conversation_id: &str,
        archive_ref: &str,
    ) -> Result<
        Option<conversation_history_archive_repository::ConversationHistoryArchiveDescriptor>,
        String,
    > {
        self.with_conversation_read_snapshot(|connection| {
            let archive = conversation_history_archive_repository::find_archive_by_ref(
                connection,
                conversation_id,
                archive_ref,
            )
            .map_err(storage_error)?;
            let Some(archive) = archive else {
                return Ok(None);
            };
            if conversation_turn_rewrite_repository::superseded_message_ids(
                connection,
                conversation_id,
            )
            .map_err(storage_error)?
            .contains(&archive.assistant_message_id)
            {
                return Ok(None);
            }
            Ok(Some(archive))
        })
    }

    pub fn find_conversation_history_archive_match_char_offset(
        &self,
        conversation_id: &str,
        archive_ref: &str,
        query: &str,
    ) -> Result<Option<u64>, String> {
        self.with_conversation_read_snapshot(|connection| {
            let Some(archive) = conversation_history_archive_repository::find_archive_by_ref(
                connection,
                conversation_id,
                archive_ref,
            )
            .map_err(storage_error)?
            else {
                return Ok(None);
            };
            if conversation_turn_rewrite_repository::superseded_message_ids(
                connection,
                conversation_id,
            )
            .map_err(storage_error)?
            .contains(&archive.assistant_message_id)
            {
                return Ok(None);
            }
            conversation_history_archive_repository::find_archive_match_char_offset(
                connection,
                conversation_id,
                archive_ref,
                query,
            )
            .map_err(storage_error)
        })
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
        self.with_conversation_read_snapshot(|connection| {
            let Some(archive) = conversation_history_archive_repository::find_archive_by_ref(
                connection,
                conversation_id,
                archive_ref,
            )
            .map_err(storage_error)?
            else {
                return Ok(None);
            };
            if conversation_turn_rewrite_repository::superseded_message_ids(
                connection,
                conversation_id,
            )
            .map_err(storage_error)?
            .contains(&archive.assistant_message_id)
            {
                return Ok(None);
            }
            conversation_history_archive_repository::read_archive_page(
                connection,
                conversation_id,
                archive_ref,
                unit,
                start,
                maximum,
            )
            .map_err(storage_error)
        })
    }

    /// Reads a Host-internal Archive audit route within the caller's conversation boundary.
    /// This storage helper remains available for verification; the model-facing history tool
    /// does not accept Archive routes or continue truncated tool output.
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

    /// Creates a Host-internal Archive audit route after verifying that the immutable Archive
    /// belongs to the caller's conversation. This is not a model-facing history capability.
    ///
    /// Host integrations use this narrow method to verify an archived result without exposing
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
