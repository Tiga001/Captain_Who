use super::*;

pub(super) struct AgentEventStream {
    events: Vec<AgentEvent>,
    emitter: Option<AgentEventEmitter>,
}

impl AgentEventStream {
    pub(super) fn new(emitter: Option<AgentEventEmitter>) -> Self {
        Self {
            events: Vec::new(),
            emitter,
        }
    }

    pub(super) fn emit(&mut self, event: AgentEvent) {
        if let Some(emitter) = &self.emitter {
            emitter(event.clone());
        }
        self.events.push(event);
    }

    pub(super) fn emit_transient(&self, event: AgentEvent) {
        if let Some(emitter) = &self.emitter {
            emitter(event);
        }
    }

    pub(super) fn into_events(self) -> Vec<AgentEvent> {
        self.events
    }
}

pub(super) fn emit_tool_input_preview(
    event_stream: &mut AgentEventStream,
    run_id: &str,
    preview: crate::tools::ToolInputStreamPreview,
) {
    match preview {
        crate::tools::ToolInputStreamPreview::FileWrite(preview) => {
            event_stream.emit_transient(AgentEvent::FileWritePreviewUpdated {
                run_id: run_id.to_string(),
                preview,
            });
        }
    }
}

pub(super) fn emit_context_manifest_if_enabled(
    run_id: &str,
    request_index: usize,
    context: &ContextFrame,
    tools: &[AgentToolDefinition],
) {
    if !context_diagnostics_enabled() {
        return;
    }

    let tool_definition_characters = tools
        .iter()
        .filter_map(|tool| serde_json::to_string(tool).ok())
        .map(|tool| tool.chars().count())
        .sum::<usize>();
    let manifest = json!({
        "items": context.manifest().entries,
        "toolDefinitions": {
            "count": tools.len(),
            "serializedCharacterCount": tool_definition_characters,
        }
    });
    match serde_json::to_string(&manifest) {
        Ok(manifest) => {
            eprintln!("[context-manifest] run={run_id} request={request_index} {manifest}")
        }
        Err(error) => eprintln!(
            "[context-manifest] run={run_id} request={request_index} serialization_error={error}"
        ),
    }
}

pub(super) fn emit_context_budget_if_enabled(
    run_id: &str,
    request_index: usize,
    report: &ContextBudgetReport,
    compaction_query: &ContextCompactionQuery,
    compaction_plan: &ContextCompactionPlan,
) {
    if !context_diagnostics_enabled() {
        return;
    }

    let diagnostic = json!({
        "capacity": report,
        "compactionQuery": compaction_query,
        "compactionPlan": compaction_plan,
    });
    match serde_json::to_string(&diagnostic) {
        Ok(diagnostic) => {
            eprintln!("[context-budget] run={run_id} request={request_index} {diagnostic}")
        }
        Err(error) => eprintln!(
            "[context-budget] run={run_id} request={request_index} serialization_error={error}"
        ),
    }
}

pub(super) fn emit_compaction_skip_if_required(
    run_id: &str,
    request_index: usize,
    compaction_attempts: usize,
    executor_available: bool,
    compaction_plan: &ContextCompactionPlan,
) {
    if compaction_plan.status == ContextCompactionPlanStatus::NotRequired {
        return;
    }

    let reason = compaction_skip_reason(compaction_plan, compaction_attempts, executor_available);
    eprintln!(
        "[context-compaction] skipped run={run_id} request={request_index} attempt={} status={:?} reason={reason} compactable={} planned={} protected={}",
        compaction_attempts.saturating_add(1),
        compaction_plan.status,
        compaction_plan.compactable_input_tokens,
        compaction_plan.planned_reclaimed_tokens,
        compaction_plan.protected.input_tokens,
    );
}

fn compaction_skip_reason(
    plan: &ContextCompactionPlan,
    compaction_attempts: usize,
    executor_available: bool,
) -> &'static str {
    if compaction_attempts >= super::MAX_CONTEXT_COMPACTION_ATTEMPTS_PER_REQUEST {
        return "attempt_budget_exhausted";
    }
    if !executor_available {
        return "executor_unavailable";
    }
    match plan.status {
        ContextCompactionPlanStatus::Unconfigured => "unconfigured",
        ContextCompactionPlanStatus::InvalidConfiguration => "invalid_configuration",
        ContextCompactionPlanStatus::InsufficientCompactableContext => {
            "insufficient_compactable_context"
        }
        ContextCompactionPlanStatus::Required => {
            if plan
                .steps
                .first()
                .is_some_and(|step| step.durable_prefix.is_none())
            {
                "missing_durable_prefix"
            } else {
                "required_without_executable_step"
            }
        }
        ContextCompactionPlanStatus::NotRequired => "not_required",
    }
}

pub(super) fn context_diagnostics_enabled() -> bool {
    std::env::var("MYCOPILOT_CONTEXT_MANIFEST")
        .ok()
        .is_some_and(|value| matches!(value.trim(), "1" | "true" | "yes"))
}
