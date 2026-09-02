use super::*;

pub(super) fn restore_pending_usage_contexts(
    storage: &StorageService,
    pending_actions: &HashMap<String, PendingActionRecord>,
) -> Result<HashMap<String, AgentRunUsageState>, String> {
    let mut restored = HashMap::new();
    for record in pending_actions.values() {
        let Some(conversation_id) = record
            .snapshot
            .conversation_id
            .as_deref()
            .filter(|value| !value.trim().is_empty())
        else {
            continue;
        };
        let Some(assistant_message_id) = record
            .snapshot
            .assistant_message_id
            .as_deref()
            .filter(|value| !value.trim().is_empty())
        else {
            continue;
        };
        let (provider_protocol_key, provider_profile_config) =
            if let Some(checkpoint) = record.agent_input.resume_checkpoint.as_ref() {
                if checkpoint.run_id != record.snapshot.run_id {
                    return Err(format!(
                        "pending run {} usage checkpoint owner mismatch",
                        record.snapshot.run_id
                    ));
                }
                (
                    &checkpoint.provider_protocol_key,
                    &checkpoint.provider_profile_config,
                )
            } else {
                let (Some(provider_protocol_key), Some(provider_profile_config)) = (
                    record.agent_input.provider_protocol_key.as_ref(),
                    record.agent_input.provider_profile_config.as_ref(),
                ) else {
                    continue;
                };
                (provider_protocol_key, provider_profile_config)
            };
        provider_protocol_key
            .validate_against_config(provider_profile_config)
            .map_err(|error| {
                format!(
                    "pending run {} usage Provider identity mismatch: {error}",
                    record.snapshot.run_id
                )
            })?;
        if provider_protocol_key.model_id != record.agent_input.model {
            return Err(format!(
                "pending run {} usage Provider model identity mismatch",
                record.snapshot.run_id
            ));
        }
        let model_config_id = record
            .agent_input
            .model_config_id
            .as_deref()
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| {
                format!(
                    "pending run {} usage local model owner is unavailable",
                    record.snapshot.run_id
                )
            })?;
        let usage_semantics =
            mycopilot_core::resolve_provider_runtime_capabilities(provider_protocol_key)
                .map_err(|error| {
                    format!(
                        "pending run {} usage capability unavailable: {error}",
                        record.snapshot.run_id
                    )
                })?
                .usage();
        let Some(persisted) = storage.load_agent_usage_for_owner(
            &record.snapshot.run_id,
            conversation_id,
            assistant_message_id,
        )?
        else {
            // Approval persistence and usage persistence commit separately, so a crash between
            // them can leave no authoritative usage row. Providers may also omit usage. Resume
            // with an explicitly unpriced, empty accumulator derived
            // only from the frozen owner/protocol; never borrow the current model Profile or
            // prices, which may have changed while approval was pending.
            let fallback_model_name = storage
                .load_model_settings()?
                .and_then(|settings| {
                    settings
                        .models
                        .into_iter()
                        .find(|model| model.id == model_config_id)
                        .map(|model| model.display_label())
                })
                .filter(|label| !label.trim().is_empty())
                .or_else(|| {
                    let provider_model_id = record.agent_input.model.trim();
                    (!provider_model_id.is_empty()).then(|| provider_model_id.to_string())
                })
                .unwrap_or_else(|| "Model unavailable".to_string());
            let state = AgentRunUsageState {
                context: AgentRunUsageContext {
                    conversation_id: conversation_id.to_string(),
                    assistant_message_id: assistant_message_id.to_string(),
                    run_id: record.snapshot.run_id.clone(),
                    project_id: record
                        .agent_input
                        .context
                        .as_ref()
                        .and_then(|context| context.project_id.clone()),
                    model_id: model_config_id.to_string(),
                    // The authoritative Usage row normally freezes a user-visible model label
                    // before approval. If a crash happened between the approval and Usage
                    // commits, recover a visible label without ever projecting the private local
                    // model-config identity.
                    model_name: fallback_model_name,
                    provider_usage_semantics: usage_semantics,
                    input_price: None,
                    cached_input_price: None,
                    output_price: None,
                    started_at: record.snapshot.created_at,
                },
                usage: None,
                status: AgentRunStatus::WaitingForApproval,
                error: None,
            };
            insert_restored_usage_state(&mut restored, &record.snapshot.run_id, state)?;
            continue;
        };
        if persisted.model_id != model_config_id {
            return Err(format!(
                "pending run {} usage model owner mismatch",
                record.snapshot.run_id
            ));
        }
        let started_at = persisted.started_at.ok_or_else(|| {
            format!(
                "pending run {} authoritative usage has no start time",
                record.snapshot.run_id
            )
        })?;
        let usage = (persisted.input_tokens.is_some()
            || persisted.output_tokens.is_some()
            || persisted.output_thinking_tokens.is_some()
            || persisted.total_tokens.is_some()
            || persisted.cached_input_tokens.is_some()
            || persisted.cache_creation_input_tokens.is_some()
            || persisted.billable_request_count > 0)
            .then_some(AgentUsage {
                input_tokens: persisted.input_tokens,
                output_tokens: persisted.output_tokens,
                output_thinking_tokens: persisted.output_thinking_tokens,
                total_tokens: persisted.total_tokens,
                cached_input_tokens: persisted.cached_input_tokens,
                cache_creation_input_tokens: persisted.cache_creation_input_tokens,
                billable_request_count: Some(persisted.billable_request_count),
            });
        let state = AgentRunUsageState {
            context: AgentRunUsageContext {
                conversation_id: persisted.conversation_id,
                assistant_message_id: persisted.message_id,
                run_id: persisted.run_id.clone(),
                project_id: persisted.project_id,
                model_id: persisted.model_id,
                model_name: persisted.model_name,
                provider_usage_semantics: usage_semantics,
                input_price: persisted.input_price,
                cached_input_price: persisted.cached_input_price,
                output_price: persisted.output_price,
                started_at,
            },
            usage,
            status: AgentRunStatus::WaitingForApproval,
            error: persisted.error,
        };
        insert_restored_usage_state(&mut restored, &persisted.run_id, state)?;
    }
    Ok(restored)
}

