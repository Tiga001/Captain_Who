use super::*;

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
        contexts.remove(run_id);
        drop(contexts);
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
        let (mut usage, provider_profile_id) = {
            let contexts = self
                .usage_contexts
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            let state = contexts.get(run_id)?;
            (state.usage.clone(), state.context.provider_profile_id)
        };
        merge_usage_for_profile(&mut usage, next, provider_profile_id);
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
            merge_usage_for_profile(&mut state.usage, usage, state.context.provider_profile_id);
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
            let output_tokens = usage.as_ref().and_then(|usage| usage.output_tokens);
            let billable_output_tokens = usage
                .as_ref()
                .and_then(|usage| billable_output_tokens(usage, state.context.provider_profile_id));
            let estimated_cost = self.storage.estimate_usage_cost(
                input_tokens,
                billable_output_tokens,
                state.context.input_price.as_deref().unwrap_or(""),
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
            contexts.remove(run_id);
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

fn merge_usage_for_profile(
    total: &mut Option<AgentUsage>,
    next: Option<AgentUsage>,
    provider_profile_id: ProviderProfileId,
) {
    if provider_profile_id == ProviderProfileId::DeepSeekV4Chat {
        merge_usage_with_disjoint_reasoning(total, next);
    } else {
        merge_usage(total, next);
    }
}

fn billable_output_tokens(
    usage: &AgentUsage,
    provider_profile_id: ProviderProfileId,
) -> Option<u64> {
    let visible_output = usage.output_tokens;
    if provider_profile_id != ProviderProfileId::DeepSeekV4Chat {
        return visible_output;
    }
    match (usage.total_tokens, usage.input_tokens) {
        (Some(total), Some(input)) => total
            .checked_sub(input)
            .or_else(|| {
                visible_output
                    .zip(usage.output_thinking_tokens)
                    .and_then(|(visible, thinking)| visible.checked_add(thinking))
            })
            .or(visible_output),
        _ => visible_output
            .zip(usage.output_thinking_tokens)
            .and_then(|(visible, thinking)| visible.checked_add(thinking))
            .or(visible_output),
    }
}

#[cfg(test)]
mod thinking_usage_tests {
    use super::{billable_output_tokens, merge_usage_for_profile, ProviderProfileId};
    use mycopilot_core::AgentUsage;

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
            billable_output_tokens(&usage(20, 80, 200), ProviderProfileId::DeepSeekV4Chat),
            Some(100)
        );
        assert_eq!(
            billable_output_tokens(&usage(100, 80, 200), ProviderProfileId::GenericOpenAiChat),
            Some(100)
        );
        assert_eq!(
            billable_output_tokens(&usage(20, 80, 110), ProviderProfileId::DeepSeekV4Chat),
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
            billable_output_tokens(&missing_breakdown, ProviderProfileId::DeepSeekV4Chat),
            Some(75)
        );
    }

    #[test]
    fn deepseek_approval_segments_do_not_revive_partial_output_breakdowns() {
        let mut total = Some(usage(20, 80, 200));
        merge_usage_for_profile(
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
            ProviderProfileId::DeepSeekV4Chat,
        );
        let total = total.unwrap();
        assert_eq!(total.input_tokens, Some(220));
        assert_eq!(total.total_tokens, Some(370));
        assert_eq!(total.output_tokens, None);
        assert_eq!(total.output_thinking_tokens, None);
        assert_eq!(
            billable_output_tokens(&total, ProviderProfileId::DeepSeekV4Chat),
            Some(150)
        );
    }
}
