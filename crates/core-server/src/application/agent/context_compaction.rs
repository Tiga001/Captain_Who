use super::*;

pub(super) struct RebuildRunningContextAfterCompactionRequest<'a> {
    agent_input: &'a AgentChatInput,
    run_id: &'a str,
    conversation_id: &'a str,
    assistant_message_id: &'a str,
    notifications: &'a CoreServerNotificationSender,
    tool_projection: Option<&'a AgentContextWindowToolProjection>,
}

pub(super) struct UpdateRunningConversationContextStateRequest<'a> {
    agent_input: &'a AgentChatInput,
    run_id: &'a str,
    conversation_id: &'a str,
    assistant_message_id: &'a str,
    trace: &'a ConversationTurnTrace,
    model_context_items: &'a [ConversationModelContextItem],
    configuration_revision: &'a str,
    tool_projection: Option<&'a AgentContextWindowToolProjection>,
}

impl AgentService {
    #[allow(clippy::too_many_arguments)]
    pub(super) fn trace_observer(
        &self,
        run_id: &str,
        conversation_id: &str,
        assistant_message_id: &str,
        created_at: i64,
        agent_input: AgentChatInput,
        tool_set_snapshot: RunContextToolProjection,
        notifications: CoreServerNotificationSender,
    ) -> AgentConversationTraceObserver {
        let service = self.clone();
        let snapshots = self.trace_snapshots.clone();
        let run_id = run_id.to_string();
        let conversation_id = conversation_id.to_string();
        let assistant_message_id = assistant_message_id.to_string();
        let configuration_revision = conversation_context_configuration_revision(&agent_input)
            .map_err(|error| error.to_string());
        Arc::new(move |snapshot| {
            snapshots
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .insert(run_id.clone(), snapshot.clone());
            let configuration_revision = configuration_revision
                .as_deref()
                .map_err(|error| AgentError::new(format!("无法准备会话上下文状态：{error}")))?;
            let tool_projection = tool_set_snapshot.projection();
            service
                .persist_in_progress_trace_snapshot(
                    &run_id,
                    &conversation_id,
                    &assistant_message_id,
                    created_at,
                    &agent_input,
                    &notifications,
                    &snapshot,
                    configuration_revision,
                    tool_projection.as_ref(),
                )
                .map_err(|error| AgentError::new(format!("无法增量持久化运行中会话轨迹：{error}")))
        })
    }