fn insert_restored_usage_state(
    restored: &mut HashMap<String, AgentRunUsageState>,
    run_id: &str,
    state: AgentRunUsageState,
) -> Result<(), String> {
    if let Some(existing) = restored.insert(run_id.to_string(), state.clone()) {
        if existing != state {
            return Err(format!(
                "pending run {run_id} has conflicting authoritative usage owners"
            ));
        }
    }
    Ok(())
}

impl AgentService {
    pub(super) fn register_usage_context(&self, run_id: &str, context: AgentRunUsageContext) {
        let mut contexts = self
            .usage_contexts
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        contexts.insert(
            run_id.to_string(),
            AgentRunUsageState {
                context,
                usage: None,
                status: AgentRunStatus::Running,
                error: None,
            },
        );
    }

    pub(super) fn discard_usage_context(&self, run_id: &str) {
        let mut contexts = self
            .usage_contexts
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let conversation_id = contexts
            .remove(run_id)
            .map(|state| state.context.conversation_id);
        drop(contexts);
        if let Some(conversation_id) = conversation_id {
            if let Err(error) = self
                .command_sessions
                .release_managed_workspace(&conversation_id, run_id)
            {
                eprintln!("failed to release managed command workspace: {error}");
            }
        }
        self.discard_trace_snapshot(run_id);
    }

    pub(super) fn find_usage_run_id(
        &self,
        conversation_id: &str,
        assistant_message_id: &str,
    ) -> Option<String> {
        let contexts = self
            .usage_contexts
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        contexts
            .iter()
            .find(|(_, state)| {
                state.context.conversation_id == conversation_id
                    && state.context.assistant_message_id == assistant_message_id
            })
            .map(|(run_id, _)| run_id.clone())
    }

