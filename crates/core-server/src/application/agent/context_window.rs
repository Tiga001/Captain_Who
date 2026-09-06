use super::*;

fn should_defer_partial_provider_trace(
    semantics: mycopilot_core::ProviderPartialTraceSemantics,
    trace: &ConversationTurnTrace,
) -> bool {
    semantics == mycopilot_core::ProviderPartialTraceSemantics::DeferUntilProviderTurnClosed
        && !trace.terminal_status.is_terminal()
        && trace
            .items
            .iter()
            .any(|item| matches!(item, ConversationTurnTraceItem::ToolCall { .. }))
}

fn resolve_context_window_provider_capabilities(
    provider_protocol_key: Option<&ProviderProtocolKey>,
) -> Result<Option<mycopilot_core::ProviderRuntimeCapabilities>, String> {
    provider_protocol_key
        .map(mycopilot_core::resolve_provider_runtime_capabilities)
        .transpose()
        .map_err(|error| error.to_string())
}

impl AgentService {
    pub fn get_context_window_snapshot(
        &self,
        input: AgentContextWindowSnapshotInput,
    ) -> Result<AgentContextWindowSnapshotOutput, AgentServiceError> {
        let model_id = input.model_id.trim();
        if model_id.is_empty() {
            return Err("modelId 不能为空。".to_string().into());
        }
        let conversation_id = normalized_optional(input.conversation_id.as_deref());
        if let Some(conversation_id) = conversation_id.as_deref() {
            self.authorize_user_conversation_write(conversation_id)?;
        }
        let settings_snapshot = self
            .storage
            .load_model_settings_snapshot_for_model(model_id, false)?
            .ok_or_else(|| "请先配置模型。".to_string())?;
        let settings = settings_snapshot.settings;
        let model = settings
            .models
            .iter()
            .find(|model| model.id == model_id)
            .cloned()
            .ok_or_else(|| "所选模型配置已不存在。".to_string())?;
        let model_label = model.display_label();
        if !model.enabled {
            return Err(format!("模型未启用：{model_label}").into());
        }
        let provider_connection_revision = settings_snapshot
            .provider_connection_revisions
            .get(&model.id)
            .cloned()
            .ok_or_else(|| format!("模型 {model_label} 的 Provider 连接身份缺失。"))?;
        let provider_protocol_revision = settings_snapshot
            .provider_protocol_revisions
            .get(&model.id)
            .cloned()
            .ok_or_else(|| format!("模型 {model_label} 的 Provider Protocol 身份缺失。"))?;
        let context_window_tokens = model.effective_context_window_tokens();
        let connection = settings.effective_connection_for(&model)?;
        let provider_dialect = ProviderProtocolDialect::detect_from_api_url(&connection.api_url);
        let provider_profile_config = model
            .resolved_provider_profile_config(provider_dialect)
            .map_err(|error| format!("模型 {model_label} 的 Provider Profile 无效：{error}"))?;
        let provider_protocol_key = ProviderProtocolKey::new(
            provider_dialect,
            &provider_profile_config,
            model.provider_model_id.clone(),
            Some(provider_protocol_revision.clone()),
        )
        .map_err(|error| format!("模型 {model_label} 的 Provider Protocol 无效：{error}"))?;

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
        let skill_discovery = prepare_enabled_skill_discovery(
            &self.storage,
            &self.skills,
            workspace,
            context_window_tokens,
        )?;
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
                )?
            }
            None => Vec::new(),
        };
        let prompt_preferences = match input.prompt_preferences {
            Some(preferences) => preferences,
            None => {
                agent_prompt_preferences_from_record(self.storage.load_agent_prompt_preferences()?)
            }
        };
        let agent_input = AgentChatInput {
            api_url: connection.api_url,
            api_token: String::new(),
            provider_configuration_revision: Some(provider_protocol_revision),
            provider_connection_revision: Some(provider_connection_revision),
            search_connection_revision: Some(settings_snapshot.search_connection_revision),
            provider_profile_config: Some(provider_profile_config),
            provider_protocol_key: Some(provider_protocol_key),
            model_config_id: Some(model.id.clone()),
            model: model.provider_model_id.clone(),
            model_capabilities: ModelCapabilities {
                image_input: model.supports_image,
            },
            api_style: Some(provider_dialect.api_style()),
            context_window_tokens: Some(context_window_tokens),
            context_window_indicator_enabled: true,
            max_tokens: input.max_tokens,
            temperature: None,
            stream: Some(false),
            context: Some(AgentRunContext {
                collaboration_identity: None,
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
                tavily_api_key: None,
            }),
            prompt_preferences: Some(prompt_preferences),
            approval_decision: None,
            tool_continuation: None,
            attachments: Vec::new(),
            resume_checkpoint: None,
            assistant_message_id: None,
            context_compaction_summary,
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
        Ok(AgentContextWindowSnapshotOutput {
            model_config_id: model.id,
            snapshot,
        })
    }

    pub(super) fn context_window_tool_projection(
        &self,
        agent_input: &AgentChatInput,
        skill_resources: Option<Arc<mycopilot_core::skills::SkillResourceSession>>,
    ) -> Result<AgentContextWindowToolProjection, String> {
        let mcp_tools = self.capture_mcp_tool_runtime(agent_input);
        self.context_window_tool_projection_with_mcp(agent_input, skill_resources, mcp_tools)
    }

    /// Rebuilds a durable in-flight run projection, restoring Automation-only capability from the
    /// run's persisted ownership when appropriate. Approval settlement/recovery uses this instead
    /// of the ordinary preview helper so its frozen Tool set cannot silently lose
    /// `automation_report` across a process restart.
    pub(super) fn context_window_tool_projection_for_agent_run(
        &self,
        agent_run_id: &str,
        agent_input: &AgentChatInput,
        skill_resources: Option<Arc<mycopilot_core::skills::SkillResourceSession>>,
    ) -> Result<AgentContextWindowToolProjection, String> {
        let mcp_tools = self.capture_mcp_tool_runtime(agent_input);
        let automation_report_sink = self.automation_report_sink_for_agent_run_id(agent_run_id)?;
        self.context_window_tool_projection_with_mcp_and_automation_report(
            agent_input,
            skill_resources,
            mcp_tools,
            automation_report_sink,
        )
    }

    pub(super) fn context_window_tool_projection_with_mcp(
        &self,
        agent_input: &AgentChatInput,
        skill_resources: Option<Arc<mycopilot_core::skills::SkillResourceSession>>,
        mcp_tools: Option<McpToolRuntime>,
    ) -> Result<AgentContextWindowToolProjection, String> {
        self.context_window_tool_projection_with_mcp_and_automation_report(
            agent_input,
            skill_resources,
            mcp_tools,
            None,
        )
    }

    /// Builds the exact provider Tool projection for one started Automation run. The explicit
    /// run-scoped sink is deliberately absent from the general context-preview entry point so an
    /// ordinary HumanRoot can never acquire the Automation-only report capability.
    pub(super) fn context_window_tool_projection_with_mcp_and_automation_report(
        &self,
        agent_input: &AgentChatInput,
        skill_resources: Option<Arc<mycopilot_core::skills::SkillResourceSession>>,
        mcp_tools: Option<McpToolRuntime>,
        automation_report_sink: Option<Arc<dyn AutomationReportSink>>,
    ) -> Result<AgentContextWindowToolProjection, String> {
        let mut host_services = self
            .context_window_provider_host_services()
            .with_office_engine(Arc::clone(&self.office_engine));
        if let Some(sink) = automation_report_sink {
            host_services = host_services.with_automation_report_sink(sink);
        }
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
        if let Some(conversation_id) = agent_input
            .context
            .as_ref()
            .and_then(|context| context.conversation_id.as_deref())
        {
            let (preview_notifications, _preview_receiver) = tokio::sync::mpsc::unbounded_channel();
            let harness = crate::application::agent_harness::AgentCollaborationHarnessAdapter::new(
                Arc::clone(&self.storage),
                self.clone(),
                self.collaboration_authorizer(),
                Arc::clone(&self.collaboration_dispatcher),
                preview_notifications,
            );
            host_services = harness
                .attach_preview_to_host_services(host_services, conversation_id)
                .map_err(|error| error.to_string())?;
        }
        // AgentService always provides the real Host action executor to a started run. Passing
        // `true` keeps preview approval schemas aligned with that production boundary without
        // constructing an executable action closure during a read-only capacity inspection.
        prepare_context_window_tool_projection(agent_input, &host_services, true)
            .map_err(|error| error.to_string())
    }

    /// Builds the Host-private capability boundary used to hydrate provider-owned Assistant Turns
    /// for context previews. The returned value contains no executable action capability.
    pub(super) fn context_window_provider_host_services(&self) -> AgentRuntimeHostServices {
        let mut host_services = AgentRuntimeHostServices::new()
            .with_storage(Arc::clone(&self.storage))
            .with_web_search_policy(self.web_search_policy_source())
            .with_human_interaction_policy(Arc::new(
                crate::application::human_interaction::StoredHumanInteractionPolicy(Arc::clone(
                    &self.storage,
                )),
            ))
            .with_human_interaction_preview_readiness(true, true);
        if let Some(builtin_capabilities) = self.builtin_capabilities.clone() {
            host_services = host_services.with_builtin_capabilities(builtin_capabilities);
        }
        if let Some(provider_continuation_vault) = self.provider_continuation_vault.as_ref() {
            host_services = host_services
                .with_provider_continuation_vault(Arc::clone(provider_continuation_vault));
        }
        host_services
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
        expected_model_config_id: &str,
        expected_model: &str,
        notifications: CoreServerNotificationSender,
    ) -> AgentContextWindowObserver {
        let service = self.clone();
        let run_id = run_id.to_string();
        let conversation_id = conversation_id.to_string();
        let expected_model_config_id = expected_model_config_id.to_string();
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
                &expected_model_config_id,
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
        )?;
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
        let host_services = self.context_window_provider_host_services();
        let mut state = create_conversation_context_state_with_host_services(
            preview_input,
            conversation_id,
            &host_services,
        )
        .map_err(|error| error.to_string())?;
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
        let provider_runtime_capabilities = resolve_context_window_provider_capabilities(
            agent_input.provider_protocol_key.as_ref(),
        )?;
        let exact_provider_replay = provider_runtime_capabilities.is_some_and(|capabilities| {
            capabilities.context_projection()
                == mycopilot_core::ProviderContextProjectionSemantics::ExactProviderTurn
        });
        let partial_trace_semantics = provider_runtime_capabilities.map_or(
            mycopilot_core::ProviderPartialTraceSemantics::IncrementalBaseline,
            |capabilities| capabilities.partial_trace(),
        );
        if should_defer_partial_provider_trace(partial_trace_semantics, &trace) {
            // Approval boundaries can durably expose only the first call of a grouped Provider
            // turn. Runtime owns the exact checkpoint/raw replay for that in-progress exchange;
            // rebuilding a Host preview here would incorrectly require later queued calls to be
            // visible already. Keep the incremental safe projection until a complete terminal
            // trace can be privately hydrated.
            return Ok(None);
        }
        // A nonterminal grouped exchange can briefly end at a safe ToolResult while a later call
        // from the same Provider turn has not been published yet. Its durable trace is useful for
        // an incremental UI preview, but it is not a complete exact turn and must not be hydrated
        // as one. Once terminal, rebuild from the encrypted Host sidecar so the next Provider
        // request and its budget use the complete original turn.
        let mut needs_rebuild = exact_provider_replay && trace.terminal_status.is_terminal();
        {
            let mut states = self
                .conversation_context_states
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            if !needs_rebuild {
                if let Some(entry) = states.get_mut(conversation_id) {
                    if entry.configuration_revision == configuration_revision
                        && entry.active_run_id.as_deref() == Some(run_id)
                        && entry.active_assistant_message_id.as_deref()
                            == Some(assistant_message_id)
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
        self.emit_context_window_snapshot(
            notifications,
            run_id,
            conversation_id,
            agent_input.model_config_id.as_deref().unwrap_or_default(),
            snapshot,
        );
    }

    pub(super) fn emit_derived_context_window_snapshot(
        &self,
        notifications: &CoreServerNotificationSender,
        agent_input: &AgentChatInput,
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
        self.emit_context_window_snapshot(
            notifications,
            run_id,
            conversation_id,
            agent_input.model_config_id.as_deref().unwrap_or_default(),
            snapshot,
        );
    }

    pub(super) fn emit_context_window_snapshot(
        &self,
        notifications: &CoreServerNotificationSender,
        run_id: &str,
        conversation_id: &str,
        model_config_id: &str,
        snapshot: Option<AgentContextWindowSnapshot>,
    ) {
        if model_config_id.is_empty() || model_config_id.trim() != model_config_id {
            eprintln!(
                "ignored context-window snapshot because its frozen model configuration identity is missing or invalid"
            );
            return;
        }
        let Some(snapshot) = snapshot else {
            return;
        };
        let _ = notifications.send(agent_event_notification(AgentEvent::ContextWindowUpdated {
            run_id: run_id.to_string(),
            conversation_id: Some(conversation_id.to_string()),
            model_config_id: model_config_id.to_string(),
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

    /// Retires only the rebuildable context derivation after its authoritative Trace and model
    /// context append has committed. The observer must acknowledge that durable append to Runtime
    /// instead of turning a cache/budget-preview failure into a false ToolCall publication error.
    ///
    /// The exact running context-window snapshot intentionally remains intact: it accounts for
    /// the last Provider request and is more authoritative than a failed trace-derived estimate.
    /// A later sendable request or terminal commit replaces it through the normal observer path.
    pub(super) fn invalidate_derived_context_after_durable_trace(&self, conversation_id: &str) {
        self.invalidate_conversation_context_state(conversation_id);
    }

    pub fn invalidate_all_conversation_context_states(&self) {
        self.conversation_context_states
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clear();
    }
}

#[cfg(test)]
mod capability_tests {
    use super::*;

    fn in_progress_trace(items: Vec<ConversationTurnTraceItem>) -> ConversationTurnTrace {
        ConversationTurnTrace {
            schema_version: mycopilot_core::CONVERSATION_TURN_TRACE_SCHEMA_VERSION,
            run_id: "run-capability-trace".to_string(),
            conversation_id: "conversation-capability-trace".to_string(),
            assistant_message_id: "assistant-capability-trace".to_string(),
            terminal_status: ConversationTurnTraceTerminalStatus::InProgress,
            terminal_error: None,
            truncated: false,
            items,
        }
    }

    #[test]
    fn deferred_provider_projection_only_applies_to_an_unclosed_tool_call() {
        let text_only = in_progress_trace(vec![ConversationTurnTraceItem::AssistantNarration {
            sequence: 0,
            content: "still sampling".to_string(),
            truncated: false,
        }]);
        assert!(!should_defer_partial_provider_trace(
            mycopilot_core::ProviderPartialTraceSemantics::DeferUntilProviderTurnClosed,
            &text_only,
        ));

        let unclosed_tool = in_progress_trace(vec![ConversationTurnTraceItem::ToolCall {
            sequence: 0,
            call_id: "call-capability-trace".to_string(),
            tool: "read_file".to_string(),
            provenance: mycopilot_core::AgentToolIdentity::Builtin {
                tool_name: "read_file".to_string(),
            },
            operation: serde_json::json!({ "path": "README.md" }),
            approval_status: AgentApprovalStatus::Required,
            truncated: false,
        }]);
        assert!(should_defer_partial_provider_trace(
            mycopilot_core::ProviderPartialTraceSemantics::DeferUntilProviderTurnClosed,
            &unclosed_tool,
        ));
        assert!(!should_defer_partial_provider_trace(
            mycopilot_core::ProviderPartialTraceSemantics::IncrementalBaseline,
            &unclosed_tool,
        ));

        let provider_turn_still_open = in_progress_trace(vec![
            unclosed_tool.items[0].clone(),
            ConversationTurnTraceItem::ToolResult {
                sequence: 1,
                call_id: "call-capability-trace".to_string(),
                tool: "read_file".to_string(),
                status: mycopilot_core::ConversationTraceToolResultStatus::Succeeded,
                success: true,
                observation: serde_json::json!({ "content": "done" }),
                approval_status: AgentApprovalStatus::Required,
                error: None,
                truncated: false,
                archive: Default::default(),
            },
        ]);
        assert!(should_defer_partial_provider_trace(
            mycopilot_core::ProviderPartialTraceSemantics::DeferUntilProviderTurnClosed,
            &provider_turn_still_open,
        ));
    }

    #[test]
    fn context_preview_rejects_an_unknown_frozen_provider_registration() {
        let unsupported = ProviderProtocolKey {
            dialect: ProviderProtocolDialect::OpenAiChatCompletions,
            profile: mycopilot_core::ProviderProfileRef {
                id: mycopilot_core::ProviderProfileId::DeepSeekV4Chat,
                version: u32::MAX,
            },
            model_id: "unsupported-preview-provider".to_string(),
            provider_configuration_revision: None,
        };

        assert!(resolve_context_window_provider_capabilities(Some(&unsupported)).is_err());
    }
}
