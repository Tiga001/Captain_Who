use super::*;
use crate::{
    AgentContextCheckpointToolCall, AgentProviderToolCallIdentity, ConversationModelContextItem,
};

fn model_rows(trace: &ConversationTurnTrace) -> Vec<ConversationModelContextItem> {
    trace
        .items
        .iter()
        .filter(|item| item.is_model_visible())
        .map(|item| {
            let mut row = ConversationModelContextItem {
                sequence: item.sequence(),
                ordinal: 0,
                role: "user".into(),
                content: String::new(),
                images: Vec::new(),
                tool_call_id: None,
                tool_calls: Vec::new(),
                is_error: false,
            };
            match item {
                ConversationTurnTraceItem::ToolCall {
                    call_id,
                    tool,
                    operation,
                    ..
                } => {
                    row.role = "assistant".into();
                    row.tool_calls.push(AgentContextCheckpointToolCall {
                        id: call_id.clone(),
                        name: tool.clone(),
                        args: operation.clone(),
                        provider_identity: AgentProviderToolCallIdentity {
                            provider_tool_index: 0,
                            provider_call_id: "opaque_provider_identity".into(),
                            runtime_call_id: call_id.clone(),
                        },
                    });
                }
                ConversationTurnTraceItem::ToolResult {
                    call_id, success, ..
                } => {
                    row.role = "tool".into();
                    row.tool_call_id = Some(call_id.clone());
                    row.content = r#"{"content":"exact retained tool projection"}"#.into();
                    row.is_error = !success;
                }
                ConversationTurnTraceItem::WorkflowDelivery { content, .. } => {
                    row.content = content.clone()
                }
                ConversationTurnTraceItem::AssistantNarration { content, .. } => {
                    row.role = "assistant".into();
                    row.content = content.clone();
                }
                _ => panic!("unsupported test trace item"),
            }
            row
        })
        .collect()
}

fn save_rows(
    connection: &Connection,
    trace: &ConversationTurnTrace,
    rows: &[ConversationModelContextItem],
) {
    conversation_model_context_repository::commit_items_in_connection(
        connection,
        &trace.conversation_id,
        &trace.assistant_message_id,
        rows,
    )
    .unwrap();
}

#[test]
fn model_source_uses_saved_tool_projection_and_deduplicates_workflow_without_changing_journal() {
    let mut connection = setup();
    connection.execute(
        "UPDATE messages SET id='workflow-message-input-1',content='UI workflow mail body' WHERE id='user-2'", [],
    ).unwrap();
    let mut trace =
        conversation_trace_repository::get_trace_for_message(&connection, "assistant-2")
            .unwrap()
            .unwrap();
    trace
        .items
        .push(ConversationTurnTraceItem::WorkflowDelivery {
            sequence: 2,
            input_id: "input-1".into(),
            instance_id: "workflow-1".into(),
            workflow_name: "Research".into(),
            content: "<workflow_delivery>authoritative workflow mail body</workflow_delivery>"
                .into(),
            created_at: 4,
            truncated: false,
        });
    conversation_trace_repository::commit_trace_in_connection(&connection, &trace, 3, 4).unwrap();
    save_rows(&connection, &trace, &model_rows(&trace));
    let cursor = ContextJournalCursor::trace_item("assistant-2", 2);
    let mut prefix = prepare_prefix(&connection, "conversation-1", &cursor).unwrap();
    let canonical_json = serde_json::to_string(&prefix).unwrap();
    let source = project_model_source_items(&connection, &prefix).unwrap();
    let serialized = serde_json::to_string(&source).unwrap();
    assert!(serialized.contains("exact retained tool projection"));
    assert!(
        !serialized.contains("page body"),
        "never regenerate tool results from the lossy audit"
    );
    assert!(!serialized.contains("UI workflow mail body"));
    assert_eq!(
        serialized
            .matches("authoritative workflow mail body")
            .count(),
        1
    );
    assert!(!serialized.contains("opaque_provider_identity"));
    assert_eq!(source.last().unwrap().created_at, 4);
    assert!(prefix.source_items.iter().any(|item| matches!(item, ContextCompactionSourceItem::Message { content, .. } if content == "UI workflow mail body")));
    prefix.model_source_items = Some(source);
    assert_eq!(
        serde_json::to_string(&prefix).unwrap(),
        canonical_json,
        "projection is never persisted or hashed into the raw journal identity"
    );
    assert_eq!(
        prepare_prefix(&connection, "conversation-1", &cursor)
            .unwrap()
            .source_revision,
        prefix.source_revision
    );
    commit_prefix_replacement(
        &mut connection,
        &prefix,
        draft(&prefix, "projected-summary"),
        "assistant-2",
    )
    .unwrap();
    let raw =
        read_adopted_summary_prefix(&connection, "conversation-1", "projected-summary").unwrap();
    assert_eq!(
        raw, prefix.source_items,
        "history retrieval retains original source records after compaction"
    );
}