    /// Projects one runtime segment onto the backend-owned logical run total without mutating it.
    ///
    /// A single agent run can cross several runtime invocations while approvals are resolved.
    /// Runtime events only know about their current invocation, while renderer state must always
    /// receive the cumulative logical-run value so replaying an event remains idempotent.
    pub(super) fn preview_cumulative_run_usage(
        &self,
        run_id: &str,
        next: Option<AgentUsage>,
    ) -> Option<AgentUsage> {
        let (mut usage, usage_semantics) = {
            let contexts = self
                .usage_contexts
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            let state = contexts.get(run_id)?;
            (state.usage.clone(), state.context.provider_usage_semantics)
        };
        usage_semantics.merge_usage(&mut usage, next);
        usage
    }

    pub(super) fn project_cumulative_usage_onto_event(&self, mut event: AgentEvent) -> AgentEvent {
        if let AgentEvent::Done { run_id, usage, .. } = &mut event {
            *usage = self.preview_cumulative_run_usage(run_id, usage.take());
        }
        event
    }

    pub(super) fn persist_run_usage(
        &self,
        run_id: &str,
        status: AgentRunStatus,
        usage: Option<AgentUsage>,
        error: Option<String>,
    ) -> Result<(), String> {
        let Some(record) = self.prepare_run_usage_record(run_id, status, usage, error) else {
            return Ok(());
        };
        self.storage.upsert_agent_usage(record)?;
        self.finish_persisted_run_usage(run_id, status);
        Ok(())
    }

    pub(super) fn prepare_run_usage_record(
        &self,
        run_id: &str,
        status: AgentRunStatus,
        usage: Option<AgentUsage>,
        error: Option<String>,
    ) -> Option<AgentUsageRecordInsert> {
        let now = now_ms();
        {
            let mut contexts = self
                .usage_contexts
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            let state = contexts.get_mut(run_id)?;
            state
                .context
                .provider_usage_semantics
                .merge_usage(&mut state.usage, usage);
            state.status = status;
            if let Some(error) = error {
                state.error = Some(error);
            }

            let usage = state.usage.clone();
            let billable_request_count = usage
                .as_ref()
                .and_then(|usage| usage.billable_request_count)
                .unwrap_or(0);
            let input_tokens = usage.as_ref().and_then(|usage| usage.input_tokens);
            let cached_input_tokens = usage.as_ref().and_then(|usage| usage.cached_input_tokens);
            let output_tokens = usage.as_ref().and_then(|usage| usage.output_tokens);
            let billable_output_tokens = usage.as_ref().and_then(|usage| {
                state
                    .context
                    .provider_usage_semantics
                    .billable_output_tokens(usage)
            });
            let estimated_cost = self.storage.estimate_usage_cost(
                input_tokens,
                cached_input_tokens,
                billable_output_tokens,
                state.context.input_price.as_deref().unwrap_or(""),
                state.context.cached_input_price.as_deref().unwrap_or(""),
                state.context.output_price.as_deref().unwrap_or(""),
            );

            Some(AgentUsageRecordInsert {
                id: format!("usage-{}", state.context.run_id),
                conversation_id: state.context.conversation_id.clone(),
                message_id: state.context.assistant_message_id.clone(),
                run_id: state.context.run_id.clone(),
                project_id: state.context.project_id.clone(),
                model_id: state.context.model_id.clone(),
                model_name: state.context.model_name.clone(),
                started_at: Some(state.context.started_at),
                completed_at: is_terminal_run_status(status).then_some(now),
                status: Some(run_status_label(status).to_string()),
                error: state.error.clone(),
                created_at: now,
                input_tokens,
                output_tokens,
                output_thinking_tokens: usage
                    .as_ref()
                    .and_then(|usage| usage.output_thinking_tokens),
                total_tokens: usage.as_ref().and_then(|usage| usage.total_tokens),
                cached_input_tokens: usage.as_ref().and_then(|usage| usage.cached_input_tokens),
                cache_creation_input_tokens: usage
                    .as_ref()
                    .and_then(|usage| usage.cache_creation_input_tokens),
                billable_request_count,
                input_price: state.context.input_price.clone(),
                cached_input_price: state.context.cached_input_price.clone(),
                output_price: state.context.output_price.clone(),
                estimated_cost,
            })
        }
    }