    pub(super) fn model_request_observer(
        &self,
        expected_run_id: &str,
        expected_conversation_id: &str,
        expected_assistant_message_id: &str,
    ) -> AgentModelRequestObserver {
        let storage = self.storage.clone();
        let expected_run_id = expected_run_id.to_string();
        let expected_conversation_id = expected_conversation_id.to_string();
        let expected_assistant_message_id = expected_assistant_message_id.to_string();
        Arc::new(move |observation| {
            let identity_matches = observation.run_id == expected_run_id
                && observation.conversation_id.as_deref()
                    == Some(expected_conversation_id.as_str())
                && observation.assistant_message_id.as_deref()
                    == Some(expected_assistant_message_id.as_str());
            if !identity_matches {
                eprintln!(
                    "refused model request observation with mismatched run or conversation identity: {}",
                    observation.id
                );
                return;
            }
            if let Err(error) = storage.save_model_request_observation(&observation) {
                // A provider response may already be visible to the user. Diagnostics must never
                // make the runtime replay that response and duplicate tool side effects.
                eprintln!(
                    "failed to persist model request observation {}: {error}",
                    observation.id
                );
            }
        })
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn context_compaction_services(
        &self,
        run_id: &str,
        conversation_id: &str,
        assistant_message_id: &str,
        agent_input: AgentChatInput,
        tool_set_snapshot: RunContextToolProjection,
        notifications: CoreServerNotificationSender,
    ) -> AgentContextCompactionServices {
        let generator: ContextCompactionSummaryGenerator = self
            .context_compaction_summary_generator
            .clone()
            .unwrap_or_else(|| {
                let generator = AgentContextCompactionModelGenerator::from_chat_input(&agent_input);
                Arc::new(move |request, cancellation| {
                    let generator = generator.clone();
                    Box::pin(async move { generator.generate(request, cancellation).await })
                })
            });
        let run_id = run_id.to_string();
        let conversation_id = conversation_id.to_string();
        let assistant_message_id = assistant_message_id.to_string();

        let prepare_service = self.clone();
        let prepare_run_id = run_id.clone();
        let prepare_conversation_id = conversation_id.clone();
        let prepare_assistant_message_id = assistant_message_id.clone();
        let prepare_agent_input = agent_input.clone();
        let prepare_tool_set_snapshot = tool_set_snapshot.clone();
        let prepare_notifications = notifications.clone();

        let commit_service = self.clone();
        let commit_run_id = run_id.clone();
        let commit_conversation_id = conversation_id.clone();
        let commit_assistant_message_id = assistant_message_id.clone();
        let commit_agent_input = agent_input;
        let commit_tool_set_snapshot = tool_set_snapshot;
        let commit_notifications = notifications.clone();

        let receipt_storage = self.storage.clone();
        let receipt_run_id = run_id;
        let receipt_conversation_id = conversation_id;
        let receipt_assistant_message_id = assistant_message_id;

        AgentContextCompactionServices::new(
            move |request, cancellation| {
                let service = prepare_service.clone();
                let run_id = prepare_run_id.clone();
                let conversation_id = prepare_conversation_id.clone();
                let assistant_message_id = prepare_assistant_message_id.clone();
                let agent_input = prepare_agent_input.clone();
                let tool_set_snapshot = prepare_tool_set_snapshot.clone();
                let notifications = prepare_notifications.clone();
                async move {
                    cancellation.check()?;
                    validate_compaction_request_identity(
                        &request.run_id,
                        &request.conversation_id,
                        &request.assistant_message_id,
                        &run_id,
                        &conversation_id,
                        &assistant_message_id,
                    )?;
                    let active_trace = service
                        .storage
                        .get_conversation_turn_trace(&assistant_message_id)
                        .map_err(AgentError::new)?;
                    validate_compaction_trace_boundary(
                        &request.covered_through,
                        active_trace.as_ref(),
                        &run_id,
                        &conversation_id,
                        &assistant_message_id,
                    )?;
                    let prefix = service
                        .storage
                        .prepare_context_compaction_prefix_if_current(
                            &conversation_id,
                            &request.covered_through,
                            request.expected_previous_summary_id.as_deref(),
                        )
                        .map_err(AgentError::new)?;
                    match prefix {
                        Some(prefix) => Ok(AgentContextCompactionPrepareOutcome::Ready(Arc::new(
                            prefix,
                        ))),
                        None => service
                            .rebuild_running_context_after_compaction(
                                RebuildRunningContextAfterCompactionRequest {
                                    agent_input: &agent_input,
                                    run_id: &run_id,
                                    conversation_id: &conversation_id,
                                    assistant_message_id: &assistant_message_id,
                                    notifications: &notifications,
                                    tool_projection: tool_set_snapshot.projection().as_ref(),
                                },
                            )
                            .map(|baseline| {
                                AgentContextCompactionPrepareOutcome::Refresh(Box::new(baseline))
                            })
                            .map_err(AgentError::new),
                    }
                }
            },
            move |request, cancellation| generator(request, cancellation),
            move |request: AgentContextCompactionCommitRequest, cancellation| {
                let service = commit_service.clone();
                let run_id = commit_run_id.clone();
                let conversation_id = commit_conversation_id.clone();
                let assistant_message_id = commit_assistant_message_id.clone();
                let agent_input = commit_agent_input.clone();
                let tool_set_snapshot = commit_tool_set_snapshot.clone();
                let notifications = commit_notifications.clone();
                async move {
                    cancellation.check()?;
                    validate_compaction_request_identity(
                        &request.run_id,
                        &request.conversation_id,
                        &request.assistant_message_id,
                        &run_id,
                        &conversation_id,
                        &assistant_message_id,
                    )?;
                    validate_compaction_request_identity(
                        &request.receipt.run_id,
                        &request.receipt.conversation_id,
                        &request.receipt.assistant_message_id,
                        &run_id,
                        &conversation_id,
                        &assistant_message_id,
                    )?;
                    let active_trace = service
                        .storage
                        .get_conversation_turn_trace(&assistant_message_id)
                        .map_err(AgentError::new)?;
                    validate_compaction_trace_boundary(
                        &request.prefix.covered_through,
                        active_trace.as_ref(),
                        &run_id,
                        &conversation_id,
                        &assistant_message_id,
                    )?;
                    let committed = service
                        .storage
                        .commit_context_compaction_prefix_with_receipt_if_current(
                            request.prefix.as_ref(),
                            request.draft,
                            &request.receipt,
                            &request.observation,
                        )
                        .map_err(AgentError::new)?;
                    service.invalidate_conversation_context_state(&conversation_id);
                    let baseline = service.rebuild_running_context_after_compaction(
                        RebuildRunningContextAfterCompactionRequest {
                            agent_input: &agent_input,
                            run_id: &run_id,
                            conversation_id: &conversation_id,
                            assistant_message_id: &assistant_message_id,
                            notifications: &notifications,
                            tool_projection: tool_set_snapshot.projection().as_ref(),
                        },
                    );
                    match (committed, baseline) {
                        (Some(summary), Ok(baseline)) => {
                            Ok(AgentContextCompactionCommitOutcome::Applied {
                                summary_id: summary.id,
                                baseline: Box::new(baseline),
                            })
                        }
                        (Some(summary), Err(error)) => Err(AgentError::structured(
                            "context_compaction_applied_rebuild_failed",
                            "上下文摘要已原子提交，但无法重建当前运行的上下文。",
                            serde_json::json!({
                                "summaryId": summary.id,
                                "cause": error,
                            }),
                        )),
                        (None, Ok(baseline)) => Ok(AgentContextCompactionCommitOutcome::Refresh(
                            Box::new(baseline),
                        )),
                        (None, Err(error)) => Err(AgentError::new(error)),
                    }
                }
            },
            move |receipt, observation| {
                let storage = receipt_storage.clone();
                let run_id = receipt_run_id.clone();
                let conversation_id = receipt_conversation_id.clone();
                let assistant_message_id = receipt_assistant_message_id.clone();
                async move {
                    validate_compaction_request_identity(
                        &receipt.run_id,
                        &receipt.conversation_id,
                        &receipt.assistant_message_id,
                        &run_id,
                        &conversation_id,
                        &assistant_message_id,
                    )?;
                    storage
                        .record_context_compaction_receipt(&receipt, observation.as_ref())
                        .map_err(AgentError::new)
                }
            },
        )
    }

    pub(super) fn rebuild_running_context_after_compaction(
        &self,
        request: RebuildRunningContextAfterCompactionRequest<'_>,
    ) -> Result<AgentContextBaseline, String> {
        let RebuildRunningContextAfterCompactionRequest {
            agent_input,
            run_id,
            conversation_id,
            assistant_message_id,
            notifications,
            tool_projection,
        } = request;
        self.invalidate_conversation_context_state(conversation_id);
        let conversation = self
            .storage
            .load_conversation(conversation_id)?
            .ok_or_else(|| format!("未找到对话：{conversation_id}"))?;
        let traces = self
            .storage
            .list_conversation_turn_traces(conversation_id)?;
        let summary = self
            .storage
            .get_active_context_compaction_summary(conversation_id)?;
        let full_model_context_logs = self
            .storage
            .list_conversation_model_context_logs(conversation_id)?;
        let active_trace = traces
            .iter()
            .find(|trace| trace.assistant_message_id == assistant_message_id);
        if let Some(trace) = active_trace {
            if trace.run_id != run_id
                || trace.conversation_id != conversation_id
                || trace.terminal_status != ConversationTurnTraceTerminalStatus::InProgress
            {
                return Err("压缩后重建上下文时，运行中 trace 身份或状态不一致。".to_string());
            }
        }

        let mut preview_input = agent_input.clone();
        preview_input.messages = conversation_history_messages_with_model_context(
            &conversation,
            &traces,
            &full_model_context_logs,
            summary.as_ref(),
            &[],
        );
        preview_input.context_compaction_summary = summary;
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

        let host_services = self.context_window_provider_host_services();
        let mut state = create_conversation_context_state_with_host_services(
            preview_input,
            conversation_id,
            &host_services,
        )
        .map_err(|error| error.to_string())?;
        let committed_activity_items = match active_trace {
            Some(trace) => {
                let model_context_items = full_model_context_logs
                    .iter()
                    .find(|log| log.assistant_message_id == assistant_message_id)
                    .map(|log| log.items.as_slice())
                    .unwrap_or_default();
                AgentConversationContextState::rendered_trace_activity_count(
                    trace,
                    model_context_items,
                )
                .map_err(|error| error.to_string())?
            }
            None => 0,
        };
        // The rebuilt baseline is the complete post-compaction model timeline: summary plus the
        // exact uncovered message/trace tail. Runtime adopts it and discards its duplicate
        // message/tool overlay while retaining non-journal run state.
        let runtime_baseline = state.shared_baseline().map_err(|error| error.to_string())?;
        let snapshot = if agent_input.context_window_indicator_enabled {
            Some(match tool_projection {
                Some(tool_projection) => state
                    .snapshot_with_skill_overlays_and_tool_projection(
                        agent_input.skill_discovery.as_ref(),
                        agent_input.skill_activation.as_ref(),
                        tool_projection,
                    )
                    .map_err(|error| error.to_string())?,
                None => state
                    .snapshot_with_skill_overlays(
                        agent_input.skill_discovery.as_ref(),
                        agent_input.skill_activation.as_ref(),
                    )
                    .map_err(|error| error.to_string())?,
            })
        } else {
            None
        };
        let entry = ConversationContextStateEntry {
            configuration_revision: state.configuration_revision().to_string(),
            state,
            active_run_id: Some(run_id.to_string()),
            active_assistant_message_id: Some(assistant_message_id.to_string()),
            committed_activity_items,
            terminal: false,
            last_access: self.next_conversation_context_state_access(),
        };
        self.insert_conversation_context_state(conversation_id, entry);
        self.emit_derived_context_window_snapshot(notifications, run_id, conversation_id, snapshot);
        Ok(runtime_baseline)
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn persist_in_progress_trace_snapshot(
        &self,
        run_id: &str,
        conversation_id: &str,
        assistant_message_id: &str,
        created_at: i64,
        agent_input: &AgentChatInput,
        notifications: &CoreServerNotificationSender,
        snapshot: &ConversationTraceSnapshot,
        configuration_revision: &str,
        tool_projection: Option<&AgentContextWindowToolProjection>,
    ) -> Result<Option<AgentContextBaseline>, String> {
        // Persist the complete audit view so a crash after an external side effect starts can be
        // correlated with its durable execution journal. Model context still receives only the
        // closed prefix and therefore never sees a half ToolCall/ToolResult exchange.
        let committed_snapshot = snapshot.committed_prefix();
        let context_trace =
            committed_snapshot.in_progress_trace(run_id, conversation_id, assistant_message_id);
        let previous_trace = self
            .storage
            .get_conversation_turn_trace(assistant_message_id)?;
        let previous_model_context_items = self
            .storage
            .get_conversation_model_context_log(assistant_message_id)?
            .map(|log| log.items)
            .unwrap_or_default();
        let previous_activity_items = previous_trace
            .as_ref()
            .map(|trace| {
                AgentConversationContextState::rendered_trace_activity_count(
                    trace,
                    &previous_model_context_items,
                )
                .map_err(|error| error.to_string())
            })
            .transpose()?
            .unwrap_or_default();
        let next_activity_items = AgentConversationContextState::rendered_trace_activity_count(
            &context_trace,
            &committed_snapshot.model_context_items,
        )
        .map_err(|error| error.to_string())?;
        let model_context_changed = previous_trace.is_none()
            || previous_activity_items != next_activity_items
            || previous_model_context_items.len() != committed_snapshot.model_context_items.len();
        // Only tools with an independently durable, non-replayable execution journal retain an
        // open call here. Approval-backed tools may enrich their call snapshot before settlement,
        // so persisting those calls early would violate the trace's append-only contract.
        let retain_open_call_for_recovery = matches!(
            snapshot.items.last(),
            Some(ConversationTurnTraceItem::ToolCall { tool, .. }) if tool == "image_generation"
        );
        let audit_trace = if retain_open_call_for_recovery {
            snapshot.in_progress_audit_trace(run_id, conversation_id, assistant_message_id)
        } else {
            context_trace.clone()
        };
        let changed = self
            .storage
            .append_in_progress_conversation_turn_trace_and_apply_guidances(
                &audit_trace,
                &committed_snapshot.model_context_items,
                created_at,
                now_ms(),
            )?;
        if provider_native_tool_trace_is_in_progress(agent_input, &context_trace) {
            // The Runtime already owns the exact grouped Provider turn in memory/checkpoint.
            // Returning no replacement baseline prevents a partial durable approval prefix from
            // splitting that turn or requiring later queued calls before they are published.
            self.invalidate_conversation_context_state(conversation_id);
            return Ok(None);
        }
        let update = self.update_running_conversation_context_state(
            UpdateRunningConversationContextStateRequest {
                agent_input,
                run_id,
                conversation_id,
                assistant_message_id,
                trace: &context_trace,
                model_context_items: &committed_snapshot.model_context_items,
                configuration_revision,
                tool_projection,
            },
        )?;
        if changed && model_context_changed {
            self.emit_derived_context_window_snapshot(
                notifications,
                run_id,
                conversation_id,
                update.snapshot,
            );
        }
        Ok(Some(update.baseline))
    }

    pub(super) fn update_running_conversation_context_state(
        &self,
        request: UpdateRunningConversationContextStateRequest<'_>,
    ) -> Result<ConversationContextStateUpdate, String> {
        let UpdateRunningConversationContextStateRequest {
            agent_input,
            run_id,
            conversation_id,
            assistant_message_id,
            trace,
            model_context_items,
            configuration_revision,
            tool_projection,
        } = request;
        let access = self.next_conversation_context_state_access();
        let needs_rebuild;
        {
            let mut states = self
                .conversation_context_states
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            if let Some(entry) = states.get_mut(conversation_id) {
                if entry.configuration_revision != configuration_revision {
                    needs_rebuild = true;
                } else {
                    let update = if !entry.terminal
                        && entry.active_run_id.as_deref() == Some(run_id)
                        && entry.active_assistant_message_id.as_deref()
                            == Some(assistant_message_id)
                    {
                        entry.state.append_trace_items(
                            trace,
                            model_context_items,
                            entry.committed_activity_items,
                        )
                    } else if entry.terminal
                        && entry.active_assistant_message_id.as_deref()
                            != Some(assistant_message_id)
                    {
                        let current_user = agent_input
                            .messages
                            .iter()
                            .rev()
                            .find(|message| message.role == "user")
                            .map(|message| {
                                (
                                    message.message_id.as_deref(),
                                    message.content.as_str(),
                                    message.created_at,
                                )
                            })
                            .unwrap_or((None, "", None));
                        (|| {
                            entry.state.append_user_message(
                                current_user.0,
                                current_user.1,
                                current_user.2,
                            )?;
                            entry.active_run_id = Some(run_id.to_string());
                            entry.active_assistant_message_id =
                                Some(assistant_message_id.to_string());
                            entry.committed_activity_items = 0;
                            entry.terminal = false;
                            entry
                                .state
                                .append_trace_items(trace, model_context_items, 0)
                        })()
                    } else {
                        Err(AgentError::new("会话上下文状态与当前运行身份不一致。"))
                    };
                    match update {
                        Ok(committed_activity_items) => {
                            entry.active_run_id = Some(run_id.to_string());
                            entry.committed_activity_items = committed_activity_items;
                            entry.last_access = access;
                            let baseline = entry
                                .state
                                .shared_baseline()
                                .map_err(|error| error.to_string())?;
                            let snapshot = if agent_input.context_window_indicator_enabled {
                                Some(match tool_projection {
                                    Some(tool_projection) => entry
                                        .state
                                        .snapshot_with_skill_overlays_and_tool_projection(
                                            agent_input.skill_discovery.as_ref(),
                                            agent_input.skill_activation.as_ref(),
                                            tool_projection,
                                        )
                                        .map_err(|error| error.to_string())?,
                                    None => entry
                                        .state
                                        .snapshot_with_skill_overlays(
                                            agent_input.skill_discovery.as_ref(),
                                            agent_input.skill_activation.as_ref(),
                                        )
                                        .map_err(|error| error.to_string())?,
                                })
                            } else {
                                None
                            };
                            return Ok(ConversationContextStateUpdate { baseline, snapshot });
                        }
                        Err(_) => needs_rebuild = true,
                    }
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
            agent_input.skill_activation.as_ref(),
            tool_projection,
        )
    }

    pub(super) fn seed_trace_snapshot_from_checkpoint(
        &self,
        run_id: &str,
        checkpoint: Option<&AgentRunCheckpoint>,
    ) {
        let Some(checkpoint) = checkpoint else {
            return;
        };
        self.trace_snapshots
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .entry(run_id.to_string())
            .or_insert_with(|| ConversationTraceSnapshot {
                items: checkpoint.conversation_trace_items.clone(),
                model_context_items: checkpoint.conversation_model_context_items.clone(),
                next_sequence: checkpoint.next_conversation_trace_sequence,
                truncated: checkpoint.conversation_trace_truncated,
            });
    }

    pub(super) fn commit_trace_snapshot_with_continuation(
        &self,
        record: &PendingActionRecord,
        agent_input: &AgentChatInput,
        notifications: &CoreServerNotificationSender,
    ) -> Result<(), String> {
        let checkpoint = record
            .agent_input
            .resume_checkpoint
            .as_ref()
            .ok_or_else(|| "审批续跑缺少会话轨迹检查点。".to_string())?;
        let continuation = agent_input
            .tool_continuation
            .as_ref()
            .ok_or_else(|| "审批续跑缺少工具结果。".to_string())?;
        let run_id = &record.snapshot.run_id;
        let conversation_id = record
            .snapshot
            .conversation_id
            .as_deref()
            .ok_or_else(|| "审批续跑缺少 conversation id。".to_string())?;
        let assistant_message_id = record
            .snapshot
            .assistant_message_id
            .as_deref()
            .ok_or_else(|| "审批续跑缺少 assistant message id。".to_string())?;
        let snapshot = self.archived_continuation_trace_snapshot(
            conversation_id,
            assistant_message_id,
            agent_input,
            checkpoint,
            continuation,
            mycopilot_core::storage::now_ms(),
        )?;
        self.trace_snapshots
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .insert(run_id.to_string(), snapshot.clone());
        let configuration_revision =
            conversation_context_configuration_revision(&record.agent_input)
                .map_err(|error| error.to_string())?;
        let skill_resources = self
            .restore_skill_resource_session(&record.agent_input)
            .map_err(|error| error.to_string())?;
        let tool_projection =
            self.context_window_tool_projection(&record.agent_input, skill_resources)?;
        self.persist_in_progress_trace_snapshot(
            run_id,
            conversation_id,
            assistant_message_id,
            record.snapshot.created_at,
            &record.agent_input,
            notifications,
            &snapshot,
            &configuration_revision,
            Some(&tool_projection),
        )
        .map(|_| ())
    }

    /// Commits a manually approved command's terminal audit and paired continuation as one fact.
    pub(super) fn commit_audited_command_result_trace_with_continuation(
        &self,
        record: &PendingActionRecord,
        agent_input: &AgentChatInput,
        target_status: PendingActionStatus,
        command_result: &AgentCommandExecutionResult,
        completed_at: i64,
        notifications: &CoreServerNotificationSender,
    ) -> Result<(), String> {
        self.commit_audited_result_trace_with_continuation(
            record,
            agent_input,
            target_status,
            Some(command_result),
            completed_at,
            notifications,
        )
    }

    /// Commits a manually approved file-producing action's terminal audit, pending target and
    /// paired ToolResult trace in one storage transaction.
    pub(super) fn commit_audited_result_trace_with_continuation(
        &self,
        record: &PendingActionRecord,
        agent_input: &AgentChatInput,
        target_status: PendingActionStatus,
        command_result: Option<&AgentCommandExecutionResult>,
        completed_at: i64,
        notifications: &CoreServerNotificationSender,
    ) -> Result<(), String> {
        self.commit_audited_result_trace_with_continuation_for_decision(
            record,
            agent_input,
            "approved",
            target_status,
            command_result,
            completed_at,
            notifications,
        )
    }

    /// Commits a user's explicit MCP rejection before the external dispatch boundary.
    ///
    /// `agent_input` must contain the durable MCP projection, not the live feedback-bearing
    /// result. The live continuation is retained only in process for the next model request.
    pub(super) fn commit_rejected_mcp_result_trace_with_continuation(
        &self,
        record: &PendingActionRecord,
        agent_input: &AgentChatInput,
        completed_at: i64,
        notifications: &CoreServerNotificationSender,
    ) -> Result<(), String> {
        self.commit_audited_result_trace_with_continuation_for_decision(
            record,
            agent_input,
            "rejected",
            PendingActionStatus::Rejected,
            None,
            completed_at,
            notifications,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn commit_audited_result_trace_with_continuation_for_decision(
        &self,
        record: &PendingActionRecord,
        agent_input: &AgentChatInput,
        decision: &str,
        target_status: PendingActionStatus,
        command_result: Option<&AgentCommandExecutionResult>,
        completed_at: i64,
        notifications: &CoreServerNotificationSender,
    ) -> Result<(), String> {
        let checkpoint = record
            .agent_input
            .resume_checkpoint
            .as_ref()
            .ok_or_else(|| "审批续跑缺少会话轨迹检查点。".to_string())?;
        let continuation = agent_input
            .tool_continuation
            .as_ref()
            .ok_or_else(|| "审批续跑缺少工具结果。".to_string())?;
        let run_id = &record.snapshot.run_id;
        let conversation_id = record
            .snapshot
            .conversation_id
            .as_deref()
            .ok_or_else(|| "审批续跑缺少 conversation id。".to_string())?;
        let assistant_message_id = record
            .snapshot
            .assistant_message_id
            .as_deref()
            .ok_or_else(|| "审批续跑缺少 assistant message id。".to_string())?;
        let snapshot = self.archived_continuation_trace_snapshot(
            conversation_id,
            assistant_message_id,
            agent_input,
            checkpoint,
            continuation,
            completed_at,
        )?;
        let configuration_revision =
            conversation_context_configuration_revision(&record.agent_input)
                .map_err(|error| error.to_string())?;
        let skill_resources = self
            .restore_skill_resource_session(&record.agent_input)
            .map_err(|error| error.to_string())?;
        let tool_projection =
            self.context_window_tool_projection(&record.agent_input, skill_resources)?;
        let trace = snapshot.in_progress_trace(run_id, conversation_id, assistant_message_id);
        let model_context_items = snapshot.committed_prefix().model_context_items;
        let trace_changed = if decision == "rejected" {
            self.persist_rejected_mcp_audited_result_trace(
                record,
                &continuation.result,
                &trace,
                &model_context_items,
                completed_at,
            )?
        } else {
            self.persist_manual_audited_result_trace(
                record,
                target_status,
                command_result,
                &continuation.result,
                &trace,
                &model_context_items,
                completed_at,
            )?
        };

        self.trace_snapshots
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .insert(run_id.to_string(), snapshot);
        if provider_native_tool_trace_is_in_progress(&record.agent_input, &trace) {
            self.invalidate_conversation_context_state(conversation_id);
            return Ok(());
        }
        match self.update_running_conversation_context_state(
            UpdateRunningConversationContextStateRequest {
                agent_input: &record.agent_input,
                run_id,
                conversation_id,
                assistant_message_id,
                trace: &trace,
                model_context_items: &model_context_items,
                configuration_revision: &configuration_revision,
                tool_projection: Some(&tool_projection),
            },
        ) {
            Ok(update) if trace_changed => self.emit_derived_context_window_snapshot(
                notifications,
                run_id,
                conversation_id,
                update.snapshot,
            ),
            Ok(_) => {}
            Err(error) => {
                self.invalidate_conversation_context_state(conversation_id);
                eprintln!("failed to update derived context after durable action result: {error}");
            }
        }
        Ok(())
    }

    /// Reconciles a commit-unknown manual command settlement from one authoritative snapshot.
    ///
    /// `CommittedAtBoundary` adopts the exact candidate into derived in-memory state so the same
    /// process may safely continue. `CommittedAndAdvanced` is owned by another continuation and
    /// deliberately invalidates local state; callers must not run the model a second time.
    pub(super) fn inspect_audited_command_result_trace_with_continuation(
        &self,
        record: &PendingActionRecord,
        agent_input: &AgentChatInput,
        target_status: PendingActionStatus,
        command_result: &AgentCommandExecutionResult,
        completed_at: i64,
    ) -> Result<AgentPendingActionSettlementInspection, String> {
        self.inspect_audited_result_trace_with_continuation(
            record,
            agent_input,
            target_status,
            Some(command_result),
            completed_at,
        )
    }

    pub(super) fn inspect_audited_result_trace_with_continuation(
        &self,
        record: &PendingActionRecord,
        agent_input: &AgentChatInput,
        target_status: PendingActionStatus,
        command_result: Option<&AgentCommandExecutionResult>,
        completed_at: i64,
    ) -> Result<AgentPendingActionSettlementInspection, String> {
        self.inspect_audited_result_trace_with_continuation_for_decision(
            record,
            agent_input,
            "approved",
            target_status,
            command_result,
            completed_at,
        )
    }

    /// Reconciles an explicit pre-dispatch MCP rejection whose durable commit returned an error.
    ///
    /// The same `completed_at` used by the commit attempt is required so the candidate audit and
    /// trace identity remain byte-for-byte authoritative.
    pub(super) fn inspect_rejected_mcp_result_trace_with_continuation(
        &self,
        record: &PendingActionRecord,
        agent_input: &AgentChatInput,
        completed_at: i64,
    ) -> Result<AgentPendingActionSettlementInspection, String> {
        self.inspect_audited_result_trace_with_continuation_for_decision(
            record,
            agent_input,
            "rejected",
            PendingActionStatus::Rejected,
            None,
            completed_at,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn inspect_audited_result_trace_with_continuation_for_decision(
        &self,
        record: &PendingActionRecord,
        agent_input: &AgentChatInput,
        decision: &str,
        target_status: PendingActionStatus,
        command_result: Option<&AgentCommandExecutionResult>,
        completed_at: i64,
    ) -> Result<AgentPendingActionSettlementInspection, String> {
        let checkpoint = record
            .agent_input
            .resume_checkpoint
            .as_ref()
            .ok_or_else(|| "审批续跑缺少会话轨迹检查点。".to_string())?;
        let continuation = agent_input
            .tool_continuation
            .as_ref()
            .ok_or_else(|| "审批续跑缺少工具结果。".to_string())?;
        let run_id = &record.snapshot.run_id;
        let conversation_id = record
            .snapshot
            .conversation_id
            .as_deref()
            .ok_or_else(|| "审批续跑缺少 conversation id。".to_string())?;
        let assistant_message_id = record
            .snapshot
            .assistant_message_id
            .as_deref()
            .ok_or_else(|| "审批续跑缺少 assistant message id。".to_string())?;
        let snapshot = self.archived_continuation_trace_snapshot(
            conversation_id,
            assistant_message_id,
            agent_input,
            checkpoint,
            continuation,
            completed_at,
        )?;
        let trace = snapshot.in_progress_trace(run_id, conversation_id, assistant_message_id);
        let model_context_items = snapshot.committed_prefix().model_context_items;
        let outcome = if decision == "rejected" {
            self.inspect_rejected_mcp_audited_result_trace(
                record,
                &continuation.result,
                &trace,
                &model_context_items,
                completed_at,
            )?
        } else {
            self.inspect_manual_audited_result_trace(
                record,
                target_status,
                command_result,
                &continuation.result,
                &trace,
                &model_context_items,
                completed_at,
            )?
        };
        match outcome {
            AgentPendingActionSettlementInspection::CommittedAtBoundary => {
                self.trace_snapshots
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .insert(run_id.to_string(), snapshot);
                match conversation_context_configuration_revision(&record.agent_input) {
                    Ok(configuration_revision) => {
                        let tool_projection = self
                            .restore_skill_resource_session(&record.agent_input)
                            .map_err(|error| error.to_string())
                            .and_then(|resources| {
                                self.context_window_tool_projection(&record.agent_input, resources)
                            });
                        let Ok(tool_projection) = tool_projection else {
                            self.invalidate_conversation_context_state(conversation_id);
                            return Ok(AgentPendingActionSettlementInspection::CommittedAtBoundary);
                        };
                        if let Err(error) = self.update_running_conversation_context_state(
                            UpdateRunningConversationContextStateRequest {
                                agent_input: &record.agent_input,
                                run_id,
                                conversation_id,
                                assistant_message_id,
                                trace: &trace,
                                model_context_items: &model_context_items,
                                configuration_revision: &configuration_revision,
                                tool_projection: Some(&tool_projection),
                            },
                        ) {
                            self.invalidate_conversation_context_state(conversation_id);
                            eprintln!(
                                "failed to adopt derived context after reconciling action result: {error}"
                            );
                        }
                    }
                    Err(error) => {
                        self.invalidate_conversation_context_state(conversation_id);
                        eprintln!(
                            "failed to rebuild derived context revision after reconciling action result: {error}"
                        );
                    }
                }
            }
            AgentPendingActionSettlementInspection::CommittedAndAdvanced => {
                self.discard_trace_snapshot(run_id);
                self.invalidate_conversation_context_state(conversation_id);
            }
            AgentPendingActionSettlementInspection::DefinitelyUncommitted
            | AgentPendingActionSettlementInspection::Diverged { .. } => {}
        }
        Ok(outcome)
    }

    /// Stores the exact Host result before constructing any model-visible continuation state.
    ///
    /// Approval execution happens outside the live tool registry and may be retried after a
    /// commit-unknown failure or process restart. The archive's trace-item identity makes this
    /// operation idempotent: the same continuation reuses the immutable blob, while a different
    /// result for the same call/sequence fails closed before Trace or model context can advance.
    fn archived_continuation_trace_snapshot(
        &self,
        conversation_id: &str,
        assistant_message_id: &str,
        agent_input: &AgentChatInput,
        checkpoint: &AgentRunCheckpoint,
        continuation: &AgentToolContinuation,
        created_at: i64,
    ) -> Result<ConversationTraceSnapshot, String> {
        validate_continuation_result_identity(&continuation.call, &continuation.result)?;
        let sequence = continuation_result_sequence(checkpoint, &continuation.call);
        let archive_result = project_persisted_continuation_for_archive(&continuation.result);
        let model_result = project_persisted_continuation_for_model(&continuation.result);
        let model_gate_truncates =
            mycopilot_core::persisted_continuation_model_projection_would_truncate(
                &agent_input.model,
                &agent_input.api_url,
                agent_input.api_style,
                &continuation.result,
            );
        let truncated_at_source =
            mycopilot_core::tool_result_truncated_at_source(&continuation.result);
        let exact_preview_truncated = continuation.result.exact_archive_file.is_some()
            && continuation.result.result.as_ref().is_some_and(
                mycopilot_core::exact_capture::value_has_recoverable_preview_truncation,
            );
        let model_projection_truncated =
            tool_result_projection_differs(&archive_result, &model_result)
                || model_gate_truncates
                || exact_preview_truncated;
        let archive_projection_truncated =
            tool_result_projection_differs(&continuation.result, &archive_result);
        let archive = if let Some(exact_file) = continuation
            .result
            .exact_archive_file
            .as_ref()
            .or(archive_result.exact_archive_file.as_ref())
        {
            self.storage.archive_conversation_tool_result_file(
                mycopilot_core::storage::conversation_history_archive_repository::ConversationHistoryArchiveFileInput {
                    conversation_id: conversation_id.to_string(),
                    assistant_message_id: assistant_message_id.to_string(),
                    sequence,
                    call_id: archive_result.call_id.clone(),
                    tool: archive_result.tool.clone(),
                    content_type:
                        "application/vnd.mycopilot.agent-tool-result+json".to_string(),
                    content_path: exact_file.path().to_path_buf(),
                    truncated_at_source,
                    model_projection_truncated,
                    archive_projection_truncated,
                    created_at,
                },
            )
        } else {
            let content = serde_json::to_string(&archive_result)
                .map_err(|error| format!("无法序列化审批工具的精确历史结果：{error}"))?;
            self.storage.archive_conversation_tool_result(
                mycopilot_core::storage::conversation_history_archive_repository::ConversationHistoryArchiveInput {
                    conversation_id: conversation_id.to_string(),
                    assistant_message_id: assistant_message_id.to_string(),
                    sequence,
                    call_id: archive_result.call_id.clone(),
                    tool: archive_result.tool.clone(),
                    content_type:
                        "application/vnd.mycopilot.agent-tool-result+json".to_string(),
                    content,
                    truncated_at_source,
                    model_projection_truncated,
                    archive_projection_truncated,
                    created_at,
                },
            )
        }
        .map_err(|error| format!("无法归档审批工具的精确历史结果：{error}"))?;
        let archive_metadata = mycopilot_core::ConversationHistoryArchiveTraceMetadata {
            archive_ref: Some(archive.archive_ref),
            content_hash: Some(archive.content_hash),
            archived_bytes: Some(archive.total_bytes),
            archived_completely: Some(archive.archived_completely),
            truncated_at_source: archive.truncated_at_source,
            model_projection_truncated: archive.model_projection_truncated,
            history_projection_truncated: false,
            archive_projection_truncated: archive.archive_projection_truncated,
        };

        let model_observation = project_persisted_continuation_observation(
            &agent_input.model,
            &agent_input.api_url,
            agent_input.api_style,
            &continuation.result,
            &archive_metadata,
        )
        .map_err(|error| error.to_string())?;
        Ok(
            conversation_trace_snapshot_from_checkpoint_and_continuation_with_projection(
                checkpoint,
                &continuation.call,
                &continuation.result,
                Some(assistant_message_id),
                &model_observation,
                archive_metadata,
            ),
        )
    }

    pub(super) fn discard_trace_snapshot(&self, run_id: &str) {
        self.trace_snapshots
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .remove(run_id);
    }
}

fn provider_native_tool_trace_is_in_progress(
    agent_input: &AgentChatInput,
    trace: &ConversationTurnTrace,
) -> bool {
    trace.terminal_status == ConversationTurnTraceTerminalStatus::InProgress
        && agent_input
            .provider_profile_config
            .as_ref()
            .is_some_and(|profile| profile.profile.id == ProviderProfileId::DeepSeekV4Chat)
        && trace
            .items
            .iter()
            .any(|item| matches!(item, ConversationTurnTraceItem::ToolCall { .. }))
}

fn validate_continuation_result_identity(
    call: &AgentToolCall,
    result: &AgentToolResult,
) -> Result<(), String> {
    if result.call_id == call.id && result.tool == call.tool {
        return Ok(());
    }
    Err(format!(
        "审批工具结果身份不匹配：callId={}，tool={}，resultCallId={}，resultTool={}",
        call.id, call.tool, result.call_id, result.tool
    ))
}

fn continuation_result_sequence(checkpoint: &AgentRunCheckpoint, call: &AgentToolCall) -> u64 {
    checkpoint
        .next_conversation_trace_sequence
        .saturating_add(u64::from(!checkpoint.conversation_trace_items.iter().any(
            |item| {
                matches!(
                    item,
                    ConversationTurnTraceItem::ToolCall { call_id, .. } if call_id == &call.id
                )
            },
        )))
}

fn tool_result_projection_differs(left: &AgentToolResult, right: &AgentToolResult) -> bool {
    match (serde_json::to_vec(left), serde_json::to_vec(right)) {
        (Ok(left), Ok(right)) => left != right,
        _ => true,
    }
}

pub(super) fn validate_compaction_request_identity(
    request_run_id: &str,
    request_conversation_id: &str,
    request_assistant_message_id: &str,
    expected_run_id: &str,
    expected_conversation_id: &str,
    expected_assistant_message_id: &str,
) -> AgentResult<()> {
    if request_run_id == expected_run_id
        && request_conversation_id == expected_conversation_id
        && request_assistant_message_id == expected_assistant_message_id
    {
        return Ok(());
    }
    Err(AgentError::structured(
        "context_compaction_identity_mismatch",
        "上下文压缩请求与当前运行身份不一致。",
        serde_json::json!({
            "requestRunId": request_run_id,
            "requestConversationId": request_conversation_id,
            "requestAssistantMessageId": request_assistant_message_id,
        }),
    ))
}

pub(super) fn validate_compaction_trace_boundary(
    covered_through: &ContextJournalCursor,
    active_trace: Option<&ConversationTurnTrace>,
    expected_run_id: &str,
    expected_conversation_id: &str,
    expected_assistant_message_id: &str,
) -> AgentResult<()> {
    let Some(trace) = active_trace else {
        if !matches!(
            covered_through,
            ContextJournalCursor::TraceItem {
                assistant_message_id,
                ..
            } if assistant_message_id == expected_assistant_message_id
        ) {
            return Ok(());
        }
        return Err(AgentError::structured(
            "context_compaction_trace_mismatch",
            "上下文压缩请求缺少当前运行的会话轨迹。",
            serde_json::json!({
                "assistantMessageId": expected_assistant_message_id,
            }),
        ));
    };
    if trace.run_id != expected_run_id
        || trace.conversation_id != expected_conversation_id
        || trace.assistant_message_id != expected_assistant_message_id
        || trace.terminal_status != ConversationTurnTraceTerminalStatus::InProgress
    {
        return Err(AgentError::structured(
            "context_compaction_trace_mismatch",
            "上下文压缩请求对应的运行中会话轨迹身份无效。",
            serde_json::json!({
                "runId": trace.run_id,
                "conversationId": trace.conversation_id,
                "assistantMessageId": trace.assistant_message_id,
                "terminalStatus": trace.terminal_status,
            }),
        ));
    }
    if let ContextJournalCursor::TraceItem {
        assistant_message_id,
        sequence,
    } = covered_through
    {
        if assistant_message_id == expected_assistant_message_id
            && !trace
                .items
                .iter()
                .any(|item| item.sequence() == *sequence && item.is_safe_compaction_boundary())
        {
            return Err(AgentError::structured(
                "context_compaction_trace_boundary_missing",
                "上下文压缩边界不是当前运行中已闭合的轨迹项。",
                serde_json::json!({
                    "assistantMessageId": assistant_message_id,
                    "sequence": sequence,
                    "persistedTraceItemCount": trace.items.len(),
                }),
            ));
        }
    }
    Ok(())
}
