use super::*;
use crate::{
    storage::models::ChatConversationRecord, ConversationTurnTrace, ConversationTurnTraceItem,
};

/// Match a display row to both the durable mailbox origin and the consuming turn. Message order
/// is a persisted admission fact: startup input precedes its assistant; accept appends after it.
/// Never infer consumption from timestamps, an accept receipt, or a renderer-supplied field.
pub fn delivery_presentations(
    conversation: &ChatConversationRecord,
    trace: &ConversationTurnTrace,
    origins: &[(String, Input)],
) -> Vec<DeliveryPresentation> {
    if trace.conversation_id != conversation.id {
        return vec![];
    }
    let Some(assistant_index) = conversation.messages.iter().position(|message| {
        message.id == trace.assistant_message_id && message.role == "assistant"
    }) else {
        return vec![];
    };
    let mut result = vec![];
    for item in &trace.items {
        let ConversationTurnTraceItem::WorkflowDelivery {
            sequence,
            input_id,
            instance_id,
            workflow_name,
            content,
            created_at,
            truncated,
        } = item
        else {
            continue;
        };
        if *truncated {
            continue;
        }
        let mut candidates = origins
            .iter()
            .filter(|(_, input)| {
                input.id == *input_id && input.instance_id == *instance_id
                && input.content == *content && !input.messages.is_empty()
                // Forks retain backend-owned origins while remapping their run/message IDs.
                // For an original recipient, require its actual owning run as well.
                && (input.conversation_id.as_deref() != Some(conversation.id.as_str())
                    || input.run_id.as_deref() == Some(trace.run_id.as_str()))
            })
            .filter_map(|(delivery_id, input)| {
                conversation
                    .messages
                    .iter()
                    .position(|message| {
                        message.id == *delivery_id
                            && message.role == "user"
                            && message.content == *content
                    })
                    .map(|index| (delivery_id, input, index))
            });
        let Some((delivery_id, input, position)) = candidates.next() else {
            continue;
        };
        // An ambiguous origin or a startup letter must retain its existing top-level view.
        if candidates.next().is_some() || position <= assistant_index {
            continue;
        }
        result.push(DeliveryPresentation {
            conversation_id: conversation.id.clone(),
            run_id: trace.run_id.clone(),
            assistant_message_id: trace.assistant_message_id.clone(),
            input_id: input_id.clone(),
            delivery_id: delivery_id.clone(),
            instance_id: instance_id.clone(),
            workflow_name: workflow_name.clone(),
            content: content.clone(),
            created_at: *created_at,
            sequence: *sequence,
            sources: input
                .messages
                .iter()
                .map(|message| DeliverySource {
                    node_id: message.source_node_id.clone(),
                    node_name: message.source_node_name.clone(),
                    conversation_id: message.source_conversation_id.clone(),
                    conversation_title: message.source_conversation_title.clone(),
                    content: message.content.clone(),
                })
                .collect(),
        });
    }
    result
}
