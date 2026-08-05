use super::*;

impl AgentService {
    pub fn get_context_window_snapshot(
        &self,
        input: AgentContextWindowSnapshotInput,
    ) -> Result<AgentContextWindowSnapshotOutput, AgentServiceError> {
        let model_id = input.model_id.trim();
        if model_id.is_empty() {
            return Err("modelId 不能为空。".to_string().into());
        }
        let settings = self
            .storage
            .load_model_settings()?
            .ok_or_else(|| "请先配置模型。".to_string())?;
        let model = settings
            .models
            .iter()
            .find(|model| model.id == model_id)
            .cloned()
            .ok_or_else(|| format!("未找到模型配置：{model_id}"))?;
        if !model.enabled {
            return Err(format!("模型未启用：{model_id}").into());
        }
        let context_window_tokens = model.effective_context_window_tokens();
        let connection = settings.effective_connection_for(&model)?;

        let conversation_id = normalized_optional(input.conversation_id.as_deref());
        let conversation = match conversation_id.as_deref() {
            Some(conversation_id) => self.storage.load_conversation(conversation_id)?,
            None => None,
        };
        let conversation_started = conversation.as_ref().is_some_and(|conversation| {
            conversation.messages.iter().any(|message| {
                message.role.trim().eq_ignore_ascii_case("user")
                    && (!message.content.trim().is_empty() || !message.attachments.is_empty())
            })
        });
        let project_id = resolve_conversation_project_id(
            conversation.as_ref(),
            normalized_optional(input.project_id.as_deref()),
        )?;
        let project = resolve_project(&self.storage, project_id.as_deref())?;
        let workspace_root = project
            .as_ref()
            .and_then(|project| project.path.as_deref())
            .map(std::path::PathBuf::from);
        let workspace = project
            .as_ref()
            .zip(workspace_root.as_deref())
            .map(|(project, root)| (project.id.as_str(), root));
        let prepared_skills =
            activate_selected_skills(&self.storage, &self.skills, workspace, &input.skills)?;
        let skill_discovery =
            prepare_enabled_skill_discovery(&self.storage, &self.skills, context_window_tokens)?;
        let attachment_library = conversation_id
            .as_deref()
            .map(|conversation_id| {
                self.storage
                    .build_attachment_library_context(conversation_id, project_id.as_deref())
            })
            .transpose()?;
        let context_compaction_summary = match conversation_id.as_deref() {
            Some(conversation_id) => self
                .storage
                .get_active_context_compaction_summary(conversation_id)?,
            None => None,
        };
        let world_state_records = match conversation_id.as_deref() {
            Some(conversation_id) => load_conversation_world_state(&self.storage, conversation_id)?,
            None => Vec::new(),
        };
        let messages = match conversation.as_ref() {
            Some(conversation) => {
                let traces = self
                    .storage
                    .list_conversation_turn_traces(&conversation.id)?;
                let model_context_logs = self
                    .storage
                    .list_conversation_model_context_logs(&conversation.id)?;
                conversation_history_messages_with_model_context(
                    conversation,
                    &traces,
                    &model_context_logs,
                    context_compaction_summary.as_ref(),
                    &[],
                )
            }
            None => Vec::new(),
        };
        let prompt_preferences = match input.prompt_preferences {
            Some(preferences) => preferences,
            None => {
                agent_prompt_preferences_from_record(self.storage.load_agent_prompt_preferences()?)
            }
        };
        let goal = conversation_id
            .as_deref()
            .map(|conversation_id| self.storage.load_visible_conversation_goal(conversation_id))
            .transpose()?
            .flatten();
        let agent_input = AgentChatInput {
            api_url: connection.api_url,
            api_token: String::new(),
            provider_configuration_revision: None,
            model: model.id.clone(),
            model_capabilities: ModelCapabilities {
                image_input: model.supports_image,
            },
            api_style: None,
            context_window_tokens: Some(context_window_tokens),
            context_window_indicator_enabled: true,
            max_tokens: input.max_tokens,
            temperature: None,
            stream: Some(false),
            context: Some(AgentRunContext {
                conversation_id: conversation_id.clone(),
                project_id: project_id.clone(),
                workspace: project
                    .as_ref()
                    .map(|project| mycopilot_core::AgentWorkspaceContext {
                        project_id: Some(project.id.clone()),
                        display_name: Some(project.name.clone()),
                        root_path: project.path.clone(),
                    }),
                attachment_library,
                permissions: input.permissions,
            }),
            search_config: Some(AgentSearchConfig {
                mode: search_mode_from_storage(&settings.search_mode),
                tavily_api_key: non_empty(settings.tavily_api_key),
            }),
            prompt_preferences: Some(prompt_preferences),
            approval_decision: None,
            tool_continuation: None,
            attachments: Vec::new(),
            resume_checkpoint: None,
            assistant_message_id: None,
            context_compaction_summary,
            goal,
            world_state_records,
            skill_activation: prepared_skills.runtime,
            skill_discovery,
            messages,
        };
        let tool_projection = self.context_window_tool_projection(
            &agent_input,
            prepared_skills.resources.as_ref().map(Arc::clone),
        )?;

        let mut snapshot = match conversation_id.as_deref() {
            Some(conversation_id) => self.context_window_snapshot_with_projection_cache(
                &agent_input,
                conversation_id,
                &tool_projection,
            )?,
            None => inspect_context_window_with_tool_projection(agent_input, &tool_projection)
                .map_err(|error| error.to_string())?,
        };
        if !conversation_started {
            if let Some(snapshot) = snapshot.as_mut() {
                snapshot.input_tokens = 0;
                snapshot.cost_breakdown = Default::default();
                snapshot.remaining_input_tokens = snapshot
                    .input_capacity_tokens
                    .map(|capacity| i64::try_from(capacity).unwrap_or(i64::MAX));
            }
        }
        Ok(AgentContextWindowSnapshotOutput { snapshot })
    }