    pub(super) fn finish_persisted_run_usage(&self, run_id: &str, status: AgentRunStatus) {
        if is_terminal_run_status(status) {
            let mut contexts = self
                .usage_contexts
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            let conversation_id = contexts
                .remove(run_id)
                .map(|state| state.context.conversation_id);
            drop(contexts);
            if let Some(conversation_id) = conversation_id {
                if let Err(error) = self
                    .command_sessions
                    .release_managed_workspace(&conversation_id, run_id)
                {
                    eprintln!("failed to release managed command workspace: {error}");
                }
            }
        }
    }

    pub fn get_usage_summary(
        &self,
        input: &AgentUsageSummaryInput,
    ) -> Result<AgentUsageSummaryOutput, String> {
        self.storage.get_usage_summary(input, now_ms())
    }

    pub fn clear_usage_records(
        &self,
        input: &AgentUsageClearInput,
    ) -> Result<AgentUsageClearOutput, String> {
        self.storage.clear_usage_records(input)
    }
}

#[cfg(test)]
mod thinking_usage_tests {
    use super::AgentUsage;
    use mycopilot_core::ProviderUsageSemantics;

    fn usage(output: u64, thinking: u64, total: u64) -> AgentUsage {
        AgentUsage {
            input_tokens: Some(100),
            output_tokens: Some(output),
            output_thinking_tokens: Some(thinking),
            total_tokens: Some(total),
            cached_input_tokens: None,
            cache_creation_input_tokens: None,
            billable_request_count: Some(1),
        }
    }

    #[test]
    fn bills_provider_authoritative_completion_without_double_counting_generic_usage() {
        assert_eq!(
            ProviderUsageSemantics::CompletionIncludesReasoning
                .billable_output_tokens(&usage(20, 80, 200)),
            Some(100)
        );
        assert_eq!(
            ProviderUsageSemantics::StandardAdditive.billable_output_tokens(&usage(100, 80, 200)),
            Some(100)
        );
        assert_eq!(
            ProviderUsageSemantics::CompletionIncludesReasoning
                .billable_output_tokens(&usage(20, 80, 110)),
            Some(10)
        );
        let missing_breakdown = AgentUsage {
            input_tokens: Some(100),
            output_tokens: None,
            output_thinking_tokens: None,
            total_tokens: Some(175),
            cached_input_tokens: None,
            cache_creation_input_tokens: None,
            billable_request_count: Some(1),
        };
        assert_eq!(
            ProviderUsageSemantics::CompletionIncludesReasoning
                .billable_output_tokens(&missing_breakdown),
            Some(75)
        );
    }

    #[test]
    fn deepseek_approval_segments_do_not_revive_partial_output_breakdowns() {
        let mut total = Some(usage(20, 80, 200));
        ProviderUsageSemantics::CompletionIncludesReasoning.merge_usage(
            &mut total,
            Some(AgentUsage {
                input_tokens: Some(120),
                output_tokens: None,
                output_thinking_tokens: None,
                total_tokens: Some(170),
                cached_input_tokens: None,
                cache_creation_input_tokens: None,
                billable_request_count: Some(1),
            }),
        );
        let total = total.unwrap();
        assert_eq!(total.input_tokens, Some(220));
        assert_eq!(total.total_tokens, Some(370));
        assert_eq!(total.output_tokens, None);
        assert_eq!(total.output_thinking_tokens, None);
        assert_eq!(
            ProviderUsageSemantics::CompletionIncludesReasoning.billable_output_tokens(&total),
            Some(150)
        );
    }
}
