//! Bind durable inbox facts at an empty-tool-batch sampling boundary.
use super::*;

pub(super) struct SamplingInboxContext<'a> {
    pub(super) collaboration_inbox: Option<&'a dyn AgentSamplingBoundaryInbox>,
    pub(super) workflow_inbox: Option<&'a dyn crate::AgentWorkflowInbox>,
    pub(super) human_interaction_runtime: Option<&'a dyn AgentHumanInteractionRuntimeHost>,
    pub(super) trace_conversation_id: Option<&'a str>,
    pub(super) trace_assistant_message_id: Option<&'a str>,
    pub(super) run_id: &'a str,
    pub(super) next_model_request_index: usize,
    pub(super) interactive_root: bool,
    pub(super) workflow_only_bootstrap: bool,
    pub(super) active_context: &'a mut ContextFrame,
    pub(super) conversation_trace: &'a Arc<Mutex<ConversationTraceRecorder>>,
    pub(super) trace_observer: Option<&'a AgentConversationTraceObserver>,
}

/// Each inbox sees the sequence after the preceding delivery was applied.
/// Only the individual idempotent Host bindings may retry on the same boundary.
pub(super) fn bind_sampling_inboxes(context: SamplingInboxContext<'_>) -> AgentResult<()> {
    let SamplingInboxContext {
        collaboration_inbox,
        workflow_inbox,
        human_interaction_runtime,
        trace_conversation_id,
        trace_assistant_message_id,
        run_id,
        next_model_request_index,
        interactive_root,
        workflow_only_bootstrap,
        active_context,
        conversation_trace,
        trace_observer,
    } = context;
    if let (Some(inbox), Some(conversation_id), Some(assistant_message_id)) = (
        collaboration_inbox,
        trace_conversation_id,
        trace_assistant_message_id,
    ) {
        let expected_next_trace_sequence = conversation_trace
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .next_sequence();
        if let Some(delivery) = inbox.bind_for_model_batch(AgentSamplingBoundaryRequest {
            conversation_id: conversation_id.to_string(),
            run_id: run_id.to_string(),
            assistant_message_id: assistant_message_id.to_string(),
            model_batch_index: u64::try_from(next_model_request_index.saturating_add(1))
                .unwrap_or(u64::MAX),
            expected_next_trace_sequence,
        })? {
            apply_agent_mailbox_delivery(
                &delivery,
                active_context,
                conversation_trace,
                trace_observer,
                assistant_message_id,
            )?;
        }
    }
    if interactive_root {
        if let (Some(inbox), Some(conversation_id), Some(assistant_message_id)) = (
            workflow_inbox,
            trace_conversation_id,
            trace_assistant_message_id,
        ) {
            let expected_next_trace_sequence = conversation_trace
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .next_sequence();
            let boundary = AgentSamplingBoundaryRequest {
                conversation_id: conversation_id.to_string(),
                run_id: run_id.to_string(),
                assistant_message_id: assistant_message_id.to_string(),
                model_batch_index: u64::try_from(next_model_request_index.saturating_add(1))
                    .unwrap_or(u64::MAX),
                expected_next_trace_sequence,
            };
            let deliveries = inbox
                .bind_for_model_batch(boundary.clone())
                .or_else(|_| inbox.bind_for_model_batch(boundary))?;
            if workflow_only_bootstrap && next_model_request_index == 0 && deliveries.is_empty() {
                return Err(AgentError::new(
                    "Organization startup mail is no longer available; no model request was sent.",
                ));
            }
            apply_workflow_deliveries(
                &deliveries,
                active_context,
                conversation_trace,
                trace_observer,
                assistant_message_id,
            )?;
        }
    }
    if interactive_root {
        if let (Some(host), Some(conversation_id), Some(assistant_message_id)) = (
            human_interaction_runtime,
            trace_conversation_id,
            trace_assistant_message_id,
        ) {
            let expected_next_trace_sequence = conversation_trace
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .next_sequence();
            let boundary = AgentSamplingBoundaryRequest {
                conversation_id: conversation_id.to_string(),
                run_id: run_id.to_string(),
                assistant_message_id: assistant_message_id.to_string(),
                model_batch_index: u64::try_from(next_model_request_index.saturating_add(1))
                    .unwrap_or(u64::MAX),
                expected_next_trace_sequence,
            };
            // Retry only this idempotent storage binding on the same boundary.
            // A commit-unknown return must not lose its already recorded facts;
            // no Provider request has been issued and no model request is retried.
            let events = host
                .bind_ignored_events(boundary.clone())
                .or_else(|_| host.bind_ignored_events(boundary))?;
            apply_human_interaction_ignored_events(
                &events,
                active_context,
                conversation_trace,
                trace_observer,
                assistant_message_id,
            )?;
        }
    }
    Ok(())
}