    pub(super) fn context_window_tool_projection(
        &self,
        agent_input: &AgentChatInput,
        skill_resources: Option<Arc<mycopilot_core::skills::SkillResourceSession>>,
    ) -> Result<AgentContextWindowToolProjection, String> {
        let mcp_tools = self.capture_mcp_tool_runtime(agent_input);
        self.context_window_tool_projection_with_mcp(agent_input, skill_resources, mcp_tools)
    }

    pub(super) fn context_window_tool_projection_with_mcp(
        &self,
        agent_input: &AgentChatInput,
        skill_resources: Option<Arc<mycopilot_core::skills::SkillResourceSession>>,
        mcp_tools: Option<McpToolRuntime>,
    ) -> Result<AgentContextWindowToolProjection, String> {
        let mut host_services =
            AgentRuntimeHostServices::new().with_office_engine(Arc::clone(&self.office_engine));
        if let Some(execution) = self.image_generation_execution.as_ref() {
            host_services = host_services.with_image_generation_execution(Arc::clone(execution));
        }
        if let Some(skill_installation_prepare) = self.skill_installation_prepare.as_ref() {
            host_services = host_services
                .with_skill_installation_prepare(Arc::clone(skill_installation_prepare));
        }
        if let Some(skill_installation) = self.skill_installation.as_ref() {
            let commit: Arc<dyn mycopilot_core::AgentSkillInstallationCommitPreparer> =
                Arc::clone(skill_installation) as Arc<_>;
            host_services = host_services.with_skill_installation_commit(commit);
        }
        if let Some(resources) = skill_resources {
            host_services = host_services.with_skill_resources(resources);
        }
        if let Some(mcp_tools) = mcp_tools {
            host_services = host_services.with_mcp_tools(mcp_tools);
        }
        // AgentService always provides the real Host action executor to a started run. Passing
        // `true` keeps preview approval schemas aligned with that production boundary without
        // constructing an executable action closure during a read-only capacity inspection.
        prepare_context_window_tool_projection(agent_input, &host_services, true)
            .map_err(|error| error.to_string())
    }

