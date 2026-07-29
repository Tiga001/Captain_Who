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
        let mut usage = {
            let contexts = self
                .usage_contexts
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            contexts.get(run_id).and_then(|state| state.usage.clone())
        };
        merge_usage(&mut usage, next);
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
            merge_usage(&mut state.usage, usage);
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
            let estimated_cost = self.storage.estimate_usage_cost(
                input_tokens,
                output_tokens,
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