#[test]
fn model_source_rejects_missing_projection_instead_of_expanding_the_raw_trace() {
    let connection = setup();
    let prefix = prepare_prefix(
        &connection,
        "conversation-1",
        &ContextJournalCursor::trace_item("assistant-2", 1),
    )
    .unwrap();
    let error = project_model_source_items(&connection, &prefix).unwrap_err();
    assert!(error
        .to_string()
        .contains("model context must cover every closed trace item"));
}

#[test]
fn model_source_deduplicates_only_bound_narration_within_the_selected_prefix() {
    let mut connection = setup();
    let second_call_id = crate::llm::model_response_tool_call_id("run-2", 1, 0, "call-2");
    let mut trace =
        conversation_trace_repository::get_trace_for_message(&connection, "assistant-2")
            .unwrap()
            .unwrap();
    trace
        .items
        .push(ConversationTurnTraceItem::AssistantNarration {
            sequence: 2,
            content: "shared narration".into(),
            provider_turn_id: None,
            first_tool_call_id: None,
            truncated: false,
        });
    trace
        .items
        .push(ConversationTurnTraceItem::AssistantNarration {
            sequence: 3,
            content: "shared narration".into(),
            provider_turn_id: Some("turn-2".into()),
            first_tool_call_id: Some(second_call_id.clone()),
            truncated: false,
        });
    let mut call = trace.items[0].clone();
    if let ConversationTurnTraceItem::ToolCall {
        sequence, call_id, ..
    } = &mut call
    {
        *sequence = 4;
        *call_id = second_call_id.clone();
    }
    let mut result = trace.items[1].clone();
    if let ConversationTurnTraceItem::ToolResult {
        sequence, call_id, ..
    } = &mut result
    {
        *sequence = 5;
        *call_id = second_call_id;
    }
    trace.items.extend([call, result]);
    conversation_trace_repository::commit_trace_in_connection(&connection, &trace, 3, 4).unwrap();
    let mut rows = model_rows(&trace);
    rows[4].content = "shared narration".into();
    save_rows(&connection, &trace, &rows);
    let early = prepare_prefix(
        &connection,
        "conversation-1",
        &ContextJournalCursor::trace_item("assistant-2", 3),
    )
    .unwrap();
    let early_source = project_model_source_items(&connection, &early).unwrap();
    assert_eq!(
        early_source
            .iter()
            .filter(|item| item.content == "shared narration")
            .count(),
        2,
        "a call outside the prefix cannot absorb covered narration"
    );
    let complete = prepare_prefix(
        &connection,
        "conversation-1",
        &ContextJournalCursor::trace_item("assistant-2", 5),
    )
    .unwrap();
    let complete_source = project_model_source_items(&connection, &complete).unwrap();
    assert_eq!(
        complete_source
            .iter()
            .filter(|item| item.content == "shared narration")
            .count(),
        2,
        "unbound narration remains, while bound narration belongs only to its call"
    );
    assert!(!complete_source
        .iter()
        .any(|item| item.cursor == ContextJournalCursor::trace_item("assistant-2", 3)));
    commit_prefix_replacement(
        &mut connection,
        &early,
        draft(&early, "narration-summary"),
        "assistant-2",
    )
    .unwrap();
    let recursive = prepare_prefix(
        &connection,
        "conversation-1",
        &ContextJournalCursor::trace_item("assistant-2", 5),
    )
    .unwrap();
    let recursive_source = project_model_source_items(&connection, &recursive).unwrap();
    assert_eq!(
        recursive_source.len(),
        2,
        "recursive compaction takes only the newly covered exchange"
    );
    assert!(recursive_source
        .iter()
        .all(|item| item.cursor.trace_sequence().unwrap() >= 4));
}