    /// Accepts the runtime's aggregate accounting for the final assembled request attempt.
    /// Sendable and terminal over-capacity attempts share this authority; intermediate compaction
    /// candidates are never published.
    /// The observer is scoped by the Host closure, so neither Skill instructions nor Tool schemas
    /// need to cross this boundary. Notification delivery is deliberately best-effort and can
    /// never abort or replay a model request.
    pub(super) fn context_window_observer(
        &self,
        run_id: &str,
        conversation_id: &str,
        expected_model: &str,
        notifications: CoreServerNotificationSender,
    ) -> AgentContextWindowObserver {
        let service = self.clone();
        let run_id = run_id.to_string();
        let conversation_id = conversation_id.to_string();
        let expected_model = expected_model.to_string();
        Arc::new(move |snapshot| {
            if snapshot.model != expected_model {
                eprintln!(
                    "ignored context-window snapshot for unexpected model `{}` (expected `{expected_model}`)",
                    snapshot.model
                );
                return;
            }
            service
                .running_context_window_snapshots
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .insert(run_id.clone(), snapshot.clone());
            service.emit_context_window_snapshot(
                &notifications,
                &run_id,
                &conversation_id,
                Some(snapshot),
            );
        })
    }

    pub(super) fn has_exact_running_context_window_snapshot(&self, run_id: &str) -> bool {
        self.running_context_window_snapshots
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .contains_key(run_id)
    }

    pub(super) fn discard_exact_running_context_window_snapshot(&self, run_id: &str) {
        self.running_context_window_snapshots
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .remove(run_id);
    }

    pub(super) fn persisted_conversation_context_state(
        &self,
        agent_input: &AgentChatInput,
        conversation_id: &str,
    ) -> Result<(AgentChatInput, Vec<ConversationTurnTrace>), String> {
        let conversation = self
            .storage
            .load_conversation(conversation_id)?
            .ok_or_else(|| format!("未找到对话：{conversation_id}"))?;
        let mut preview_input = agent_input.clone();
        let traces = self
            .storage
            .list_conversation_turn_traces(conversation_id)?;
        let model_context_logs = self
            .storage
            .list_conversation_model_context_logs(conversation_id)?;
        let context_compaction_summary = self
            .storage
            .get_active_context_compaction_summary(conversation_id)?;
        preview_input.messages = conversation_history_messages_with_model_context(
            &conversation,
            &traces,
            &model_context_logs,
            context_compaction_summary.as_ref(),
            &[],
        );
        preview_input.context_compaction_summary = context_compaction_summary;
        preview_input.world_state_records =
            load_conversation_world_state(&self.storage, conversation_id)?;
        preview_input.attachments.clear();
        preview_input.approval_decision = None;
        preview_input.tool_continuation = None;
        preview_input.resume_checkpoint = None;
        if let Some(context) = preview_input.context.as_mut() {
            context.conversation_id = Some(conversation_id.to_string());
            context.attachment_library = Some(self.storage.build_attachment_library_context(
                conversation_id,
                context.project_id.as_deref(),
            )?);
        }
        Ok((preview_input, traces))
    }

    pub(super) fn context_window_snapshot_with_projection_cache(
        &self,
        agent_input: &AgentChatInput,
        conversation_id: &str,
        tool_projection: &AgentContextWindowToolProjection,
    ) -> Result<Option<AgentContextWindowSnapshot>, String> {
        if !agent_input.context_window_indicator_enabled {
            return Ok(None);
        }
        let configuration_revision = conversation_context_configuration_revision(agent_input)
            .map_err(|error| error.to_string())?;
        let access = self.next_conversation_context_state_access();
        {
            let mut states = self
                .conversation_context_states
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            if let Some(entry) = states.get_mut(conversation_id) {
                if entry.configuration_revision == configuration_revision {
                    entry.last_access = access;
                    return entry
                        .state
                        .snapshot_with_skill_overlays_and_tool_projection(
                            agent_input.skill_discovery.as_ref(),
                            agent_input.skill_activation.as_ref(),
                            tool_projection,
                        )
                        .map(Some)
                        .map_err(|error| error.to_string());
                }
                states.remove(conversation_id);
            }
        }
        self.rebuild_conversation_context_state(
            agent_input,
            conversation_id,
            None,
            agent_input.skill_activation.as_ref(),
            Some(tool_projection),
        )
        .map(|update| update.snapshot)
    }

    pub(super) fn rebuild_conversation_context_state(
        &self,
        agent_input: &AgentChatInput,
        conversation_id: &str,
        active_run_id: Option<&str>,
        snapshot_skill_activation: Option<&mycopilot_core::AgentSkillActivation>,
        tool_projection: Option<&AgentContextWindowToolProjection>,
    ) -> Result<ConversationContextStateUpdate, String> {
        let (preview_input, traces) =
            self.persisted_conversation_context_state(agent_input, conversation_id)?;
        let latest_trace = traces.last();
        let latest_committed_activity_items = latest_trace
            .map(|trace| {
                let model_context_items = preview_input
                    .messages
                    .iter()
                    .find(|message| {
                        message.message_id.as_deref() == Some(trace.assistant_message_id.as_str())
                    })
                    .map(|message| message.conversation_model_context_items.as_slice())
                    .unwrap_or_default();
                AgentConversationContextState::rendered_trace_activity_count(
                    trace,
                    model_context_items,
                )
                .map_err(|error| error.to_string())
            })
            .transpose()?
            .unwrap_or_default();
        let mut state =
            create_conversation_context_state(preview_input).map_err(|error| error.to_string())?;
        let baseline = state.shared_baseline().map_err(|error| error.to_string())?;
        let snapshot = if agent_input.context_window_indicator_enabled {
            Some(match tool_projection {
                Some(tool_projection) => state
                    .snapshot_with_skill_overlays_and_tool_projection(
                        agent_input.skill_discovery.as_ref(),
                        snapshot_skill_activation,
                        tool_projection,
                    )
                    .map_err(|error| error.to_string())?,
                None => state
                    .snapshot_with_skill_overlays(
                        agent_input.skill_discovery.as_ref(),
                        snapshot_skill_activation,
                    )
                    .map_err(|error| error.to_string())?,
            })
        } else {
            None
        };
        let entry = ConversationContextStateEntry {
            configuration_revision: state.configuration_revision().to_string(),
            state,
            active_run_id: latest_trace
                .filter(|trace| !trace.terminal_status.is_terminal())
                .and(active_run_id)
                .map(ToString::to_string),
            active_assistant_message_id: latest_trace
                .map(|trace| trace.assistant_message_id.clone()),
            committed_activity_items: latest_committed_activity_items,
            terminal: latest_trace.is_none_or(|trace| trace.terminal_status.is_terminal()),
            last_access: self.next_conversation_context_state_access(),
        };
        self.insert_conversation_context_state(conversation_id, entry);
        Ok(ConversationContextStateUpdate { baseline, snapshot })
    }

    pub(super) fn finalize_conversation_context_state(
        &self,
        agent_input: &AgentChatInput,
        run_id: &str,
        conversation_id: &str,
        assistant_message_id: &str,
        assistant_content: &str,
    ) -> Result<Option<AgentContextWindowSnapshot>, String> {
        let trace = self
            .storage
            .get_conversation_turn_trace(assistant_message_id)?
            .ok_or_else(|| format!("assistant 终态缺少会话轨迹：{assistant_message_id}"))?;
        let model_context_items = self
            .storage
            .get_conversation_model_context_log(assistant_message_id)?
            .map(|log| log.items)
            .unwrap_or_default();
        let assistant_created_at = self
            .storage
            .get_assistant_message_created_at(conversation_id, assistant_message_id)?
            .ok_or_else(|| format!("assistant 终态缺少消息创建时间：{assistant_message_id}"))?;
        let configuration_revision = conversation_context_configuration_revision(agent_input)
            .map_err(|error| error.to_string())?;
        let access = self.next_conversation_context_state_access();
        let mut needs_rebuild = false;
        {
            let mut states = self
                .conversation_context_states
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            if let Some(entry) = states.get_mut(conversation_id) {
                if entry.configuration_revision == configuration_revision
                    && entry.active_run_id.as_deref() == Some(run_id)
                    && entry.active_assistant_message_id.as_deref() == Some(assistant_message_id)
                {
                    if !entry.terminal {
                        match entry.state.finalize_conversation_turn(
                            &trace,
                            &model_context_items,
                            entry.committed_activity_items,
                            assistant_content,
                            Some(assistant_created_at),
                        ) {
                            Ok(committed_activity_items) => {
                                entry.committed_activity_items = committed_activity_items;
                                entry.terminal = true;
                                entry.active_run_id = None;
                            }
                            Err(_) => needs_rebuild = true,
                        }
                    }
                    if !needs_rebuild {
                        entry.last_access = access;
                        entry
                            .state
                            .shared_baseline()
                            .map_err(|error| error.to_string())?;
                        return Ok(agent_input
                            .context_window_indicator_enabled
                            .then(|| entry.state.snapshot()));
                    }
                } else {
                    needs_rebuild = true;
                }
            } else {
                needs_rebuild = true;
            }
            if needs_rebuild {
                states.remove(conversation_id);
            }
        }
        self.rebuild_conversation_context_state(
            agent_input,
            conversation_id,
            Some(run_id),
            None,
            None,
        )
        .map(|update| update.snapshot)
    }

    pub(super) fn emit_terminal_context_window_snapshot(
        &self,
        notifications: &CoreServerNotificationSender,
        agent_input: &AgentChatInput,
        run_id: &str,
        conversation_id: &str,
        assistant_message_id: &str,
        assistant_content: &str,
    ) {
        let snapshot = match self.finalize_conversation_context_state(
            agent_input,
            run_id,
            conversation_id,
            assistant_message_id,
            assistant_content,
        ) {
            Ok(snapshot) => snapshot,
            Err(error) => {
                eprintln!(
                    "failed to refresh context window snapshot for conversation {conversation_id}: {error}"
                );
                return;
            }
        };
        // A terminal snapshot measures the newly committed durable baseline without run overlays.
        // Retire the last pre-request exact snapshot before publishing that new authority.
        self.discard_exact_running_context_window_snapshot(run_id);
        self.emit_context_window_snapshot(notifications, run_id, conversation_id, snapshot);
    }

    pub(super) fn emit_derived_context_window_snapshot(
        &self,
        notifications: &CoreServerNotificationSender,
        run_id: &str,
        conversation_id: &str,
        snapshot: Option<AgentContextWindowSnapshot>,
    ) {
        // Once the runtime has published exact request accounting, a trace-derived approximation
        // must never become the last writer. The next sendable request or terminal commit will
        // publish the next authoritative value.
        if self.has_exact_running_context_window_snapshot(run_id) {
            return;
        }
        self.emit_context_window_snapshot(notifications, run_id, conversation_id, snapshot);
    }

    pub(super) fn emit_context_window_snapshot(
        &self,
        notifications: &CoreServerNotificationSender,
        run_id: &str,
        conversation_id: &str,
        snapshot: Option<AgentContextWindowSnapshot>,
    ) {
        let Some(snapshot) = snapshot else {
            return;
        };
        let _ = notifications.send(agent_event_notification(AgentEvent::ContextWindowUpdated {
            run_id: run_id.to_string(),
            conversation_id: Some(conversation_id.to_string()),
            snapshot,
        }));
    }

    pub(super) fn next_conversation_context_state_access(&self) -> u64 {
        self.conversation_context_state_clock
            .fetch_add(1, Ordering::Relaxed)
    }

    pub(super) fn insert_conversation_context_state(
        &self,
        conversation_id: &str,
        entry: ConversationContextStateEntry,
    ) {
        let mut states = self
            .conversation_context_states
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if !states.contains_key(conversation_id)
            && states.len() >= MAX_CONVERSATION_CONTEXT_STATE_CACHE_ENTRIES
        {
            if let Some(oldest) = states
                .iter()
                .min_by_key(|(_, entry)| entry.last_access)
                .map(|(conversation_id, _)| conversation_id.clone())
            {
                states.remove(&oldest);
            }
        }
        states.insert(conversation_id.to_string(), entry);
    }

    /// Compression, message deletion, rollback and any future durable-history rewrite must call
    /// this before the next durable-context read.
    pub fn invalidate_conversation_context_state(&self, conversation_id: &str) {
        self.conversation_context_states
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .remove(conversation_id);
    }

    pub fn invalidate_all_conversation_context_states(&self) {
        self.conversation_context_states
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clear();
    }
}
